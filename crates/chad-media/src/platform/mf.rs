//! Windows cameras: Media Foundation's source reader, which decodes MJPG and
//! converts to NV12 for us (advanced video processing). Frames come back on
//! the thread that reads them.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};

use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::core::{GUID, Interface, PWSTR};

use super::desktop::Running;
use crate::Device;
use crate::screen::{FrameSink, OnEnd, ScreenSource, nv12};

const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

/// Runs `f` with COM up on this thread, in whatever mode it already has.
fn com<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: balanced below; a thread already in another mode fails and
    // keeps it, which is fine for MF.
    let ours = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let r = f();
    if ours {
        // SAFETY: pairs with the successful CoInitializeEx.
        unsafe { CoUninitialize() };
    }
    r
}

fn attributes() -> windows::core::Result<IMFAttributes> {
    let mut a = None;
    // SAFETY: an out pointer.
    unsafe { MFCreateAttributes(&mut a, 1)? };
    a.ok_or_else(|| E_FAIL.into())
}

/// Video capture devices, in the system's order.
fn devices() -> windows::core::Result<Vec<IMFActivate>> {
    // Once per process, never shut down: it's refcounted, and we'd only
    // start it again for the next enumeration.
    static STARTED: OnceLock<windows::core::Result<()>> = OnceLock::new();
    // SAFETY: MFStartup takes no pointers.
    STARTED
        .get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) })
        .clone()?;
    // SAFETY: plain MF calls; the array is ours to free once emptied.
    unsafe {
        let attrs = attributes()?;
        attrs.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
        let (mut list, mut n) = (std::ptr::null_mut(), 0u32);
        MFEnumDeviceSources(&attrs, &mut list, &mut n)?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        let out = (0..n as usize)
            .filter_map(|i| (*list.add(i)).take())
            .collect();
        CoTaskMemFree(Some(list as *const _));
        Ok(out)
    }
}

fn string(a: &IMFActivate, key: &GUID) -> Option<String> {
    let (mut p, mut len) = (PWSTR::null(), 0);
    // SAFETY: MF allocates the string; we free it.
    unsafe {
        a.GetAllocatedString(key, &mut p, &mut len).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Cameras by symbolic link, for the settings.
pub fn cameras() -> Vec<Device> {
    com(|| {
        devices()
            .unwrap_or_default()
            .iter()
            .filter_map(|a| {
                Some(Device {
                    id: string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK)?,
                    name: string(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)?,
                })
            })
            .collect()
    })
}

/// Opens camera `id`; `None`, or one that's gone, takes the first.
pub async fn start(
    id: Option<String>,
    mut sink: FrameSink,
    on_end: OnEnd,
) -> Result<Box<dyn ScreenSource>, String> {
    let stop = Arc::new(AtomicBool::new(false));
    // Activation can take seconds (a USB camera waking up): awaited, not blocked on.
    let (ready_tx, ready) = tokio::sync::oneshot::channel();
    let st = stop.clone();
    std::thread::Builder::new()
        .name("irchad-camera".into())
        .spawn(move || {
            com(|| {
                let reader = match open(id.as_deref()) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                if let Err(e) = read(&reader, &mut sink, &st) {
                    tracing::warn!("camera: {e}");
                }
                if !st.load(Relaxed) {
                    on_end();
                }
            });
        })
        .map_err(|e| e.to_string())?;
    match ready.await {
        Ok(Ok(())) => Ok(Box::new(Running(stop))),
        Ok(Err(e)) => {
            tracing::warn!("opening the camera: {e}");
            Err("Couldn't open the camera.".into())
        }
        Err(_) => Err("Couldn't open the camera.".into()),
    }
}

fn open(id: Option<&str>) -> windows::core::Result<IMFSourceReader> {
    let devices = devices()?;
    let link = |a: &&IMFActivate| {
        string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).as_deref() == id
    };
    let a = devices
        .iter()
        .find(link)
        .or(devices.first())
        .ok_or_else(|| windows::core::Error::from(E_FAIL))?;
    // SAFETY: plain MF calls on objects we hold. Releasing the reader shuts
    // the source down.
    unsafe {
        let source: IMFMediaSource = a.ActivateObject()?;
        let attrs = attributes()?;
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs)?;
        if let Some(t) = best_mode(&reader)
            && let Err(e) = reader.SetCurrentMediaType(STREAM, None, &t)
        {
            tracing::debug!("camera mode: {e}");
        }
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        reader.SetCurrentMediaType(STREAM, None, &out)?;
        Ok(reader)
    }
}

/// (width, height) or (numerator, denominator), packed high:low.
fn pair(t: &IMFMediaType, key: &GUID) -> (u32, u32) {
    // SAFETY: reading an attribute.
    let v = unsafe { t.GetUINT64(key) }.unwrap_or(0);
    ((v >> 32) as u32, v as u32)
}

/// The camera's own mode nearest 720p at 25 fps or more; the reader decodes
/// whatever it is.
fn best_mode(reader: &IMFSourceReader) -> Option<IMFMediaType> {
    // SAFETY: enumerating until the reader says there are no more.
    (0..256)
        .map_while(|i| unsafe { reader.GetNativeMediaType(STREAM, i) }.ok())
        .max_by_key(|t| {
            let (w, h) = pair(t, &MF_MT_FRAME_SIZE);
            let (n, d) = pair(t, &MF_MT_FRAME_RATE);
            let area = i64::from(w) * i64::from(h);
            let fps = n / d.max(1);
            let fits = area <= 1280 * 720;
            (fps >= 25, fits, if fits { area } else { -area }, fps)
        })
}

fn read(
    reader: &IMFSourceReader,
    sink: &mut FrameSink,
    stop: &AtomicBool,
) -> windows::core::Result<()> {
    // SAFETY: plain MF calls; each locked buffer is unlocked before the next.
    unsafe {
        let mut size = pair(&reader.GetCurrentMediaType(STREAM)?, &MF_MT_FRAME_SIZE);
        let mut joined = Vec::new();
        tracing::info!("camera: {}x{} NV12", size.0, size.1);
        while !stop.load(Relaxed) {
            let (mut flags, mut sample) = (0u32, None);
            reader.ReadSample(STREAM, 0, None, Some(&mut flags), None, Some(&mut sample))?;
            let flags = flags as i32;
            if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 | MF_SOURCE_READERF_ERROR.0) != 0 {
                return Ok(());
            }
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 != 0 {
                size = pair(&reader.GetCurrentMediaType(STREAM)?, &MF_MT_FRAME_SIZE);
            }
            let Some(sample) = sample else { continue };
            let buf = sample.ConvertToContiguousBuffer()?;
            let (w, h) = size;
            // 2D buffers know their pitch and length; otherwise rows are packed.
            if let Ok(b2) = buf.cast::<IMF2DBuffer2>() {
                let (mut p, mut pitch) = (std::ptr::null_mut(), 0i32);
                let (mut start, mut len) = (std::ptr::null_mut(), 0u32);
                b2.Lock2DSize(
                    MF2DBuffer_LockFlags_Read,
                    &mut p,
                    &mut pitch,
                    &mut start,
                    &mut len,
                )?;
                let skip = (p as usize).wrapping_sub(start as usize);
                if !p.is_null() && pitch > 0 && skip <= len as usize {
                    let data = std::slice::from_raw_parts(p, len as usize - skip);
                    nv12(data, w, h, pitch as u32, &mut joined, sink);
                }
                b2.Unlock2D()?;
            } else {
                let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
                buf.Lock(&mut p, None, Some(&mut len))?;
                if !p.is_null() {
                    let data = std::slice::from_raw_parts(p, len as usize);
                    nv12(data, w, h, w, &mut joined, sink);
                }
                buf.Unlock()?;
            }
        }
        Ok(())
    }
}
