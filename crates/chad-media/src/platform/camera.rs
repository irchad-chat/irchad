//! Linux cameras: the Camera portal grants access and hands us a PipeWire
//! remote that shows only cameras; we read the one picked in the settings
//! (by `node.name`) on our own stream, uncompressed.

use std::os::fd::OwnedFd;
use std::sync::Mutex;

use ashpd::desktop::camera::Camera;
use pipewire as pw;
use pw::spa;
use pw::spa::pod::{Object, Pod, Property, PropertyFlags, Value};
use pw::spa::sys as ss;
use pw::spa::utils::{Fraction, Rectangle, SpaTypes};

use super::portal::serialize;
use crate::screen::{Frame, FrameSink, MAX_DIM, OnEnd, PixelFormat, ScreenSource};

fn failed(e: ashpd::Error) -> String {
    match e {
        ashpd::Error::Response(_) => "Camera access was denied.".into(),
        e => {
            tracing::warn!("Camera portal: {e}");
            "Couldn't open the camera.".into()
        }
    }
}

struct Running(Mutex<pw::channel::Sender<()>>);

impl ScreenSource for Running {}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = crate::engine::lock(&self.0).send(());
    }
}

/// Opens camera `id` (a `node.name`); `None`, or one that's gone, takes the default.
pub async fn start(
    id: Option<String>,
    sink: FrameSink,
    on_end: OnEnd,
) -> Result<Box<dyn ScreenSource>, String> {
    let proxy = Camera::new().await.map_err(failed)?;
    proxy
        .request_access(Default::default())
        .await
        .map_err(failed)?
        .response()
        .map_err(failed)?;
    if !proxy.is_present().await.unwrap_or(false) {
        return Err("No camera found.".into());
    }
    let fd = proxy
        .open_pipe_wire_remote(Default::default())
        .await
        .map_err(failed)?;
    let (quit, quit_rx) = pw::channel::channel();
    let (ready_tx, ready) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("irchad-camera".into())
        .spawn(move || {
            let mut ready_tx = Some(ready_tx);
            if let Err(e) = run(fd, id, sink, on_end, quit_rx, &mut ready_tx) {
                tracing::error!("camera: {e}");
                if let Some(tx) = ready_tx {
                    let _ = tx.send(false);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    if !ready.await.unwrap_or(false) {
        return Err("Couldn't open the camera.".into());
    }
    Ok(Box::new(Running(Mutex::new(quit))))
}

struct State {
    sink: FrameSink,
    on_end: Option<OnEnd>,
    format: Option<(PixelFormat, u32, u32)>,
    /// NV12 planes that came in two pieces, joined.
    joined: Vec<u8>,
    streaming: bool,
}

impl State {
    fn end(&mut self) {
        if let Some(f) = self.on_end.take() {
            f();
        }
    }
}

fn run(
    fd: OwnedFd,
    target: Option<String>,
    sink: FrameSink,
    on_end: OnEnd,
    quit: pw::channel::Receiver<()>,
    ready: &mut Option<tokio::sync::oneshot::Sender<bool>>,
) -> Result<(), pw::Error> {
    super::linux::init_pipewire();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(fd, None)?;
    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Camera",
    };
    if let Some(t) = target {
        props.insert("target.object", t);
    }
    let stream = pw::stream::StreamRc::new(core, "irchad-camera", props)?;
    let state = State {
        sink,
        on_end: Some(on_end),
        format: None,
        joined: Vec::new(),
        streaming: false,
    };
    let _listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(|_, st, _, new| match new {
            pw::stream::StreamState::Streaming => st.streaming = true,
            pw::stream::StreamState::Error(e) => {
                tracing::warn!("camera stream: {e}");
                st.end();
            }
            // Unplugged, or access revoked.
            pw::stream::StreamState::Unconnected if st.streaming => st.end(),
            _ => {}
        })
        .param_changed(|_, st, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::video::VideoInfoRaw::new();
            if info.parse(param).is_err() {
                return;
            }
            use spa::param::video::VideoFormat as F;
            let format = match info.format() {
                F::YUY2 => PixelFormat::Yuy2,
                F::NV12 => PixelFormat::Nv12,
                F::BGRx | F::BGRA => PixelFormat::Bgra,
                F::RGBx | F::RGBA => PixelFormat::Rgba,
                other => {
                    tracing::warn!("camera: unexpected format {other:?}");
                    st.format = None;
                    return;
                }
            };
            let size = info.size();
            let rate = info.framerate();
            tracing::info!(
                "camera: {}x{} {:?} at {}/{} fps",
                size.width,
                size.height,
                info.format(),
                rate.num,
                rate.denom
            );
            st.format = Some((format, size.width, size.height));
        })
        .process(process)
        .register()?;

    let format = format_pod()?;
    let buffers = buffers_pod()?;
    let mut params: Vec<&Pod> = [&format, &buffers]
        .into_iter()
        .filter_map(|b| Pod::from_bytes(b))
        .collect();
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;
    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    if let Some(tx) = ready.take() {
        let _ = tx.send(true);
    }
    mainloop.run();
    Ok(())
}

/// Uncompressed formats we convert from, near 720p, at 15 fps or better.
// shortcut: no MJPG, so a camera that only does 720p30 compressed falls back
// to a smaller uncompressed mode; decode MJPG if people ask for sharper video.
fn format_pod() -> Result<Vec<u8>, pw::Error> {
    use spa::param::format::{FormatProperties as P, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat as F;
    serialize(spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(P::MediaType, Id, MediaType::Video),
        spa::pod::property!(P::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            P::VideoFormat,
            Choice,
            Enum,
            Id,
            F::YUY2,
            F::YUY2,
            F::NV12,
            F::BGRx,
            F::BGRA,
            F::RGBx,
            F::RGBA
        ),
        spa::pod::property!(
            P::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1280,
                height: 720
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: MAX_DIM,
                height: MAX_DIM
            }
        ),
        spa::pod::property!(
            P::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 30, denom: 1 },
            Fraction { num: 15, denom: 1 },
            Fraction { num: 60, denom: 1 }
        ),
    ))
}

/// Memory we can read without a GPU.
fn buffers_pod() -> Result<Vec<u8>, pw::Error> {
    serialize(Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![Property {
            key: ss::SPA_PARAM_BUFFERS_dataType,
            flags: PropertyFlags::empty(),
            value: Value::Int((1 << ss::SPA_DATA_MemPtr) | (1 << ss::SPA_DATA_MemFd)),
        }],
    })
}

fn process(stream: &pw::stream::Stream, st: &mut State) {
    // SAFETY: a valid stream; each dequeued buffer is queued back below. Only
    // the newest buffer matters, so older ones go straight back.
    unsafe {
        let s = stream.as_raw_ptr();
        let mut b = pw::sys::pw_stream_dequeue_buffer(s);
        if b.is_null() {
            return;
        }
        loop {
            let next = pw::sys::pw_stream_dequeue_buffer(s);
            if next.is_null() {
                break;
            }
            pw::sys::pw_stream_queue_buffer(s, b);
            b = next;
        }
        let buf = (*b).buffer;
        if !buf.is_null() {
            read_buffer(&*buf, st);
        }
        pw::sys::pw_stream_queue_buffer(s, b);
    }
}

/// `rows` rows of a mapped data's valid chunk, and its stride.
///
/// # Safety
/// `d` belongs to a buffer the stream still owns.
unsafe fn plane(d: &ss::spa_data, rows: u32, row: u32) -> Option<(&[u8], u32)> {
    if d.data.is_null() || d.chunk.is_null() {
        return None;
    }
    // SAFETY: non-null chunk of this data.
    let c = unsafe { *d.chunk };
    if c.flags & ss::SPA_CHUNK_FLAG_CORRUPTED as i32 != 0 {
        return None;
    }
    let stride = if c.stride > 0 { c.stride as u32 } else { row };
    let need = stride as usize * rows as usize;
    if c.offset as usize + need > d.maxsize as usize {
        return None;
    }
    // SAFETY: inside the mapping (checked above).
    let bytes =
        unsafe { std::slice::from_raw_parts((d.data as *const u8).add(c.offset as usize), need) };
    Some((bytes, stride))
}

/// # Safety
/// `buf` is a buffer the stream handed us and still owns.
unsafe fn read_buffer(buf: &ss::spa_buffer, st: &mut State) {
    let Some((format, w, h)) = st.format else {
        return;
    };
    if buf.datas.is_null() || buf.n_datas == 0 {
        return;
    }
    // SAFETY: n_datas entries, per the stream.
    let datas = unsafe { std::slice::from_raw_parts(buf.datas, buf.n_datas as usize) };
    let (row, chroma) = match format {
        PixelFormat::Bgra | PixelFormat::Rgba => (w * 4, 0),
        PixelFormat::Yuy2 => (w.next_multiple_of(2) * 2, 0),
        PixelFormat::Nv12 => (w.next_multiple_of(2), h.div_ceil(2)),
    };
    // SAFETY: the stream's own buffer, for both calls.
    let found = match datas {
        [d] => unsafe { plane(d, h + chroma, row) },
        // NV12 in two planes (libcamera): joined if their strides agree.
        [y, uv, ..] if chroma > 0 => unsafe {
            match (plane(y, h, row), plane(uv, chroma, row)) {
                (Some((y, s)), Some((uv, s2))) if s == s2 => {
                    st.joined.clear();
                    st.joined.extend_from_slice(y);
                    st.joined.extend_from_slice(uv);
                    Some((st.joined.as_slice(), s))
                }
                _ => None,
            }
        },
        [d, ..] => unsafe { plane(d, h, row) },
        [] => None,
    };
    let Some((data, stride)) = found else {
        return;
    };
    // The encoder checks the frame again (`Frame::valid`).
    (st.sink)(Frame {
        data,
        width: w,
        height: h,
        stride,
        format,
    });
}
