//! IRCv3 `server-time` (`2026-10-28T01:54:21.178Z`) ⇄ Unix milliseconds.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `YYYY-MM-DDThh:mm:ss[.fff]Z`. Anything else is `None`.
pub fn parse(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, mo, da) = (d.next()??, d.next()??, d.next()??);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.splitn(3, ':').map(|p| p.parse::<i64>().ok());
    let (h, mi, se) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    if !(1..=9999).contains(&y) || frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ms = format!("{frac:0<3}")[..3].parse::<i64>().ok()?;
    let secs = days_from_civil(y, mo, da) * 86_400 + h * 3600 + mi * 60 + se;
    Some(secs * 1000 + ms)
}

pub fn format(ms: i64) -> String {
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let (y, mo, d) = civil_from_days(days);
    let s = rem / 1000;
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        s / 3600,
        s / 60 % 60,
        s % 60,
        rem % 1000
    )
}

// Howard Hinnant's algorithms, http://howardhinnant.github.io/date_algorithms.html
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let t = "2026-10-28T01:54:21.178Z";
        assert_eq!(parse(t), Some(1_793_152_461_178));
        assert_eq!(format(parse(t).unwrap()), t);
        assert_eq!(parse("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(format(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            parse("2024-02-29T23:59:59.5Z").map(format).unwrap(),
            "2024-02-29T23:59:59.500Z"
        );
    }

    #[test]
    fn rejects_junk() {
        for bad in [
            "",
            "Z",
            "2026-13-01T00:00:00Z",
            "2026-10-28 01:54:21Z",
            "2026-10-28T01:54:21.Z",
            "x-1-1T1:1:1Z",
            "2026-10-28T01:54:21.1a2Z",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
