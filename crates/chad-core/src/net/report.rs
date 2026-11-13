//! Reporting a message or person: one PRIVMSG to the account named by the home
//! channel's `chad/report`, tagged so the reporter's own clients never show it.

use super::*;

/// On our own report, so our devices and history skip it.
pub(super) const REPORT_TAG: &str = "+chad/report";
/// Under ergo's 512-byte line, with room for the tag and prefix.
const MAX_REPORT: usize = 380;
const MAX_QUOTE: usize = 200;

impl Net {
    /// `chad/report` on `target`: who reports go to, if it's the home channel.
    pub(super) fn report_metadata(&mut self, target: &str, value: Option<&str>) {
        if !self.home.as_ref().is_some_and(|h| self.cm.eq(h, target)) {
            return;
        }
        // Server input: it becomes a PRIVMSG target, so it must be a plain nick.
        self.report_to = value
            .map(str::trim)
            .filter(|a| valid_name(a) && !self.is_channel(a))
            .map(str::to_owned);
        let (network, account) = (self.id(), self.report_to.clone());
        self.events.push(Event::ReportTo { network, account });
    }

    pub(super) fn report(&mut self, r: crate::Report) {
        let Some(to) = self.report_to.clone() else {
            return self.error("This network doesn't take reports.");
        };
        if !self.connected {
            return self.error("Connect first to send a report.");
        }
        let text = report_text(&r);
        let m = Message::new("PRIVMSG", [to.as_str(), truncate(&text, MAX_REPORT)])
            .with_tag(REPORT_TAG, "1");
        let labeled = self.caps.enabled("labeled-response");
        self.send_labeled(m, Pending::Report);
        if !labeled {
            // Nothing will answer it: it went out, which is all we can say.
            self.events.push(Event::Reported { network: self.id() });
        }
    }

    pub(super) fn report_reply(&mut self, items: &[Item]) {
        match items.iter().find_map(failure) {
            Some(e) => self.error(format!("Couldn't send the report: {e}")),
            None => self.events.push(Event::Reported { network: self.id() }),
        }
    }

    /// Ours, tagged as a report: never shown on our side.
    pub(super) fn own_report(&self, m: &Message) -> bool {
        m.tag(REPORT_TAG).is_some()
            && m.source_nick()
                .is_some_and(|n| self.cm.eq(n, &self.info.nick))
    }

    /// TARGETS would open a DM with whoever takes reports, holding only our hidden reports.
    pub(super) fn is_report_to(&self, nick: &str) -> bool {
        self.report_to.as_ref().is_some_and(|r| self.cm.eq(r, nick))
    }
}

/// One line a moderator reads in any IRC client.
fn report_text(r: &crate::Report) -> String {
    let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = format!("[Report] {}", r.nick);
    if let Some(t) = r
        .target
        .as_deref()
        .filter(|t| !t.eq_ignore_ascii_case(&r.nick))
    {
        out += &format!(" in {t}");
    }
    if let Some(id) = &r.msgid {
        out += &format!(" (msgid {id})");
    }
    if let Some(text) = r.text.as_deref().map(flat).filter(|t| !t.is_empty()) {
        let quote = truncate(&text, MAX_QUOTE);
        let more = if quote.len() < text.len() { "…" } else { "" };
        out += &format!(": \"{quote}{more}\"");
    }
    let reason = flat(&r.reason);
    if !reason.is_empty() {
        out += &format!(" | Reason: {reason}");
    }
    out
}
