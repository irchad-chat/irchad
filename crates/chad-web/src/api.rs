//! chadd, through chad-core's client: uploads, link previews, proxied
//! images, GIFs, custom emoji and D69 tombstones. Each call the page makes
//! returns a Promise; failures reject with an `Error` whose message is for
//! the user.

use std::future::Future;
use std::rc::Rc;

use chad_core::{Event, NetCommand, chadd};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, spawn_local};

use crate::client::{Client, Inner, W, later, run};
use crate::snapshot::to_js;

#[wasm_bindgen]
impl Client {
    /// Upload a file through chadd, then post its link to `target`.
    #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
    pub fn upload(&self, target: String, name: String, bytes: Vec<u8>) -> js_sys::Promise {
        let max = self.inner.borrow().net.info.uploads;
        self.chadd(move |client, w| async move {
            let url = put(&client, max, name, bytes).await?;
            // Posted as a plain link, like the other apps: every client can open it.
            if let Some(rc) = w.upgrade() {
                run(&rc, |i, _| {
                    i.command(NetCommand::Send {
                        target,
                        text: url.into(),
                        reply_to: None,
                        mentions: Vec::new(),
                    });
                });
            }
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Upload a picture through chadd and make it ours.
    #[wasm_bindgen(js_name = uploadAvatar, unchecked_return_type = "Promise<void>")]
    pub fn upload_avatar(&self, name: String, bytes: Vec<u8>) -> js_sys::Promise {
        let max = self.inner.borrow().net.info.uploads;
        self.chadd(move |client, w| async move {
            let url = put(&client, max, name, bytes).await?;
            if let Some(rc) = w.upgrade() {
                run(&rc, |i, _| {
                    i.command(NetCommand::SetAvatar { url: url.into() })
                });
            }
            Ok(JsValue::UNDEFINED)
        })
    }

    /// A Discord account's password for other IRC clients (D96).
    #[wasm_bindgen(js_name = setPassword, unchecked_return_type = "Promise<void>")]
    pub fn set_password(&self, password: String) -> js_sys::Promise {
        self.chadd(move |client, _| async move {
            client
                .call(|api, t| {
                    let password = password.clone();
                    async move { chadd::set_password(&api, &t, &password).await }
                })
                .await?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Deletes our account, its messages and uploads (D69), then signs out
    /// for good. The page goes back to logging in once it resolves.
    #[wasm_bindgen(js_name = deleteAccount, unchecked_return_type = "Promise<void>")]
    pub fn delete_account(&self) -> js_sys::Promise {
        let (account, password, discord, guest, chadd_deletes) = {
            let i = self.inner.borrow();
            (
                i.acct.name.clone(),
                i.acct.password.clone(),
                i.discord,
                i.net.info.guest,
                i.discovery.as_ref().is_some_and(|d| d.chadd.delete),
            )
        };
        if guest {
            return js_sys::Promise::reject(
                &JsError::new("Guests have no account to delete.").into(),
            );
        }
        // shortcut: no NickServ UNREGISTER fallback as conn.rs has; add it if
        // a network without chadd's delete ever serves the web app.
        if !chadd_deletes {
            return js_sys::Promise::reject(
                &JsError::new("This network doesn't delete accounts from here. Ask NickServ.")
                    .into(),
            );
        }
        self.chadd(move |client, w| async move {
            if discord {
                // No password to ask for again (D96): the session vouches.
                client
                    .call(|api, t| {
                        let account = account.clone();
                        async move { chadd::delete_account(&api, &account, "", Some(&t)).await }
                    })
                    .await?;
            } else {
                chadd::delete_account(&client.api, &account, &password, None)
                    .await
                    .map_err(chadd::Error::message)?;
            }
            if let Some(rc) = w.upgrade() {
                run(&rc, |i, _| i.sign_out("account deleted"));
            }
            Ok(JsValue::UNDEFINED)
        })
    }

    /// What a link says about itself, or `null`. Its `image` goes through
    /// `media`, never straight to the page (D25).
    #[wasm_bindgen(unchecked_return_type = "Promise<LinkPreview | null>")]
    pub fn preview(&self, url: String) -> js_sys::Promise {
        self.chadd(move |client, _| async move {
            let p = client
                .call(|api, t| {
                    let url = url.clone();
                    async move { chadd::preview(&api, &t, &url).await }
                })
                .await
                .ok();
            Ok(to_js(&p))
        })
    }

    /// An image's bytes, fetched by chadd so the image's host never sees
    /// the reader (D25). The network's own URLs (uploads, emoji) the page
    /// loads itself.
    #[wasm_bindgen(unchecked_return_type = "Promise<Uint8Array>")]
    pub fn media(&self, url: String) -> js_sys::Promise {
        self.chadd(move |client, _| async move {
            let bytes = client
                .call(|api, t| {
                    let url = url.clone();
                    async move { chadd::media(&api, &t, &url).await }
                })
                .await?;
            Ok(js_sys::Uint8Array::from(&bytes[..]).into())
        })
    }

    /// A page of GIFs; empty `query` for what's trending, `pos` from the
    /// previous page's `next`.
    #[wasm_bindgen(unchecked_return_type = "Promise<GifPage>")]
    pub fn gifs(&self, query: String, pos: Option<String>) -> js_sys::Promise {
        self.chadd(move |client, _| async move {
            let page = client
                .call(|api, t| {
                    let (query, pos) = (query.clone(), pos.clone());
                    async move { chadd::gifs(&api, &t, query, pos).await }
                })
                .await?;
            Ok(to_js(&page))
        })
    }

    /// Start a meeting (D86) and join it. Resolves with its channel and the
    /// link to hand out; join its call once the channel shows up.
    #[wasm_bindgen(
        js_name = createMeeting,
        unchecked_return_type = "Promise<{ channel: string, url: string }>"
    )]
    pub fn create_meeting(&self) -> js_sys::Promise {
        self.chadd(move |client, w| async move {
            let r = client
                .call(|api, t| async move { chadd::create_meeting(&api, &t).await })
                .await?;
            if let Some(rc) = w.upgrade() {
                run(&rc, |i, _| {
                    i.command(NetCommand::JoinMeeting {
                        channel: r.channel.clone(),
                        new: true,
                    });
                });
            }
            Ok(to_js(&r))
        })
    }

    /// Ask for the custom emoji again, as when a picker opens.
    #[wasm_bindgen(js_name = refreshEmoji)]
    pub fn refresh_emoji(&self) {
        let rc = self.inner.clone();
        let i = rc.borrow();
        if let Some(client) = i.chadd.clone() {
            emoji(&i, &client, &Rc::downgrade(&rc));
        }
    }

    /// `f` with chadd's client, as a Promise; rejected at once when there's
    /// no connection.
    fn chadd<F, Fut>(&self, f: F) -> js_sys::Promise
    where
        F: FnOnce(Rc<chadd::Client>, W) -> Fut,
        Fut: Future<Output = Result<JsValue, String>> + 'static,
    {
        let client = self.inner.borrow().chadd.clone();
        let Some(client) = client else {
            return js_sys::Promise::reject(&JsError::new("You're offline right now.").into());
        };
        let fut = f(client, Rc::downgrade(&self.inner));
        future_to_promise(async move { fut.await.map_err(|e| JsError::new(&e).into()) })
    }
}

/// Uploads `bytes` if they fit under the network's limit.
async fn put(
    client: &chadd::Client,
    max: Option<u64>,
    name: String,
    bytes: Vec<u8>,
) -> Result<url::Url, String> {
    let max = max.ok_or("This server doesn't take uploads.")?;
    if bytes.len() as u64 > max {
        return Err(format!(
            "That file is too big. The limit here is {} MB.",
            max >> 20
        ));
    }
    client
        .call(|api, t| {
            let (name, bytes) = (name.clone(), bytes.clone());
            async move { chadd::upload(&api, &t, &name, bytes).await }
        })
        .await
}

/// What every connect fetches from chadd: custom emoji, and messages chadd
/// deleted with other accounts while we were away (D69).
pub(crate) fn on_connect(i: &Inner, client: &Rc<chadd::Client>, w: &W) {
    emoji(i, client, w);
    if i.discovery.as_ref().is_some_and(|d| d.chadd.delete) {
        let api = client.api.clone();
        let since = i.net.redacted_since();
        let w = w.clone();
        spawn_local(async move {
            if let Some((ids, next)) = chadd::redacted(&api, since).await {
                later(&w, move |i, _| i.net.chadd_redacted(&ids, next));
            }
        });
    }
}

fn emoji(i: &Inner, client: &Rc<chadd::Client>, w: &W) {
    if !i.net.info.emoji {
        return;
    }
    let (api, network, w) = (client.api.clone(), i.net.info.network.clone(), w.clone());
    spawn_local(async move {
        // On failure the set we have stays.
        if let Ok(list) = chadd::emoji(&api).await {
            let emoji = chad_core::emoji::custom(list, &network);
            later(&w, move |i, _| {
                i.view.apply(&Event::Emoji { network, emoji });
                i.dirty = true;
            });
        }
    });
}

/// The channel of a meeting link's code, or `undefined` if it isn't one.
#[wasm_bindgen(js_name = meetingChannel)]
pub fn meeting_channel(code: &str) -> Option<String> {
    chad_proto::meeting::channel_of(code)
}

/// Whether a channel is a meeting's (D86).
#[wasm_bindgen(js_name = isMeeting)]
pub fn is_meeting(channel: &str) -> bool {
    chad_proto::meeting::is_meeting(channel)
}

/// The first-visit cards, in order: show them once, before the sign-in form.
#[wasm_bindgen(js_name = welcomeCards, unchecked_return_type = "{ title: string; body: string }[]")]
pub fn welcome_cards() -> JsValue {
    #[derive(serde::Serialize)]
    struct Card {
        title: &'static str,
        body: &'static str,
    }
    let cards: Vec<Card> = chad_core::welcome::CARDS
        .iter()
        .map(|c| Card {
            title: c.title,
            body: c.body,
        })
        .collect();
    crate::snapshot::to_js(&cards)
}
