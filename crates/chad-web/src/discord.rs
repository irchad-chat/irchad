//! "Sign in with Discord" (D96) for the web client. The page goes to chadd's
//! `start?web=1`, Discord, and back to itself with `#oauth=<code>`. The PKCE
//! verifier waits in sessionStorage meanwhile: only this tab has it, so a
//! code from someone else's link is useless here.

use chad_core::chadd;
use chad_core::login::{self, Account};
use chad_core::oauth::Flow;
use chad_proto::chadd::OAuthOutcome;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use crate::client::{Client, W, device_id, run};

/// sessionStorage key for the verifier of the sign-in in progress.
const VERIFIER_KEY: &str = "irchad.oauth";

#[wasm_bindgen]
impl Client {
    /// Whether this network signs in with Discord.
    #[wasm_bindgen(js_name = discordAvailable, unchecked_return_type = "Promise<boolean>")]
    pub fn discord_available(&self) -> js_sys::Promise {
        let origin = self.inner.borrow().net.info.network.clone();
        future_to_promise(async move {
            let on = chad_core::fetch_discovery(&origin)
                .await
                .is_ok_and(|d| d.chadd.discord);
            Ok(on.into())
        })
    }

    /// The URL to send this tab to. It comes back with `#oauth=<code>` for
    /// `discordFinish`, or `#oauth_error=<message>`.
    #[wasm_bindgen(js_name = discordStart, unchecked_return_type = "Promise<string>")]
    pub fn discord_start(&self) -> js_sys::Promise {
        let origin = self.inner.borrow().net.info.network.clone();
        future_to_promise(async move {
            let d = chad_core::fetch_discovery(&origin).await.map_err(err)?;
            if !d.chadd.discord {
                return Err(err(format!("{} doesn't sign in with Discord.", d.name)));
            }
            let f = Flow::new(String::new(), origin, d.chadd.api);
            let storage =
                session_storage().ok_or_else(|| err("This browser won't keep the sign-in."))?;
            storage
                .set_item(VERIFIER_KEY, &f.verifier)
                .map_err(|_| err("This browser won't keep the sign-in."))?;
            Ok(format!("{}&web=1", f.start_url()).into())
        })
    }

    /// The code from `#oauth=`. Resolves with `null` once it's connecting, or
    /// with a suggested name to confirm through `discordName`.
    #[wasm_bindgen(js_name = discordFinish, unchecked_return_type = "Promise<string | null>")]
    pub fn discord_finish(&self, code: String) -> js_sys::Promise {
        let origin = self.inner.borrow().net.info.network.clone();
        let w = std::rc::Rc::downgrade(&self.inner);
        future_to_promise(async move {
            let expired = || err("That sign-in expired. Try again.");
            // One use: a reload doesn't try it again.
            let storage = session_storage().ok_or_else(expired)?;
            let verifier = storage
                .get_item(VERIFIER_KEY)
                .ok()
                .flatten()
                .ok_or_else(expired)?;
            let _ = storage.remove_item(VERIFIER_KEY);
            if !(1..=128).contains(&code.len()) {
                return Err(expired());
            }
            let d = chad_core::fetch_discovery(&origin).await.map_err(err)?;
            let outcome = chadd::oauth_exchange(&d.chadd.api, &code, &verifier)
                .await
                .map_err(|e| err(e.message()))?;
            match outcome {
                OAuthOutcome::SignedIn { account, refresh } => {
                    signed_in(&w, account, refresh)?;
                    Ok(JsValue::NULL)
                }
                OAuthOutcome::NeedsName { suggestion, ticket } => {
                    // Shown in an input (D14).
                    let suggestion: String = suggestion
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(64)
                        .collect();
                    if let Some(rc) = w.upgrade() {
                        rc.borrow_mut().ticket = Some((d.chadd.api, ticket));
                    }
                    Ok(suggestion.into())
                }
            }
        })
    }

    /// The account name for a first Discord sign-in. Rejects with why a name
    /// can't be had; then try another.
    #[wasm_bindgen(js_name = discordName, unchecked_return_type = "Promise<void>")]
    pub fn discord_name(&self, name: String) -> js_sys::Promise {
        let ticket = self.inner.borrow().ticket.clone();
        let w = std::rc::Rc::downgrade(&self.inner);
        future_to_promise(async move {
            let (api, ticket) = ticket.ok_or_else(|| err("That sign-in expired. Try again."))?;
            match chadd::oauth_register(&api, &ticket, name.trim()).await {
                Ok(OAuthOutcome::SignedIn { account, refresh }) => {
                    signed_in(&w, account, refresh)?;
                    Ok(JsValue::UNDEFINED)
                }
                Ok(OAuthOutcome::NeedsName { .. }) => Err(err("Something went wrong. Try again.")),
                Err(e) => Err(err(e.message())),
            }
        })
    }
}

/// Connect as the account chadd vouched for.
fn signed_in(w: &W, account: String, refresh: String) -> Result<(), JsValue> {
    // Bounded like everything else from the server (D14).
    if !login::valid_nick(&account) || account.len() > 64 || !(1..=4096).contains(&refresh.len()) {
        return Err(err("The server sent something odd. Try again later."));
    }
    if let Some(rc) = w.upgrade() {
        run(&rc, |i, w| {
            i.ticket = None;
            let acct = Account {
                name: account,
                password: refresh,
                device: device_id(),
                ..Default::default()
            };
            i.start(acct, true, w);
        });
    }
    Ok(())
}

fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

fn err(message: impl AsRef<str>) -> JsValue {
    JsError::new(message.as_ref()).into()
}
