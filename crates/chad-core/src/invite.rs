//! What the user pastes into "Join a network": an invite URL, an `irchad://` link
//! or a bare host. All of them name one origin (D5).

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// `https://host[:port]`, no path. Also the network's id.
    pub origin: String,
    /// `?channel=#general`: a hint to join, never authority (D5).
    pub channel: Option<String>,
}

pub fn parse(input: &str) -> Result<Invite, String> {
    let input = input.trim();
    let bad = || format!("“{input}” isn't an invite link or a server address");
    let url = if input.contains("://") {
        Url::parse(input).map_err(|_| bad())?
    } else {
        Url::parse(&format!("https://{input}")).map_err(|_| bad())?
    };
    let query = |k: &str| {
        url.query_pairs()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.into_owned())
    };
    let channel = query("channel").filter(|c| c.starts_with('#') && !c.contains([' ', ',']));
    let origin = match url.scheme() {
        "irchad" => {
            let inner = query("origin").ok_or_else(bad)?;
            let inner = Url::parse(&inner).map_err(|_| bad())?;
            if inner.scheme() != "https" {
                return Err(bad());
            }
            inner
        }
        "https" => url,
        _ => return Err(bad()),
    };
    if origin.host_str().is_none_or(str::is_empty) {
        return Err(bad());
    }
    Ok(Invite {
        origin: origin.origin().ascii_serialization(),
        channel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> (String, Option<String>) {
        let i = parse(s).unwrap();
        (i.origin, i.channel)
    }

    #[test]
    fn all_forms_name_the_origin() {
        let o = || "https://chat.example.com".to_owned();
        assert_eq!(ok("https://chat.example.com/invite"), (o(), None));
        assert_eq!(ok("https://chat.example.com/invite/abc123"), (o(), None));
        assert_eq!(ok("  chat.example.com "), (o(), None));
        assert_eq!(ok("CHAT.example.com/"), (o(), None));
        assert_eq!(
            ok("irchad://join?origin=https://chat.example.com&code=x&channel=%23general"),
            (o(), Some("#general".into()))
        );
        assert_eq!(
            ok("https://chat.example.com/invite?channel=%23dev"),
            (o(), Some("#dev".into()))
        );
        assert_eq!(
            ok("chat.localhost:8443"),
            ("https://chat.localhost:8443".into(), None)
        );
    }

    #[test]
    fn rejects_other_schemes_and_junk() {
        for bad in [
            "",
            "http://chat.example.com",
            "irchad://join?origin=http://evil",
            "irchad://join",
            "file:///etc/passwd",
            "has space.com",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(ok("x.com/invite?channel=nohash").1, None);
    }
}
