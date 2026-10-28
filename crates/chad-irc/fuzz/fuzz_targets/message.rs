#![no_main]

use chad_irc::Message;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    if let Ok(m) = Message::parse(s) {
        let line = m.to_line().expect("parsed message must serialize");
        assert_eq!(Message::parse(&line).unwrap(), m);
    }
});
