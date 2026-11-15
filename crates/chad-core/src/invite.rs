//! What the user pastes into "Join a network": an invite URL, an `irchad://` link
//! or a bare host. All of them name one origin (D5). `ircs://` and `irc://`
//! name a classic IRC network instead (D72).

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// `https://host[:port]`, no path, or `ircs://host:port` / `irc://host:port`
    /// for a classic network. Also the network's id.
    pub origin: String,
    /// `?channel=#general`: a hint to join, never authority (D5).
    pub channel: Option<String>,
}

pub fn parse(input: &str) -> Result<Invite, String> {
    let input = input.trim();
    let bad = || format!("“{input}” isn't a server URL");
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
        // `irchad://oauth` is a Discord sign-in coming back (D96), not an invite.
        "irchad" if url.host_str() == Some("join") => {
            let inner = query("origin").ok_or_else(bad)?;
            let inner = Url::parse(&inner).map_err(|_| bad())?;
            if inner.scheme() != "https" {
                return Err(bad());
            }
            inner
        }
        "https" => {
            // A meeting link (D86): `/meet/<code>` names its channel.
            if let Some(ch) = url
                .path()
                .strip_prefix("/meet/")
                .and_then(chad_proto::meeting::channel_of)
            {
                let mut origin = url.clone();
                origin.set_path("/");
                return Ok(Invite {
                    origin: origin.origin().ascii_serialization(),
                    channel: Some(ch),
                });
            }
            url
        }
        "ircs" | "irc" => {
            let host = url.host_str().filter(|h| !h.is_empty()).ok_or_else(bad)?;
            let tls = url.scheme() == "ircs";
            let port = url.port().unwrap_or(if tls { 6697 } else { 6667 });
            return Ok(Invite {
                origin: format!("{}://{}:{port}", url.scheme(), host.to_ascii_lowercase()),
                channel,
            });
        }
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

/// A network id naming a classic IRC network, not an IrChad server (D72).
pub fn classic(origin: &str) -> bool {
    !origin.starts_with("https://")
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
    fn meeting_links_name_their_channel() {
        let code = "0123456789abcdefghijklmnopqrstuv0123";
        assert_eq!(
            ok(&format!("https://chat.example.com/meet/{code}")),
            (
                "https://chat.example.com".into(),
                Some(format!("#meet-{code}"))
            )
        );
        assert_eq!(ok("https://chat.example.com/meet/nope").1, None);
    }

    #[test]
    fn classic_urls() {
        assert_eq!(
            ok("ircs://irc.Libera.Chat").0,
            "ircs://irc.libera.chat:6697"
        );
        assert_eq!(ok("irc://irc.efnet.org").0, "irc://irc.efnet.org:6667");
        assert_eq!(
            ok("ircs://irc.rizon.net:9999/").0,
            "ircs://irc.rizon.net:9999"
        );
        assert_eq!(
            ok("ircs://irc.libera.chat?channel=%23libera"),
            ("ircs://irc.libera.chat:6697".into(), Some("#libera".into()))
        );
        assert!(classic("ircs://irc.libera.chat:6697"));
        assert!(!classic("https://chat.example.com"));
        assert!(parse("ircs://").is_err());
    }

    #[test]
    fn rejects_other_schemes_and_junk() {
        for bad in [
            "",
            "http://chat.example.com",
            "irchad://join?origin=http://evil",
            "irchad://join",
            "irchad://oauth?origin=https://chat.example.com&code=x",
            "file:///etc/passwd",
            "has space.com",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(ok("x.com/invite?channel=nohash").1, None);
    }
}
