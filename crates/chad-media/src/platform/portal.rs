//! Wayland (and X11 where the desktop serves it): the ScreenCast portal picks
//! the source and hands us a PipeWire node. We read it on our own PipeWire
//! connection, asking for linear DMA-BUF first and shared memory second, with
//! the cursor as metadata that we draw onto the frame ourselves.

use std::os::fd::OwnedFd;
use std::sync::Mutex;

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, StartCastOptions,
};
use pipewire as pw;
use pw::spa;
use pw::spa::pod::{Object, Pod, Property, PropertyFlags, Value};
use pw::spa::sys as ss;
use pw::spa::utils::{Fraction, Rectangle, SpaTypes};

use crate::screen::{Frame, FrameSink, MAX_DIM, OnEnd, PixelFormat, ScreenSource};

/// Largest cursor image we take from the metadata.
const CURSOR_MAX: u32 = 128;

pub async fn available() -> bool {
    match Screencast::new().await {
        Ok(p) => p
            .available_source_types()
            .await
            .is_ok_and(|t| !t.is_empty()),
        Err(_) => false,
    }
}

fn failed(e: ashpd::Error) -> String {
    match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
            "Screen sharing was cancelled.".into()
        }
        e => {
            tracing::warn!("ScreenCast portal: {e}");
            "Couldn't start screen sharing.".into()
        }
    }
}

struct Running {
    quit: Mutex<pw::channel::Sender<()>>,
    session: Option<ashpd::desktop::Session<Screencast>>,
    rt: tokio::runtime::Handle,
}

impl ScreenSource for Running {}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = crate::engine::lock(&self.quit).send(());
        if let Some(s) = self.session.take() {
            self.rt.spawn(async move {
                let _ = s.close().await;
            });
        }
    }
}

/// Returns the capture and the token that skips the picker next time.
pub async fn start(
    restore_token: Option<String>,
    fps: u32,
    sink: FrameSink,
    on_end: OnEnd,
) -> Result<(Box<dyn ScreenSource>, Option<String>), String> {
    let proxy = Screencast::new().await.map_err(failed)?;
    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(failed)?;
    let cursor = match proxy.available_cursor_modes().await {
        Ok(m) if m.contains(CursorMode::Metadata) => CursorMode::Metadata,
        _ => CursorMode::Embedded,
    };
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(cursor)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(restore_token.as_deref()),
        )
        .await
        .map_err(failed)?;
    let streams = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .map_err(failed)?
        .response()
        .map_err(failed)?;
    let node = streams
        .streams()
        .first()
        .ok_or("Nothing was picked to share.")?
        .pipe_wire_node_id();
    let token = streams.restore_token().map(str::to_owned);
    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(failed)?;

    let (quit, quit_rx) = pw::channel::channel();
    let (ready_tx, ready) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("irchad-screencast".into())
        .spawn(move || {
            let mut ready_tx = Some(ready_tx);
            if let Err(e) = run(fd, node, fps, sink, on_end, quit_rx, &mut ready_tx) {
                tracing::error!("screen capture: {e}");
                if let Some(tx) = ready_tx {
                    let _ = tx.send(false);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    if !ready.await.unwrap_or(false) {
        return Err("Couldn't start screen sharing.".into());
    }
    Ok((
        Box::new(Running {
            quit: Mutex::new(quit),
            session: Some(session),
            rt: tokio::runtime::Handle::current(),
        }),
        token,
    ))
}

/// The cursor as last seen in the metadata.
#[derive(Default)]
struct Cursor {
    image: Vec<u8>,
    w: u32,
    h: u32,
    /// The bitmap is B,G,R,A (else R,G,B,A).
    bgra: bool,
    x: i32,
    y: i32,
    visible: bool,
}

struct State {
    sink: FrameSink,
    on_end: Option<OnEnd>,
    format: Option<(PixelFormat, u32, u32)>,
    /// The last frame, packed, without the cursor.
    frame: Vec<u8>,
    size: (u32, u32),
    cursor: Cursor,
    under: Vec<u8>,
    streaming: bool,
}

fn run(
    fd: OwnedFd,
    node: u32,
    fps: u32,
    sink: FrameSink,
    on_end: OnEnd,
    quit: pw::channel::Receiver<()>,
    ready: &mut Option<tokio::sync::oneshot::Sender<bool>>,
) -> Result<(), pw::Error> {
    super::linux::init_pipewire();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(fd, None)?;
    let stream = pw::stream::StreamRc::new(
        core,
        "irchad-screen",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;
    let state = State {
        sink,
        on_end: Some(on_end),
        format: None,
        frame: Vec::new(),
        size: (0, 0),
        cursor: Cursor::default(),
        under: Vec::new(),
        streaming: false,
    };
    let _listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(|_, st, _, new| match new {
            pw::stream::StreamState::Streaming => st.streaming = true,
            pw::stream::StreamState::Error(e) => {
                tracing::warn!("screen capture stream: {e}");
                if let Some(f) = st.on_end.take() {
                    f();
                }
            }
            // The portal session went away (stopped from the desktop).
            pw::stream::StreamState::Unconnected if st.streaming => {
                if let Some(f) = st.on_end.take() {
                    f();
                }
            }
            _ => {}
        })
        .param_changed(|stream, st, id, param| {
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
                F::BGRx | F::BGRA => PixelFormat::Bgra,
                F::RGBx | F::RGBA => PixelFormat::Rgba,
                other => {
                    tracing::warn!("screen capture: unexpected format {other:?}");
                    return;
                }
            };
            let size = info.size();
            tracing::info!(
                "screen capture: {}x{} {:?}, modifier {:#x}",
                size.width,
                size.height,
                info.format(),
                info.modifier()
            );
            st.format = Some((format, size.width, size.height));
            if let Ok(params) = buffer_params() {
                let mut pods: Vec<&Pod> =
                    params.iter().filter_map(|b| Pod::from_bytes(b)).collect();
                let _ = stream.update_params(&mut pods);
            }
        })
        .process(process)
        .register()?;

    let formats = [format_pod(fps, true)?, format_pod(fps, false)?];
    let mut params: Vec<&Pod> = formats.iter().filter_map(|b| Pod::from_bytes(b)).collect();
    stream.connect(
        spa::utils::Direction::Input,
        Some(node),
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

fn serialize(obj: Object) -> Result<Vec<u8>, pw::Error> {
    Ok(spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(obj),
    )
    .map_err(|_| pw::Error::CreationFailed)?
    .0
    .into_inner())
}

/// Raw video in the formats we convert from; `dmabuf` adds the linear
/// modifier, which makes the producer offer DMA-BUF.
fn format_pod(fps: u32, dmabuf: bool) -> Result<Vec<u8>, pw::Error> {
    use spa::param::format::{FormatProperties as P, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat as F;
    let mut obj = spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(P::MediaType, Id, MediaType::Video),
        spa::pod::property!(P::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            P::VideoFormat,
            Choice,
            Enum,
            Id,
            F::BGRx,
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
                width: 1920,
                height: 1080
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
            Fraction { num: fps, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction {
                num: 1000,
                denom: 1
            }
        ),
        spa::pod::property!(
            P::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: fps, denom: 1 },
            Fraction { num: 1, denom: 1 },
            Fraction { num: fps, denom: 1 }
        ),
    );
    if dmabuf {
        // shortcut: linear only, read through mmap. Tiled modifiers need a GPU
        // import (EGL/Vulkan); see docs/notes/screenshare-perf.md.
        obj.properties.push(Property {
            key: P::VideoModifier.as_raw(),
            flags: PropertyFlags::MANDATORY,
            value: Value::Long(0),
        });
    }
    serialize(obj)
}

/// Buffer types we read, and the cursor and crop metadata.
fn buffer_params() -> Result<Vec<Vec<u8>>, pw::Error> {
    let types: i32 =
        (1 << ss::SPA_DATA_MemPtr) | (1 << ss::SPA_DATA_MemFd) | (1 << ss::SPA_DATA_DmaBuf);
    let buffers = Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![Property {
            key: ss::SPA_PARAM_BUFFERS_dataType,
            flags: PropertyFlags::empty(),
            value: Value::Int(types),
        }],
    };
    let meta = |kind: u32, size: usize| Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property {
                key: ss::SPA_PARAM_META_type,
                flags: PropertyFlags::empty(),
                value: Value::Id(spa::utils::Id(kind)),
            },
            Property {
                key: ss::SPA_PARAM_META_size,
                flags: PropertyFlags::empty(),
                value: Value::Int(size as i32),
            },
        ],
    };
    let cursor = size_of::<ss::spa_meta_cursor>()
        + size_of::<ss::spa_meta_bitmap>()
        + (CURSOR_MAX * CURSOR_MAX * 4) as usize;
    [
        buffers,
        meta(ss::SPA_META_Cursor, cursor),
        meta(ss::SPA_META_VideoCrop, size_of::<ss::spa_meta_region>()),
    ]
    .into_iter()
    .map(serialize)
    .collect()
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

/// # Safety
/// `buf` is a buffer the stream handed us and still owns.
unsafe fn read_buffer(buf: &ss::spa_buffer, st: &mut State) {
    let Some((format, fw, fh)) = st.format else {
        return;
    };
    let metas: &[ss::spa_meta] = if buf.metas.is_null() {
        &[]
    } else {
        // SAFETY: n_metas entries, per the stream.
        unsafe { std::slice::from_raw_parts(buf.metas, buf.n_metas as usize) }
    };
    let mut crop = None;
    for m in metas {
        if m.data.is_null() {
            continue;
        }
        // SAFETY: each meta's data is `size` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(m.data as *const u8, m.size as usize) };
        match m.type_ {
            ss::SPA_META_Cursor => read_cursor(bytes, &mut st.cursor),
            ss::SPA_META_VideoCrop if bytes.len() >= size_of::<ss::spa_meta_region>() => {
                // SAFETY: size checked; unaligned read of a plain C struct.
                let r: ss::spa_meta_region =
                    unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
                crop = Some(r.region);
            }
            _ => {}
        }
    }

    if buf.n_datas > 0 && !buf.datas.is_null() {
        // SAFETY: at least one data entry.
        let d = unsafe { &*buf.datas };
        let chunk = if d.chunk.is_null() {
            None
        } else {
            // SAFETY: non-null chunk of this data.
            Some(unsafe { *d.chunk })
        };
        if let Some(c) =
            chunk.filter(|c| c.size > 0 && c.flags & ss::SPA_CHUNK_FLAG_CORRUPTED as i32 == 0)
        {
            let stride = if c.stride > 0 {
                c.stride as u32
            } else {
                fw * 4
            };
            // SAFETY: the stream's own buffer.
            unsafe { copy_frame(d, &c, stride, fw, fh, crop, st) };
        }
    }
    if st.size.0 == 0 {
        return;
    }
    // Draw the cursor, hand the frame over, then put back what it covered,
    // so cursor-only updates can reuse the clean frame.
    let (w, h) = st.size;
    draw_cursor(
        &mut st.frame,
        w,
        h,
        format,
        &st.cursor,
        &mut st.under,
        false,
    );
    (st.sink)(Frame {
        data: &st.frame,
        width: w,
        height: h,
        stride: w * 4,
        format,
    });
    draw_cursor(&mut st.frame, w, h, format, &st.cursor, &mut st.under, true);
}

/// Copies the (cropped) picture into `st.frame`, packed.
///
/// # Safety
/// `d` and `c` belong to a buffer the stream still owns.
unsafe fn copy_frame(
    d: &ss::spa_data,
    c: &ss::spa_chunk,
    stride: u32,
    fw: u32,
    fh: u32,
    crop: Option<ss::spa_region>,
    st: &mut State,
) {
    let (mut x, mut y, mut w, mut h) = (0u32, 0u32, fw, fh);
    if let Some(r) = crop
        && r.size.width > 0
        && r.size.height > 0
    {
        let (rx, ry) = (r.position.x.max(0) as u32, r.position.y.max(0) as u32);
        if rx < fw && ry < fh {
            (x, y) = (rx, ry);
            w = r.size.width.min(fw - rx);
            h = r.size.height.min(fh - ry);
        }
    }
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM || stride < fw * 4 {
        return;
    }
    let need = c.offset as usize + stride as usize * fh as usize;
    let (src, mapped) = if !d.data.is_null() && need <= d.maxsize as usize {
        (d.data as *const u8, None)
    } else if d.type_ == ss::SPA_DATA_DmaBuf && d.fd >= 0 {
        // shortcut: maps per frame; cache maps per buffer if it shows in perf.
        let len = d.maxsize as usize + d.mapoffset as usize;
        if need > d.maxsize as usize {
            return;
        }
        // SAFETY: mapping a DMA-BUF the producer shared with us, read-only.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                d.fd as i32,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return;
        }
        sync(d.fd as i32, DMA_BUF_SYNC_START);
        // SAFETY: inside the mapping.
        (
            unsafe { (p as *const u8).add(d.mapoffset as usize) },
            Some((p, len)),
        )
    } else {
        return;
    };
    // SAFETY: `need` bytes from `src` are inside the buffer (checked above).
    let all =
        unsafe { std::slice::from_raw_parts(src.add(c.offset as usize), need - c.offset as usize) };
    st.frame.resize(w as usize * h as usize * 4, 0);
    for row in 0..h as usize {
        let from = (y as usize + row) * stride as usize + x as usize * 4;
        st.frame[row * w as usize * 4..(row + 1) * w as usize * 4]
            .copy_from_slice(&all[from..from + w as usize * 4]);
    }
    st.size = (w, h);
    if let Some((p, len)) = mapped {
        sync(d.fd as i32, DMA_BUF_SYNC_END);
        // SAFETY: our own mapping from above.
        unsafe { libc::munmap(p, len) };
    }
}

const DMA_BUF_SYNC_READ: u64 = 1;
const DMA_BUF_SYNC_START: u64 = 0;
const DMA_BUF_SYNC_END: u64 = 4;
/// `_IOW('b', 0, struct dma_buf_sync)`
const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;

/// Brackets CPU reads of a DMA-BUF so they see the GPU's writes.
fn sync(fd: i32, when: u64) {
    let flags: u64 = when | DMA_BUF_SYNC_READ;
    // SAFETY: the ioctl reads one u64 from the pointer.
    unsafe { libc::ioctl(fd, DMA_BUF_IOCTL_SYNC, &flags) };
}

/// Updates the cursor from a `spa_meta_cursor` (and its bitmap, if new).
fn read_cursor(bytes: &[u8], c: &mut Cursor) {
    if bytes.len() < size_of::<ss::spa_meta_cursor>() {
        return;
    }
    // SAFETY: size checked; plain C struct.
    let m: ss::spa_meta_cursor = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
    c.visible = m.id != 0;
    if !c.visible {
        return;
    }
    let off = m.bitmap_offset as usize;
    if off >= size_of::<ss::spa_meta_cursor>()
        && off + size_of::<ss::spa_meta_bitmap>() <= bytes.len()
    {
        // SAFETY: size checked; plain C struct.
        let b: ss::spa_meta_bitmap =
            unsafe { std::ptr::read_unaligned(bytes[off..].as_ptr().cast()) };
        let (w, h) = (b.size.width, b.size.height);
        let stride = b.stride.max(0) as usize;
        let start = off + b.offset as usize;
        use spa::param::video::VideoFormat as F;
        let bgra = match F::from_raw(b.format) {
            F::BGRA | F::BGRx => Some(true),
            F::RGBA | F::RGBx => Some(false),
            _ => None,
        };
        if let Some(bgra) = bgra
            && (1..=CURSOR_MAX).contains(&w)
            && (1..=CURSOR_MAX).contains(&h)
            && stride >= w as usize * 4
            && start + stride * h as usize <= bytes.len()
        {
            c.image.clear();
            for row in 0..h as usize {
                let r = start + row * stride;
                c.image.extend_from_slice(&bytes[r..r + w as usize * 4]);
            }
            (c.w, c.h, c.bgra) = (w, h, bgra);
        }
    }
    c.x = m.position.x.saturating_sub(m.hotspot.x);
    c.y = m.position.y.saturating_sub(m.hotspot.y);
}

/// Alpha-blends the cursor onto the frame, saving what it covers in `under`;
/// with `restore`, puts that back instead.
fn draw_cursor(
    frame: &mut [u8],
    fw: u32,
    fh: u32,
    format: PixelFormat,
    c: &Cursor,
    under: &mut Vec<u8>,
    restore: bool,
) {
    if !c.visible || c.image.is_empty() {
        return;
    }
    let (x0, y0) = (c.x.max(0) as u32, c.y.max(0) as u32);
    let x1 = c.x.saturating_add(c.w as i32).clamp(0, fw as i32) as u32;
    let y1 = c.y.saturating_add(c.h as i32).clamp(0, fh as i32) as u32;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let row_len = (x1 - x0) as usize * 4;
    if !restore {
        under.clear();
    }
    let swap = c.bgra != (format == PixelFormat::Bgra);
    for (i, y) in (y0..y1).enumerate() {
        let at = (y as usize * fw as usize + x0 as usize) * 4;
        let dst = &mut frame[at..at + row_len];
        if restore {
            if let Some(saved) = under.get(i * row_len..(i + 1) * row_len) {
                dst.copy_from_slice(saved);
            }
            continue;
        }
        under.extend_from_slice(dst);
        let cy = (y as i32 - c.y) as usize;
        let cx0 = (x0 as i32 - c.x) as usize;
        let src = &c.image[(cy * c.w as usize + cx0) * 4..][..row_len];
        for (d, s) in dst
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(src.as_chunks::<4>().0)
        {
            let a = u32::from(s[3]);
            let (s0, s2) = if swap { (s[2], s[0]) } else { (s[0], s[2]) };
            for (dc, sc) in d.iter_mut().zip([s0, s[1], s2]) {
                *dc = ((u32::from(sc) * a + u32::from(*dc) * (255 - a)) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_draws_and_restores() {
        let (fw, fh) = (8, 8);
        let mut frame = vec![10u8; fw * fh * 4];
        let clean = frame.clone();
        let c = Cursor {
            image: [200, 100, 50, 255].repeat(4 * 4),
            w: 4,
            h: 4,
            bgra: false,
            // Hangs off the bottom right corner.
            x: 6,
            y: 5,
            visible: true,
        };
        let mut under = Vec::new();
        draw_cursor(&mut frame, 8, 8, PixelFormat::Bgra, &c, &mut under, false);
        let px = (5 * fw + 6) * 4;
        // RGBA cursor onto a BGRA frame: swapped.
        assert_eq!(&frame[px..px + 3], &[50, 100, 200]);
        assert_eq!(under.len(), 2 * 3 * 4);
        draw_cursor(&mut frame, 8, 8, PixelFormat::Bgra, &c, &mut under, true);
        assert_eq!(frame, clean);
    }

    #[test]
    fn cursor_offscreen_is_ignored() {
        let mut frame = vec![0u8; 16 * 4];
        let c = Cursor {
            image: vec![255; 4 * 4],
            w: 2,
            h: 2,
            bgra: true,
            x: -5,
            y: 9,
            visible: true,
        };
        let mut under = Vec::new();
        draw_cursor(&mut frame, 4, 4, PixelFormat::Bgra, &c, &mut under, false);
        assert!(frame.iter().all(|&b| b == 0));
    }
}
