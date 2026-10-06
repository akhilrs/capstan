//! Just enough ISO 8601 to read the timestamps the daemon writes.

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn digits(text: &str, from: usize, count: usize) -> Option<i64> {
    let part = text.get(from..from + count)?;
    if part.bytes().all(|b| b.is_ascii_digit()) {
        part.parse().ok()
    } else {
        None
    }
}

/// Milliseconds since the Unix epoch for `YYYY-MM-DD[THH:MM[:SS[.fff]][Z|+HH:MM]]`; a time without an offset is UTC.
/// `None` when the text is not in that form.
pub fn parse_iso_ms(text: &str) -> Option<i64> {
    let year = digits(text, 0, 4)?;
    if text.get(4..5)? != "-" || text.get(7..8)? != "-" {
        return None;
    }
    let month = digits(text, 5, 2)?;
    let day = digits(text, 8, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut ms = days_from_civil(year, month, day) * 86_400_000;
    let rest = &text[10..];
    if rest.is_empty() {
        return Some(ms);
    }
    let rest = rest.strip_prefix('T')?;
    let hour = digits(rest, 0, 2)?;
    if rest.get(2..3)? != ":" {
        return None;
    }
    let minute = digits(rest, 3, 2)?;
    if hour > 24 || minute > 59 {
        return None;
    }
    ms += hour * 3_600_000 + minute * 60_000;
    let mut at = 5;
    if rest.get(at..at + 1) == Some(":") {
        let second = digits(rest, at + 1, 2)?;
        if second > 59 {
            return None;
        }
        ms += second * 1000;
        at += 3;
        if rest.get(at..at + 1) == Some(".") {
            let fraction: String = rest[at + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if fraction.is_empty() {
                return None;
            }
            let milli: String = format!("{fraction:0<3}").chars().take(3).collect();
            ms += milli.parse::<i64>().ok()?;
            at += 1 + fraction.len();
        }
    }
    let zone = &rest[at..];
    match zone {
        "" | "Z" => Some(ms),
        _ => {
            let sign = match zone.get(0..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let hours = digits(zone, 1, 2)?;
            let minutes = if zone.get(3..4) == Some(":") {
                digits(zone, 4, 2)?
            } else {
                digits(zone, 3, 2)?
            };
            Some(ms - sign * (hours * 60 + minutes) * 60_000)
        }
    }
}
