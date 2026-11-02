//! Web Push registration.

use super::*;

impl Net {
    pub(super) fn set_push(&mut self, sub: Option<PushSubscription>) {
        // D14 for the distributor's values too: they go out as IRC params.
        let sub = sub.filter(|s| {
            let ok = url::Url::parse(&s.endpoint).is_ok_and(|u| u.scheme() == "https")
                && s.endpoint.len() <= 1000
                && !s
                    .endpoint
                    .contains(|c: char| c.is_whitespace() || c.is_control())
                && is_base64url(&s.p256dh, 200)
                && is_base64url(&s.auth, 100);
            if !ok {
                tracing::warn!("{}: unusable push subscription", self.info.network);
            }
            ok
        });
        if sub == self.push && (self.push_registered || !self.connected) {
            return;
        }
        if sub != self.push {
            (self.push_tries, self.push_retry) = (0, None);
        }
        if let Some(old) = self
            .push
            .take()
            .filter(|_| self.connected && self.push_registered)
        {
            let same = sub.as_ref().is_some_and(|s| s.endpoint == old.endpoint);
            if !same {
                self.out.push(Message::new(
                    "WEBPUSH",
                    ["UNREGISTER", old.endpoint.as_str()],
                ));
            }
        }
        self.push_registered = false;
        if let Some(s) = &sub
            && self.connected
            && self.caps.enabled("draft/webpush")
        {
            let keys = format!("p256dh={};auth={}", s.p256dh, s.auth);
            self.out.push(Message::new(
                "WEBPUSH",
                ["REGISTER", s.endpoint.as_str(), keys.as_str()],
            ));
            self.push_registered = true;
        }
        self.push = sub;
    }

    /// When to send REGISTER again after ergo's test push failed.
    pub fn push_retry_at(&self) -> Option<Instant> {
        self.push_retry
    }

    pub fn retry_push(&mut self) {
        self.push_retry = None;
        self.set_push(self.push.clone());
    }
}
