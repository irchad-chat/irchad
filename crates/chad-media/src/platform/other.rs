//! cpal: WASAPI on Windows (shared mode), CoreAudio by coincidence on macOS.
//! Streams live on their own thread because cpal's aren't `Send` everywhere.

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

use super::Backend;
use crate::engine::{Shared, lock};
use crate::{Device, Devices, RATE};

enum Cmd {
    Start(Option<String>, Option<String>),
    Stop,
    /// A stream died (device unplugged, say): reopen on the same settings,
    /// which falls back to the default if the device is gone.
    Restart,
}

pub struct Cpal {
    tx: Mutex<Sender<Cmd>>,
    devices: Mutex<(Option<Instant>, Devices)>,
    #[cfg(windows)]
    app: Mutex<Option<super::loopback::Capture>>,
    /// Filled off the UI thread: MF enumeration can take a while.
    #[cfg(windows)]
    cameras: Arc<Mutex<Vec<Device>>>,
    #[cfg(windows)]
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Cpal {
    pub fn spawn(shared: Arc<Shared>) -> Self {
        #[cfg(windows)]
        let notify = shared.notify.clone();
        let (tx, rx) = channel();
        let restart = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("irchad-audio".into())
            .spawn(move || {
                let mut streams = Vec::new();
                let (mut last, mut running) = ((None, None), false);
                while let Ok(cmd) = rx.recv() {
                    streams.clear();
                    match cmd {
                        Cmd::Start(i, o) => (last, running) = ((i, o), true),
                        Cmd::Stop => running = false,
                        Cmd::Restart => std::thread::sleep(Duration::from_millis(500)),
                    }
                    if !running {
                        continue;
                    }
                    for input in [true, false] {
                        let id = if input { &last.0 } else { &last.1 };
                        match open(&shared, input, id.as_deref(), restart.clone()) {
                            Ok(s) => streams.push(s),
                            Err(e) => tracing::error!("audio {}: {e}", direction(input)),
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::error!("starting the audio thread: {e}");
        }
        Self {
            tx: Mutex::new(tx),
            devices: Mutex::new((None, Devices::default())),
            #[cfg(windows)]
            app: Mutex::default(),
            #[cfg(windows)]
            cameras: Arc::default(),
            #[cfg(windows)]
            notify,
        }
    }
}

impl Backend for Cpal {
    fn start(&self, input: Option<String>, output: Option<String>) {
        let _ = lock(&self.tx).send(Cmd::Start(input, output));
    }

    fn stop(&self) {
        let _ = lock(&self.tx).send(Cmd::Stop);
    }

    /// Enumerating is slow on WASAPI, so at most every 2 s.
    fn devices(&self) -> Devices {
        let mut d = lock(&self.devices);
        if d.0.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            let host = cpal::default_host();
            let list = |devs: Result<_, _>| -> Vec<Device> {
                let devs: Box<dyn Iterator<Item = cpal::Device>> = match devs {
                    Ok(d) => Box::new(d),
                    Err(_) => Box::new(std::iter::empty()),
                };
                devs.filter_map(|d| {
                    Some(Device {
                        id: d.id().ok()?.to_string(),
                        name: d.description().ok()?.name().to_owned(),
                    })
                })
                .collect()
            };
            d.1 = Devices {
                inputs: list(
                    host.input_devices()
                        .map(|i| i.collect::<Vec<_>>().into_iter()),
                ),
                outputs: list(
                    host.output_devices()
                        .map(|i| i.collect::<Vec<_>>().into_iter()),
                ),
                cameras: Vec::new(),
            };
            #[cfg(windows)]
            {
                let (cameras, notify) = (self.cameras.clone(), self.notify.clone());
                let _ = std::thread::Builder::new()
                    .name("irchad-cameras".into())
                    .spawn(move || {
                        *lock(&cameras) = super::mf::cameras();
                        notify();
                    });
            }
            d.0 = Some(Instant::now());
        }
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut out = d.1.clone();
        #[cfg(windows)]
        {
            out.cameras = lock(&self.cameras).clone();
        }
        out
    }

    #[cfg(windows)]
    fn apps(&self) -> Vec<crate::screen::AudioApp> {
        super::loopback::apps()
    }

    #[cfg(windows)]
    fn capture_app(&self, app: Option<(String, crate::screen::AppSink)>) {
        *lock(&self.app) = app.and_then(|(id, sink)| super::loopback::start(&id, sink));
    }
}

/// Catches a device whose clock disagrees with the format it reports: a
/// Scarlett switched to 44.1 kHz behind Windows' back still says 48 kHz, so
/// everyone hears us pitched up and full of gaps. Counts what the callback
/// delivers against the wall clock.
#[derive(Default)]
struct RateCheck {
    since: Option<Instant>,
    samples: usize,
    warned: bool,
}

impl RateCheck {
    const WINDOW: Duration = Duration::from_secs(5);

    fn tick(&mut self, samples: usize, now: Instant) {
        let Some(since) = self.since else {
            // The first packets come in a burst; measure from after them.
            self.since = Some(now);
            return;
        };
        self.samples += samples;
        let elapsed = now.duration_since(since);
        if elapsed < Self::WINDOW {
            return;
        }
        let rate = self.samples as f64 / elapsed.as_secs_f64();
        let off = (rate / f64::from(RATE) - 1.0).abs();
        if off > 0.03 && !self.warned {
            tracing::warn!(
                "microphone delivers ~{rate:.0} samples/s but says {RATE} Hz: its clock \
                 changed under Windows (check the interface's sample rate); others will \
                 hear us sped up or slowed down, with gaps"
            );
            self.warned = true;
        } else if off < 0.01 && self.warned {
            tracing::info!("microphone clock back to ~{rate:.0} samples/s");
            self.warned = false;
        }
        (self.since, self.samples) = (Some(now), 0);
    }
}

fn direction(input: bool) -> &'static str {
    if input { "input" } else { "output" }
}

fn open(
    shared: &Arc<Shared>,
    input: bool,
    id: Option<&str>,
    restart: Sender<Cmd>,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let chosen = id
        .and_then(|id| id.parse().ok())
        .and_then(|id| host.device_by_id(&id));
    let device = match chosen {
        Some(d) => d,
        None if input => host.default_input_device().ok_or("no microphone")?,
        None => host.default_output_device().ok_or("no speakers")?,
    };
    let default = if input {
        device.default_input_config()
    } else {
        device.default_output_config()
    }
    .map_err(|e| e.to_string())?;
    let config = cpal::StreamConfig {
        channels: default.channels(),
        sample_rate: RATE,
        buffer_size: cpal::BufferSize::Default,
    };
    let stream = match default.sample_format() {
        cpal::SampleFormat::I16 => build::<i16>(shared, &device, config, input, restart),
        cpal::SampleFormat::I32 => build::<i32>(shared, &device, config, input, restart),
        cpal::SampleFormat::F32 => build::<f32>(shared, &device, config, input, restart),
        f => return Err(format!("unsupported sample format {f}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

/// Mono in the engine: the first channel in, every channel out.
fn build<T>(
    shared: &Arc<Shared>,
    device: &cpal::Device,
    config: cpal::StreamConfig,
    input: bool,
    restart: Sender<Cmd>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<i16> + Send + 'static,
    i16: FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    let shared = shared.clone();
    let mut scratch: Vec<i16> = Vec::with_capacity(8192);
    // WASAPI reports every glitch (routinely the first capture packet) as an
    // Xrun; restarting on those reopens the streams forever.
    let on_error = move |e: cpal::Error| {
        use cpal::ErrorKind::{DeviceChanged, RealtimeDenied, Xrun};
        if matches!(e.kind(), Xrun | DeviceChanged | RealtimeDenied) {
            tracing::debug!("audio stream: {e}");
            return;
        }
        tracing::warn!("audio stream: {e}");
        let _ = restart.send(Cmd::Restart);
    };
    if input {
        let mut clock = RateCheck::default();
        device.build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                scratch.clear();
                scratch.extend(data.chunks(channels).map(|f| f[0].to_sample::<i16>()));
                clock.tick(scratch.len(), Instant::now());
                shared.capture(&scratch);
            },
            on_error,
            None,
        )
    } else {
        device.build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                scratch.resize(data.len() / channels, 0);
                shared.render(&mut scratch);
                for (frame, &s) in data.chunks_mut(channels).zip(&scratch) {
                    frame.fill(T::from_sample(s));
                }
            },
            on_error,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_check_spots_a_44k_device_posing_as_48k() {
        let t0 = Instant::now();
        let mut c = RateCheck::default();
        c.tick(441, t0);
        for i in 1..=600 {
            c.tick(441, t0 + Duration::from_millis(10 * i));
        }
        assert!(c.warned);
        for i in 601..=1600 {
            c.tick(480, t0 + Duration::from_millis(10 * i));
        }
        assert!(!c.warned);
    }
}
