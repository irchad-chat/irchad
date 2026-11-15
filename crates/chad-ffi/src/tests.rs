use super::*;

#[test]
fn finds_links() {
    let net = "https://chat.example";
    let spans: Vec<Span> = format::parse(
        "see (https://a.example/x.PNG), https://chat.example/uploads/1/a.zip and \
         <https://b.example/page>. \x02https://c.example/v.mp4\x02 http:// ftp://x",
    )
    .into_iter()
    .map(Span::from)
    .collect();
    assert_eq!(
        Span::from(format::parse("\x0304,12x\x04ff8800y").remove(1)).fg,
        Some(0xff8800)
    );
    assert_eq!(
        Span::from(format::parse("\x0304,12x").remove(0)).bg,
        Some(0x0000fc)
    );
    let got = links(&spans, net);
    let k: Vec<_> = got.iter().map(|l| (l.url.as_str(), l.kind)).collect();
    assert_eq!(
        k,
        [
            ("https://a.example/x.PNG", LinkKind::Image),
            ("https://chat.example/uploads/1/a.zip", LinkKind::File),
            ("https://b.example/page", LinkKind::Page),
            ("https://c.example/v.mp4", LinkKind::Video),
        ]
    );
}

#[test]
fn reads_pushes() {
    let dm = "@time=2026-11-01T10:00:00.000Z;msgid=x;account=al :alice!a@h PRIVMSG bob :hi \x02there\x02";
    assert_eq!(
        read_push(dm.into()),
        Some(Push::Message {
            target: "alice".into(),
            from: "alice".into(),
            text: "hi there".into(),
            channel: false,
        })
    );
    let me = ":alice!a@h PRIVMSG #dev :\x01ACTION waves at bob\x01";
    assert_eq!(
        read_push(me.into()),
        Some(Push::Message {
            target: "#dev".into(),
            from: "alice".into(),
            text: "* alice waves at bob".into(),
            channel: true,
        })
    );
    let long = format!(":alice!a@h PRIVMSG bob :{}", "é".repeat(300));
    let Some(Push::Message { text, .. }) = read_push(long) else {
        panic!()
    };
    assert_eq!(text.chars().count(), 201);
    assert_eq!(
        read_push(":srv MARKREAD #dev timestamp=2026-11-01T10:00:00.000Z".into()),
        Some(Push::Read {
            target: "#dev".into()
        })
    );
    for junk in [
        "PING webpush",
        ":alice!a@h PRIVMSG bob :\x01VERSION\x01",
        ":alice!a@h PRIVMSG bob",
        ":srv 001 bob :hi",
        "",
    ] {
        assert_eq!(read_push(junk.into()), None, "{junk:?}");
    }
    assert_eq!(read_push("x".repeat(9000)), None);
}

#[test]
fn finds_emoji() {
    assert!(emojis("thumbsup".into()).contains(&"👍".to_owned()));
    assert!(emojis("".into()).len() > 1000);
    assert!(emojis("zzzzqqq".into()).is_empty());
}

#[test]
fn shortcodes_and_denoiser() {
    let r = replace_shortcodes("hi :smile: x".into(), 10, &[]).unwrap();
    assert_eq!((r.text.as_str(), r.cursor), ("hi 😄 x", 4));
    assert!(replace_shortcodes("12:30".into(), 0, &[]).is_none());

    let d = Denoiser::new();
    assert_eq!(d.process(vec![1, 2, 3]), vec![1, 2, 3]);
    assert_eq!(d.process(vec![0; 960]).len(), 960);
}

struct Nop;
impl Listener for Nop {
    fn changed(&self) {}
    fn notice(&self, _: String) {}
    fn uploaded(&self, _: String, _: String, _: String) {}
    fn join_failed(&self, _: String, _: String) {}
    fn preview(&self, _: String, _: String, _: Option<LinkPreview>) {}
    fn media(&self, _: String, _: String, _: Option<Vec<u8>>) {}
    fn gifs(&self, _: String, _: String, _: Option<String>, _: GifPage) {}
    fn push_available(&self, _: String, _: String) {}
    fn notify(&self, _: String, _: String, _: String, _: String, _: bool) {}
    fn verification(&self, _: String, _: String, _: Sas) {}
    fn sounds(&self, _: String, _: Vec<Clip>, _: Option<String>) {}
    fn sound(&self, _: String, _: String, _: Option<Vec<u8>>) {}
    fn reported(&self, _: String) {}
    fn open_url(&self, _: String) {}
    fn discord_needs_name(&self, _: String, _: String, _: Option<String>) {}
    fn password_set(&self, _: String, _: Option<String>) {}
}
impl Voice for Nop {
    fn join(&self, _: u64, _: String, _: String, _: Arc<Call>) {}
    fn hang_up(&self, _: u64) {}
}
impl Secrets for Nop {
    fn save(&self, _: String, _: String, _: String) -> Option<String> {
        None
    }
    fn load(&self, _: String) -> Option<Credentials> {
        None
    }
    fn delete(&self, _: String) {}
}

#[derive(Default)]
struct Log(Mutex<Vec<String>>);
impl Voice for Log {
    fn join(&self, id: u64, url: String, _: String, call: Arc<Call>) {
        call.connected();
        self.0.lock().unwrap().push(format!("join {id} {url}"));
    }
    fn hang_up(&self, id: u64) {
        self.0.lock().unwrap().push(format!("hang up {id}"));
    }
}

#[tokio::test]
async fn dropping_a_call_hangs_it_up() {
    use chad_core::Media;
    let log = Arc::new(Log::default());
    let calls = Calls {
        voice: log.clone(),
        next: AtomicU64::new(1),
    };
    let mut a = calls.join("wss://a".into(), "t".into());
    assert_eq!(a.recv().await, Some(MediaEvent::Connected));
    let b = calls.join("wss://b".into(), "t".into());
    drop(a);
    drop(b);
    for _ in 0..100 {
        if log.0.lock().unwrap().len() == 4 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let mut got = log.0.lock().unwrap().clone();
    got[2..].sort();
    assert_eq!(
        got,
        ["join 1 wss://a", "join 2 wss://b", "hang up 1", "hang up 2"]
    );
}

#[test]
fn starts_empty_and_stops() {
    let dir = std::env::temp_dir().join(format!("chad-ffi-{}", std::process::id()));
    let c = Client::new(
        dir.display().to_string(),
        "test".into(),
        Arc::new(Nop),
        Arc::new(Nop),
        Arc::new(Nop),
    );
    assert!(c.networks().is_empty());
    assert!(c.buffers("https://nope".into()).is_empty());
    assert!(c.member_list("https://nope".into(), "#a".into()).is_none());
    drop(c);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mention_spans() {
    let ms = [chad_core::Mention {
        nick: "bob".into(),
        account: "b".into(),
    }];
    // Bold text around a mention keeps its style; a link that contains the nick isn't split.
    let got = spans("\x02hi bob!\x02 https://x.example/bob", &ms, &[], |m| {
        format!("<{}>", m.account)
    });
    let k: Vec<_> = got
        .iter()
        .map(|s| (s.text.as_str(), s.bold, s.mention.as_deref()))
        .collect();
    assert_eq!(
        k,
        [
            ("hi ", true, None),
            ("bob", true, Some("<b>")),
            ("!", true, None),
            (" ", false, None),
            ("https://x.example/bob", false, None),
        ]
    );
}

#[test]
fn custom_emoji_spans() {
    let set = [chad_core::CustomEmoji {
        name: "bigint".into(),
        url: "https://chat.example/emoji/bigint.png".parse().unwrap(),
    }];
    // Not in code or links, and the text stays what plain clients see.
    let got = spans("\x02a :bigint:\x02 \x11:bigint:\x11", &[], &set, |_| {
        String::new()
    });
    let k: Vec<_> = got
        .iter()
        .map(|s| (s.text.as_str(), s.bold, s.emoji.as_deref()))
        .collect();
    assert_eq!(
        k,
        [
            ("a ", true, None),
            (
                ":bigint:",
                true,
                Some("https://chat.example/emoji/bigint.png")
            ),
            (" ", false, None),
            (":bigint:", false, None),
        ]
    );
}
