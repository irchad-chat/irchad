//! PipeWire, natively: named `Communication` streams that WirePlumber links to
//! the default devices (following them when the default changes or a device is
//! unplugged), and a registry watch for the device list (hotplug).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use pipewire as pw;
use pw::{properties::properties, spa, types::ObjectType};

use super::Backend;
use crate::engine::{Shared, lock};
use crate::screen::{AppSink, AudioApp};
use crate::{Device, Devices, RATE};

enum Cmd {
    Start(Option<String>, Option<String>),
    Stop,
    /// Capture an app's output for screen share (`object.serial`), or stop.
    App(Option<(String, AppSink)>),
}

pub struct PipeWire {
    tx: Mutex<pw::channel::Sender<Cmd>>,
    devices: Arc<Mutex<Devices>>,
    /// Registry id → app playing audio.
    apps: Arc<Mutex<HashMap<u32, AudioApp>>>,
}

unsafe extern "C" {
    /// `bool webrtc::InitializePipeWire()`: dlopens libpipewire and fills
    /// libwebrtc's lazy `pw_*` stubs. libwebrtc.a defines those stubs as weak
    /// symbols, and statically linked they win over libpipewire.so, so
    /// pipewire-rs calls them too. Until this runs each one is a null pointer
    /// (`docs/notes/livekit-sdk.md`).
    #[link_name = "_ZN6webrtc18InitializePipeWireEv"]
    fn webrtc_initialize_pipewire() -> bool;
}

/// Run before anything touches PipeWire, on any thread.
pub(super) fn init_pipewire() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: no arguments; idempotent (a function-local static inside).
        if !unsafe { webrtc_initialize_pipewire() } {
            tracing::error!("libwebrtc couldn't load libpipewire");
        }
        pw::init();
    });
}

impl PipeWire {
    pub fn spawn(shared: Arc<Shared>) -> Self {
        let (tx, rx) = pw::channel::channel();
        let devices = Arc::new(Mutex::new(Devices::default()));
        let apps = Arc::default();
        let (d, a) = (devices.clone(), Arc::clone(&apps));
        let spawned = std::thread::Builder::new()
            .name("irchad-pipewire".into())
            .spawn(move || {
                if let Err(e) = run(&shared, rx, &d, &a) {
                    tracing::error!("PipeWire: {e}");
                }
            });
        if let Err(e) = spawned {
            tracing::error!("starting the PipeWire thread: {e}");
        }
        Self {
            tx: Mutex::new(tx),
            devices,
            apps,
        }
    }

    fn send(&self, cmd: Cmd) {
        if lock(&self.tx).send(cmd).is_err() {
            tracing::warn!("PipeWire isn't running");
        }
    }
}

impl Backend for PipeWire {
    fn start(&self, input: Option<String>, output: Option<String>) {
        self.send(Cmd::Start(input, output));
    }

    fn stop(&self) {
        self.send(Cmd::Stop);
    }

    fn devices(&self) -> Devices {
        lock(&self.devices).clone()
    }

    fn apps(&self) -> Vec<AudioApp> {
        let mut apps: Vec<_> = lock(&self.apps).values().cloned().collect();
        apps.sort_by(|a, b| a.name.cmp(&b.name));
        apps
    }

    fn capture_app(&self, app: Option<(String, AppSink)>) {
        self.send(Cmd::App(app));
    }
}

type Streams = Vec<(pw::stream::StreamRc, pw::stream::StreamListener<Vec<i16>>)>;

fn run(
    shared: &Arc<Shared>,
    rx: pw::channel::Receiver<Cmd>,
    devices: &Arc<Mutex<Devices>>,
    apps: &Arc<Mutex<HashMap<u32, AudioApp>>>,
) -> Result<(), pw::Error> {
    init_pipewire();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;

    // id → (input 0, output 1 or camera 2, device)
    let nodes: Rc<RefCell<HashMap<u32, (usize, Device)>>> = Rc::default();
    let publish = {
        let (nodes, devices, shared) = (nodes.clone(), devices.clone(), shared.clone());
        move || {
            let mut d = Devices::default();
            for (kind, dev) in nodes.borrow().values() {
                [&mut d.inputs, &mut d.outputs, &mut d.cameras][*kind].push(dev.clone());
            }
            for list in [&mut d.inputs, &mut d.outputs, &mut d.cameras] {
                list.sort_by(|a, b| a.name.cmp(&b.name));
            }
            *lock(&devices) = d;
            (shared.notify)();
        }
    };
    let publish = Rc::new(publish);
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let (nodes, publish, apps) = (nodes.clone(), publish.clone(), apps.clone());
            let notify = shared.notify.clone();
            let me = std::process::id().to_string();
            move |g| {
                let Some(props) = g.props.filter(|_| g.type_ == ObjectType::Node) else {
                    return;
                };
                let kind = match props.get(*pw::keys::MEDIA_CLASS) {
                    Some("Audio/Source") => 0,
                    Some("Audio/Sink") => 1,
                    // Screen casts are video sources too; cameras say so.
                    Some("Video/Source") if props.get(*pw::keys::MEDIA_ROLE) == Some("Camera") => 2,
                    // An app playing something. Never our own voice output.
                    Some("Stream/Output/Audio")
                        if props.get(*pw::keys::APP_PROCESS_ID) != Some(me.as_str()) =>
                    {
                        let Some(serial) = props.get(*pw::keys::OBJECT_SERIAL) else {
                            return;
                        };
                        let app = props
                            .get(*pw::keys::APP_NAME)
                            .or_else(|| props.get(*pw::keys::NODE_NAME))
                            .unwrap_or("Unknown app");
                        let name = match props.get(*pw::keys::MEDIA_NAME) {
                            Some(m) if m != app => format!("{app}: {m}"),
                            _ => app.to_owned(),
                        };
                        let name = name.chars().take(80).collect();
                        lock(&apps).insert(
                            g.id,
                            AudioApp {
                                id: serial.to_owned(),
                                name,
                            },
                        );
                        notify();
                        return;
                    }
                    _ => return,
                };
                let Some(id) = props.get(*pw::keys::NODE_NAME) else {
                    return;
                };
                let name = props
                    .get(*pw::keys::NODE_DESCRIPTION)
                    .or_else(|| props.get(*pw::keys::NODE_NICK))
                    .unwrap_or(id);
                let dev = Device {
                    id: id.to_owned(),
                    name: name.to_owned(),
                };
                nodes.borrow_mut().insert(g.id, (kind, dev));
                publish();
            }
        })
        .global_remove({
            let (nodes, publish, apps) = (nodes, publish, apps.clone());
            move |id| {
                lock(&apps).remove(&id);
                if nodes.borrow_mut().remove(&id).is_some() {
                    publish();
                }
            }
        })
        .register();

    let streams: Rc<RefCell<Streams>> = Rc::default();
    let app_stream = RefCell::new(None);
    let _rx = rx.attach(mainloop.loop_(), {
        let (core, shared) = (core, shared.clone());
        move |cmd| {
            if let Cmd::App(app) = cmd {
                *app_stream.borrow_mut() = None;
                if let Some((target, sink)) = app {
                    match capture_app(&core, target, sink) {
                        Ok(s) => *app_stream.borrow_mut() = Some(s),
                        Err(e) => tracing::error!("PipeWire app capture: {e}"),
                    }
                }
                return;
            }
            // Dropping a stream disconnects it.
            streams.borrow_mut().clear();
            if let Cmd::Start(input, output) = cmd {
                for (dir, target) in [
                    (spa::utils::Direction::Input, input),
                    (spa::utils::Direction::Output, output),
                ] {
                    match stream(&core, &shared, dir, target) {
                        Ok(s) => streams.borrow_mut().push(s),
                        Err(e) => tracing::error!("PipeWire {dir:?} stream: {e}"),
                    }
                }
            }
        }
    });
    mainloop.run();
    Ok(())
}

fn stream(
    core: &pw::core::CoreRc,
    shared: &Arc<Shared>,
    dir: spa::utils::Direction,
    target: Option<String>,
) -> Result<(pw::stream::StreamRc, pw::stream::StreamListener<Vec<i16>>), pw::Error> {
    let capture = dir == spa::utils::Direction::Input;
    let (node, description, category) = if capture {
        ("irchad-mic", "IrChad microphone", "Capture")
    } else {
        ("irchad-voice", "IrChad voice", "Playback")
    };
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::MEDIA_CATEGORY => category,
        *pw::keys::APP_NAME => "IrChad",
        *pw::keys::NODE_NAME => node,
        *pw::keys::NODE_DESCRIPTION => description,
        // 10 ms quanta suit the 10 ms DSP frames.
        *pw::keys::NODE_LATENCY => "480/48000",
    };
    if let Some(t) = target {
        props.insert(*pw::keys::TARGET_OBJECT, t);
    }
    let stream = pw::stream::StreamRc::new(core.clone(), node, props)?;
    let shared = shared.clone();
    let listener = stream
        .add_local_listener_with_user_data(Vec::<i16>::with_capacity(8192))
        .process(move |stream, scratch| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let requested = buffer.requested() as usize;
            let data = &mut buffer.datas_mut()[0];
            if capture {
                let size = data.chunk().size() as usize;
                let Some(bytes) = data.data() else { return };
                let bytes = &bytes[..size.min(bytes.len())];
                scratch.clear();
                scratch.extend(
                    bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| i16::from_le_bytes(*b)),
                );
                shared.capture(scratch);
            } else {
                let Some(bytes) = data.data() else { return };
                let mut n = bytes.len() / 2;
                if requested > 0 {
                    n = n.min(requested);
                }
                scratch.resize(n, 0);
                shared.render(scratch);
                for (b, s) in bytes.as_chunks_mut::<2>().0.iter_mut().zip(scratch.iter()) {
                    *b = s.to_le_bytes();
                }
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = 2;
                *chunk.size_mut() = (n * 2) as u32;
            }
        })
        .register()?;

    let bytes = s16_format(1)?;
    let mut params = [spa::pod::Pod::from_bytes(&bytes).ok_or(pw::Error::CreationFailed)?];
    stream.connect(
        dir,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    Ok((stream, listener))
}

type AppStream = (pw::stream::StreamRc, pw::stream::StreamListener<Vec<i16>>);

/// Records one app's output: our input linked straight to its stream node.
/// It must never fall back to the default source, or the mic would go out
/// as screen share audio.
fn capture_app(
    core: &pw::core::CoreRc,
    target: String,
    mut sink: AppSink,
) -> Result<AppStream, pw::Error> {
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::APP_NAME => "IrChad",
        *pw::keys::NODE_NAME => "irchad-screen-audio",
        *pw::keys::NODE_DESCRIPTION => "IrChad screen share audio",
        *pw::keys::TARGET_OBJECT => target,
        "node.dont-reconnect" => "true",
        "node.dont-fallback" => "true",
    };
    let stream = pw::stream::StreamRc::new(core.clone(), "irchad-screen-audio", props)?;
    let listener = stream
        .add_local_listener_with_user_data(Vec::<i16>::new())
        .process(move |stream, scratch| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let data = &mut buffer.datas_mut()[0];
            let size = data.chunk().size() as usize;
            let Some(bytes) = data.data() else { return };
            let bytes = &bytes[..size.min(bytes.len())];
            scratch.clear();
            scratch.extend(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| i16::from_le_bytes(*b)),
            );
            sink(scratch);
        })
        .register()?;
    let bytes = s16_format(2)?;
    let mut params = [spa::pod::Pod::from_bytes(&bytes).ok_or(pw::Error::CreationFailed)?];
    // Not RT_PROCESS: the sink calls into libwebrtc, which may lock.
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;
    Ok((stream, listener))
}

/// S16LE at 48 kHz, mono or stereo.
fn s16_format(channels: u32) -> Result<Vec<u8>, pw::Error> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::S16LE);
    info.set_rate(RATE);
    info.set_channels(channels);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    if channels == 1 {
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    } else {
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    }
    info.set_position(position);
    Ok(spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .map_err(|_| pw::Error::CreationFailed)?
    .0
    .into_inner())
}
