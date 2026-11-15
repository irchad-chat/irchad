//! "Sign in with Discord" (D96), the app's half: PKCE, the browser link, and
//! the `irchad://oauth` redirect that comes back. chadd does the talking to
//! Discord; [`crate::chadd::oauth_exchange`] and friends do the talking to chadd.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use chad_proto::chadd;
use sha2::{Digest, Sha256};
use url::Url;

/// A sign-in waiting on the browser, and then maybe on a name. Only one at a
/// time: a redirect we didn't ask for, or for another server, is dropped, so
/// a link from someone else can't sign us into their account.
pub struct Flow {
    /// What the user joined with, for [`crate::Event::JoinFailed`].
    pub invite: String,
    /// `https://host[:port]`, what the redirect's `origin` must say.
    pub origin: String,
    pub api: Url,
    pub verifier: String,
    /// Set once chadd asks for a name.
    pub ticket: Option<String>,
}

impl Flow {
    pub fn new(invite: String, origin: String, api: Url) -> Self {
        let mut b = [0u8; 32];
        getrandom::fill(&mut b).expect("the OS has randomness");
        Self {
            invite,
            origin,
            api,
            verifier: B64URL.encode(b),
            ticket: None,
        }
    }

    /// The page to open in the browser.
    pub fn start_url(&self) -> String {
        let challenge = B64URL.encode(Sha256::digest(self.verifier.as_bytes()));
        let mut url = Url::parse(&chadd::endpoint(&self.api, chadd::OAUTH_DISCORD_START))
            .expect("the API URL is a base");
        url.query_pairs_mut().append_pair("challenge", &challenge);
        url.into()
    }
}

/// What came back through `irchad://oauth?origin=…&code=…` (or `&error=…`).
#[derive(Debug, PartialEq, Eq)]
pub struct Callback {
    pub origin: String,
    pub result: Result<String, String>,
}

/// Whether a URL the OS handed us is a Discord sign-in coming back.
pub fn is_callback(url: &str) -> bool {
    url.trim().starts_with("irchad://oauth")
}

pub fn parse_callback(url: &str) -> Option<Callback> {
    let url = Url::parse(url.trim()).ok()?;
    if url.scheme() != "irchad" || url.host_str() != Some("oauth") {
        return None;
    }
    let query = |k: &str| {
        url.query_pairs()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.into_owned())
    };
    let origin = query("origin")?;
    // chadd's codes are 43 characters; its errors are a sentence (D14).
    let result = match (query("code"), query("error")) {
        (Some(c), _) if (1..=128).contains(&c.len()) => Ok(c),
        (_, Some(e)) => Err(e.chars().filter(|c| !c.is_control()).take(200).collect()),
        _ => return None,
    };
    Some(Callback { origin, result })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_url_carries_the_s256_of_the_verifier() {
        let mut f = Flow::new(
            "chat.example.com".into(),
            "https://chat.example.com".into(),
            Url::parse("https://chat.example.com/api").unwrap(),
        );
        assert_eq!(f.verifier.len(), 43);
        // Cross-checked with Python: urlsafe_b64encode(sha256(v)).rstrip('=').
        f.verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWjOEjXk".into();
        assert_eq!(
            f.start_url(),
            "https://chat.example.com/api/oauth/discord/start?challenge=VYFANLqdx_HDV6BqEhluZJ63rtrIPROSSFdB3P6G83I"
        );
    }

    #[test]
    fn callbacks() {
        let ok = parse_callback("irchad://oauth?origin=https%3A%2F%2Fchat.example.com&code=abc")
            .unwrap();
        assert_eq!(ok.origin, "https://chat.example.com");
        assert_eq!(ok.result, Ok("abc".into()));
        let err =
            parse_callback("irchad://oauth?origin=https://x&error=Discord+said+no%0A").unwrap();
        assert_eq!(err.result, Err("Discord said no".into()));
        assert!(is_callback(" irchad://oauth?x"));
        for bad in [
            "irchad://join?origin=https://x&code=a",
            "irchad://oauth?code=a",
            "irchad://oauth?origin=https://x",
            "https://oauth?origin=https://x&code=a",
            &format!("irchad://oauth?origin=https://x&code={}", "a".repeat(200)),
        ] {
            assert_eq!(parse_callback(bad), None, "{bad}");
        }
    }
}
