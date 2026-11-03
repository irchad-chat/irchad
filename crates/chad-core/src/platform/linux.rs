use crate::secrets::{Fut, SecretStore};

/// Secret Service over D-Bus, or the portal's file keyring inside Flatpak (`oo7`).
#[derive(Default)]
pub struct SystemSecrets;

const APP: (&str, &str) = ("application", "irchad");

/// A locked keyring (the usual state after login without PAM unlock) refuses
/// reads and writes; unlocking shows the desktop's own password prompt.
async fn open() -> Result<oo7::Keyring, String> {
    let keyring = oo7::Keyring::new().await.map_err(plain)?;
    tracing::debug!("secret store: unlocking");
    // The prompt can get lost or never show; don't hang the connect on it forever.
    tokio::time::timeout(std::time::Duration::from_secs(60), keyring.unlock())
        .await
        .map_err(|_| {
            tracing::warn!("secret store: unlock prompt not answered in 60s");
            "The system keyring is locked and didn't unlock.".to_owned()
        })?
        .map_err(plain)?;
    Ok(keyring)
}

fn plain(e: oo7::Error) -> String {
    tracing::warn!("secret store: {e}");
    "Couldn't use the system keyring. Is a Secret Service (GNOME Keyring, KWallet) running?".into()
}

impl SecretStore for SystemSecrets {
    fn save<'a>(&'a self, origin: &'a str, account: &'a str, password: &'a str) -> Fut<'a, ()> {
        Box::pin(async move {
            let keyring = open().await?;
            let attrs = [APP, ("origin", origin), ("account", account)];
            // One item per origin, whatever the account was before.
            keyring
                .delete(&[APP, ("origin", origin)])
                .await
                .map_err(plain)?;
            keyring
                .create_item(
                    &format!("IrChad: {account} on {origin}"),
                    &attrs,
                    password,
                    true,
                )
                .await
                .map_err(plain)
        })
    }

    fn load<'a>(&'a self, origin: &'a str) -> Fut<'a, Option<(String, String)>> {
        Box::pin(async move {
            let keyring = open().await?;
            let items = keyring
                .search_items(&[APP, ("origin", origin)])
                .await
                .map_err(plain)?;
            let Some(item) = items.first() else {
                return Ok(None);
            };
            let account = item.attributes().await.map_err(plain)?.remove("account");
            let secret = item.secret().await.map_err(plain)?;
            let password = String::from_utf8(secret.as_bytes().to_vec()).ok();
            Ok(account.zip(password))
        })
    }

    fn delete<'a>(&'a self, origin: &'a str) -> Fut<'a, ()> {
        Box::pin(async move {
            let keyring = open().await?;
            keyring
                .delete(&[APP, ("origin", origin)])
                .await
                .map_err(plain)
        })
    }
}
