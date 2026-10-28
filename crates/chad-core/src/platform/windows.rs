use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, FILETIME, GetLastError};
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};

use crate::secrets::{Fut, SecretStore};

/// Windows Credential Manager: one generic credential per origin, named
/// `irchad:<origin>`, with the account as its user name.
#[derive(Default)]
pub struct SystemSecrets;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn target(origin: &str) -> Vec<u16> {
    wide(&format!("irchad:{origin}"))
}

fn plain(what: &str) -> String {
    // SAFETY: reads the calling thread's last-error value.
    let code = unsafe { GetLastError() };
    tracing::warn!("credential manager: {what} failed ({code})");
    "Couldn't use Windows Credential Manager.".into()
}

impl SecretStore for SystemSecrets {
    fn save<'a>(&'a self, origin: &'a str, account: &'a str, password: &'a str) -> Fut<'a, ()> {
        let mut name = target(origin);
        let mut user = wide(account);
        let mut blob = password.as_bytes().to_vec();
        let cred = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: name.as_mut_ptr(),
            Comment: std::ptr::null_mut(),
            LastWritten: FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: user.as_mut_ptr(),
        };
        // SAFETY: every pointer in `cred` outlives the call.
        let ok = unsafe { CredWriteW(&cred, 0) } != 0;
        let r = if ok { Ok(()) } else { Err(plain("write")) };
        Box::pin(async move { r })
    }

    fn load<'a>(&'a self, origin: &'a str) -> Fut<'a, Option<(String, String)>> {
        let name = target(origin);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `name` is NUL-terminated; on success `cred` is freed below.
        let r = if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) } == 0 {
            // SAFETY: as in `plain`.
            if unsafe { GetLastError() } == ERROR_NOT_FOUND {
                Ok(None)
            } else {
                Err(plain("read"))
            }
        } else {
            // SAFETY: CredReadW returned a valid credential; its strings are
            // NUL-terminated and the blob is `CredentialBlobSize` bytes.
            let found = unsafe {
                let c = &*cred;
                let blob =
                    std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
                let mut len = 0;
                while !c.UserName.is_null() && *c.UserName.add(len) != 0 {
                    len += 1;
                }
                let user = std::slice::from_raw_parts(c.UserName, len);
                let found = String::from_utf16(user)
                    .ok()
                    .zip(String::from_utf8(blob.to_vec()).ok());
                CredFree(cred.cast());
                found
            };
            Ok(found)
        };
        Box::pin(async move { r })
    }

    fn delete<'a>(&'a self, origin: &'a str) -> Fut<'a, ()> {
        let name = target(origin);
        // SAFETY: `name` is NUL-terminated.
        let r = if unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0
            // SAFETY: as in `plain`.
            || unsafe { GetLastError() } == ERROR_NOT_FOUND
        {
            Ok(())
        } else {
            Err(plain("delete"))
        };
        Box::pin(async move { r })
    }
}
