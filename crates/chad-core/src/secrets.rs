//! Per-network credentials in the OS secret store. The only other thing a network
//! keeps locally is its origin (`ARCHITECTURE.md`).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

pub type Fut<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// Keyed by network origin. Errors are plain words for the UI.
pub trait SecretStore: Send + Sync {
    fn save<'a>(&'a self, origin: &'a str, account: &'a str, password: &'a str) -> Fut<'a, ()>;
    /// `(account, password)`.
    fn load<'a>(&'a self, origin: &'a str) -> Fut<'a, Option<(String, String)>>;
    fn delete<'a>(&'a self, origin: &'a str) -> Fut<'a, ()>;
}

/// Secret Service (portal-aware) on Linux; see `platform`.
pub use crate::platform::SystemSecrets;

/// Forgets everything on exit. For tests, and for running without a keyring.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, (String, String)>>);

impl SecretStore for MemorySecrets {
    fn save<'a>(&'a self, origin: &'a str, account: &'a str, password: &'a str) -> Fut<'a, ()> {
        let mut m = crate::lock(&self.0);
        m.insert(origin.into(), (account.into(), password.into()));
        Box::pin(async { Ok(()) })
    }

    fn load<'a>(&'a self, origin: &'a str) -> Fut<'a, Option<(String, String)>> {
        let found = crate::lock(&self.0).get(origin).cloned();
        Box::pin(async { Ok(found) })
    }

    fn delete<'a>(&'a self, origin: &'a str) -> Fut<'a, ()> {
        crate::lock(&self.0).remove(origin);
        Box::pin(async { Ok(()) })
    }
}
