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
            let max = max.ok_or("This server doesn't take uploads.")?;
            if bytes.len() as u64 > max {
                return Err(format!(
                    "That file is too big. The limit here is {} MB.",
                    max >> 20
                ));
            }
            let url = client
                .call(|api, t| {
                    let (name, bytes) = (name.clone(), bytes.clone());
                    async move { chadd::upload(&api, &t, &name, bytes).await }
                })
                .await?;
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
