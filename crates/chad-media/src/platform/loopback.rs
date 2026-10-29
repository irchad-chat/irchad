//! Windows app audio for screen share: WASAPI process loopback (Windows 10
//! 2004+) captures one process tree's output, never ours. The app list comes
//! from the default output's audio sessions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0};
use windows::Win32::System::Com::{
    BLOB, CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Interface, PWSTR, Ref, implement};

use crate::screen::{AppSink, AudioApp};

/// Processes with an audio session on the default output, by name.
pub fn apps() -> Vec<AudioApp> {
    let me = std::process::id();
    // SAFETY: plain COM calls on this thread, which we initialize for them.
    let pids = unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let pids = sessions().unwrap_or_default();
        CoUninitialize();
        pids
    };
    let mut out: Vec<AudioApp> = Vec::new();
    for pid in pids {
        if pid == 0 || pid == me || out.iter().any(|a| a.id == pid.to_string()) {
            continue;
        }
        out.push(AudioApp {
            id: pid.to_string(),
            name: process_name(pid).unwrap_or_else(|| format!("Process {pid}")),
        });
    }
    out
}

unsafe fn sessions() -> windows::core::Result<Vec<u32>> {
    // SAFETY: COM is initialized by the caller.
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
        let list = manager.GetSessionEnumerator()?;
        let mut pids = Vec::new();
        for i in 0..list.GetCount()? {
            if let Ok(pid) = list
                .GetSession(i)
                .and_then(|s| s.cast::<IAudioSessionControl2>())
                .and_then(|s| s.GetProcessId())
            {
                pids.push(pid);
            }
        }
        Ok(pids)
    }
}

fn process_name(pid: u32) -> Option<String> {
    // SAFETY: a handle we open and close here; the buffer outlives the call.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let r =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Some(path.rsplit('\\').next().unwrap_or(&path).to_owned())
    }
}

/// A running capture; dropping it stops.
pub struct Capture(Arc<AtomicBool>);

impl Drop for Capture {
    fn drop(&mut self) {
        self.0.store(true, Relaxed);
    }
}

pub fn start(id: &str, sink: AppSink) -> Option<Capture> {
    let pid: u32 = id.parse().ok()?;
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    std::thread::Builder::new()
        .name("irchad-app-audio".into())
        .spawn(move || {
            // SAFETY: COM on this thread only.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                if let Err(e) = run(pid, sink, &st) {
                    tracing::warn!("app audio capture: {e}");
                }
                CoUninitialize();
            }
        })
        .ok()?;
    Some(Capture(stop))
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Done(std::sync::mpsc::SyncSender<()>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Done_Impl {
    fn ActivateCompleted(
        &self,
        _op: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.0.send(());
        Ok(())
    }
}

unsafe fn run(pid: u32, mut sink: AppSink, stop: &AtomicBool) -> windows::core::Result<()> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    // Borrows `params`: never cleared with PropVariantClear.
    let prop = std::mem::ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                Anonymous: windows::Win32::System::Com::StructuredStorage::PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: (&raw mut params).cast(),
                    },
                },
                ..Default::default()
            }),
        },
    });
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let handler: IActivateAudioInterfaceCompletionHandler = Done(tx).into();
    // SAFETY: `prop` and `params` outlive the activation, which we wait for.
    let client: IAudioClient = unsafe {
        let op = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*prop as *const PROPVARIANT),
            &handler,
        )?;
        let _ = rx.recv_timeout(Duration::from_secs(5));
        let mut hr = windows::core::HRESULT(0);
        let mut unknown = None;
        op.GetActivateResult(&mut hr, &mut unknown)?;
        hr.ok()?;
        unknown
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?
            .cast()?
    };
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 2,
        nSamplesPerSec: crate::RATE,
        nAvgBytesPerSec: crate::RATE * 4,
        nBlockAlign: 4,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let mut samples: Vec<i16> = Vec::new();
    // SAFETY: the client we just activated; buffers are released each round.
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            200_000,
            0,
            &format,
            None,
        )?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        while !stop.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            while capture.GetNextPacketSize()? > 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize * 2;
                samples.clear();
                if data.is_null() || flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    samples.resize(n, 0);
                } else {
                    let bytes = std::slice::from_raw_parts(data, n * 2);
                    samples.extend(
                        bytes
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|b| i16::from_le_bytes(*b)),
                    );
                }
                capture.ReleaseBuffer(frames)?;
                sink(&samples);
            }
        }
        let _ = client.Stop();
    }
    Ok(())
}
