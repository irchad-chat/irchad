use crate::secrets::{Fut, SecretStore};

/// Secret Service over D-Bus, or the portal's file keyring inside Flatpak (`oo7`).
#[derive(Default)]
pub struct SystemSecrets;

const APP: (&str, &str) = ("application", "irchad");

fn plain(e: oo7::Error) -> String {
    tracing::warn!("secret store: {e}");
    "Couldn't use the system keyring. Is a Secret Service (GNOME Keyring, KWallet) running?".into()
}

impl SecretStore for SystemSecrets {
    fn save<'a>(&'a self, origin: &'a str, account: &'a str, password: &'a str) -> Fut<'a, ()> {
        Box::pin(async move {
            let keyring = oo7::Keyring::new().await.map_err(plain)?;
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
            let keyring = oo7::Keyring::new().await.map_err(plain)?;
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
            let keyring = oo7::Keyring::new().await.map_err(plain)?;
            keyring
                .delete(&[APP, ("origin", origin)])
                .await
                .map_err(plain)
        })
    }
}
