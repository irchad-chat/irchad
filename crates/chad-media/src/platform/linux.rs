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
use crate::{Device, Devices, RATE};

enum Cmd {
    Start(Option<String>, Option<String>),
    Stop,
}

pub struct PipeWire {
    tx: Mutex<pw::channel::Sender<Cmd>>,
    devices: Arc<Mutex<Devices>>,
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

impl PipeWire {
    pub fn spawn(shared: Arc<Shared>) -> Self {
        let (tx, rx) = pw::channel::channel();
        let devices = Arc::new(Mutex::new(Devices::default()));
        let d = devices.clone();
        let spawned = std::thread::Builder::new()
            .name("irchad-pipewire".into())
            .spawn(move || {
                if let Err(e) = run(&shared, rx, &d) {
                    tracing::error!("PipeWire: {e}");
                }
            });
        if let Err(e) = spawned {
            tracing::error!("starting the PipeWire thread: {e}");
        }
        Self {
            tx: Mutex::new(tx),
            devices,
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
}

type Streams = Vec<(pw::stream::StreamRc, pw::stream::StreamListener<Vec<i16>>)>;

fn run(
    shared: &Arc<Shared>,
    rx: pw::channel::Receiver<Cmd>,
    devices: &Arc<Mutex<Devices>>,
) -> Result<(), pw::Error> {
    // SAFETY: no arguments; idempotent (a function-local static inside).
    if !unsafe { webrtc_initialize_pipewire() } {
        tracing::error!("libwebrtc couldn't load libpipewire");
    }
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;

    // id → (is input, device)
    let nodes: Rc<RefCell<HashMap<u32, (bool, Device)>>> = Rc::default();
    let publish = {
        let (nodes, devices, shared) = (nodes.clone(), devices.clone(), shared.clone());
        move || {
            let mut d = Devices::default();
            for (input, dev) in nodes.borrow().values() {
                if *input {
                    &mut d.inputs
                } else {
                    &mut d.outputs
                }
                .push(dev.clone());
            }
            d.inputs.sort_by(|a, b| a.name.cmp(&b.name));
            d.outputs.sort_by(|a, b| a.name.cmp(&b.name));
            *lock(&devices) = d;
            (shared.notify)();
        }
    };
    let publish = Rc::new(publish);
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let (nodes, publish) = (nodes.clone(), publish.clone());
            move |g| {
                let Some(props) = g.props.filter(|_| g.type_ == ObjectType::Node) else {
                    return;
                };
                let input = match props.get(*pw::keys::MEDIA_CLASS) {
                    Some("Audio/Source") => true,
                    Some("Audio/Sink") => false,
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
                nodes.borrow_mut().insert(g.id, (input, dev));
                publish();
            }
        })
        .global_remove({
            let (nodes, publish) = (nodes.clone(), publish.clone());
            move |id| {
                if nodes.borrow_mut().remove(&id).is_some() {
                    publish();
                }
            }
        })
        .register();

    let streams: Rc<RefCell<Streams>> = Rc::default();
    let _rx = rx.attach(mainloop.loop_(), {
        let (core, shared) = (core.clone(), shared.clone());
        move |cmd| {
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

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::S16LE);
    info.set_rate(RATE);
    info.set_channels(1);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);
    let bytes = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .map_err(|_| pw::Error::CreationFailed)?
    .0
    .into_inner();
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
