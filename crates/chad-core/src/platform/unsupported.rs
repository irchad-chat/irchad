use crate::secrets::{Fut, SecretStore};

/// shortcut: no Windows Credential Manager yet, so nothing is saved and every
/// restart asks for the password again. Phase 1's Windows item replaces this.
#[derive(Default)]
pub struct SystemSecrets;

impl SecretStore for SystemSecrets {
    fn save<'a>(&'a self, _: &'a str, _: &'a str, _: &'a str) -> Fut<'a, ()> {
        Box::pin(async { Err("Saving passwords isn't supported on this system yet.".into()) })
    }

    fn load<'a>(&'a self, _: &'a str) -> Fut<'a, Option<(String, String)>> {
        Box::pin(async { Ok(None) })
    }

    fn delete<'a>(&'a self, _: &'a str) -> Fut<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
