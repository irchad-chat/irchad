//! Voice: chad-core decides who may join and publishes `chad/voice`; the page
//! runs the call itself with `livekit-client`. This only fetches the token and
//! hears back how the call went, as `conn.rs` does for chad-media.

use chad_core::{NetCommand, chadd};
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use crate::client::{Client, run};
use crate::snapshot::to_js;

#[derive(Serialize)]
struct VoiceJoin {
    url: String,
    token: String,
}

#[wasm_bindgen]
impl Client {
    /// Join `channel`'s voice room, leaving any other. Resolves with where to
    /// connect, or `null` when there's nothing to connect (refused, already
    /// in it, or left meanwhile); why goes to the notices.
    #[wasm_bindgen(js_name = joinVoice, unchecked_return_type = "Promise<VoiceJoin | null>")]
    pub fn join_voice(&self, channel: String) -> js_sys::Promise {
        let start = run(&self.inner, |i, _| {
            i.command(NetCommand::JoinVoice { channel });
            let channel = i.net.voice_request.take()?;
            let (Some(client), Some(d)) = (i.chadd.clone(), &i.discovery) else {
                i.net.voice_ended(Some("You're offline right now.".into()));
                return None;
            };
            Some((channel, client, d.livekit.url.to_string()))
        });
        let Some((channel, client, url)) = start else {
            return js_sys::Promise::resolve(&JsValue::NULL);
        };
        let w = std::rc::Rc::downgrade(&self.inner);
        future_to_promise(async move {
            let r = client
                .call(|api, t| {
                    let channel = channel.clone();
                    async move { chadd::voice_token(&api, &t, &channel).await }
                })
                .await;
            let Some(rc) = w.upgrade() else {
                return Ok(JsValue::NULL);
            };
            let join = run(&rc, |i, _| {
                let wanted = i.net.voice.as_ref().is_some_and(|v| v.channel == channel);
                match r {
                    Ok(t) if wanted => Some(VoiceJoin {
                        url,
                        token: t.token,
                    }),
                    Err(e) if wanted => {
                        i.net.voice_ended(Some(e));
                        None
                    }
                    _ => None,
                }
            });
            Ok(to_js(&join))
        })
    }

    /// The page's LiveKit room connected: tell everyone we're in.
    #[wasm_bindgen(js_name = voiceConnected)]
    pub fn voice_connected(&self) {
        run(&self.inner, |i, _| i.net.voice_connected());
    }

    /// We left the call, or it dropped with `error`.
    #[wasm_bindgen(js_name = leaveVoice)]
    pub fn leave_voice(&self, error: Option<String>) {
        run(&self.inner, |i, _| i.net.voice_ended(error));
    }
}
