//! The date check of `smol-toml`: `new TomlDate(value).isValid()` (date.js). A text that is neither a number nor a keyword
//! is a date or time when `Date` reads what the constructor makes of it, so `isValid` is the shape of the text plus what
//! V8 accepts of the string handed to `Date`: the ISO form, with days up to 31 in every month and a time that may be
//! 24:00, and the one legacy form `YYYY-MM-DD Z`.

/// `DATE_TIME_RE`, matched whole: `^(\d{4}-\d{2}-\d{2})?[T ]?(?:(\d{2}):\d{2}(?::\d{2}(?:\.\d+)?)?)?(Z|[-+]\d{2}:\d{2})?$`
/// with the `i` flag. The captures: the date, the hour and the offset.
struct Shape<'a> {
    date: Option<&'a str>,
    hour: Option<&'a str>,
    offset: Option<&'a str>,
}

fn digits(bytes: &[u8], at: usize, n: usize) -> bool {
    bytes.len() >= at + n && bytes[at..at + n].iter().all(u8::is_ascii_digit)
}

fn date_at(bytes: &[u8], at: usize) -> bool {
    digits(bytes, at, 4)
        && bytes.get(at + 4) == Some(&b'-')
        && digits(bytes, at + 5, 2)
        && bytes.get(at + 7) == Some(&b'-')
        && digits(bytes, at + 8, 2)
}

/// The end of `\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?` starting at `at`, longest first (the order the pattern prefers).
fn time_ends(bytes: &[u8], at: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    if digits(bytes, at, 2) && bytes.get(at + 2) == Some(&b':') && digits(bytes, at + 3, 2) {
        let minutes_end = at + 5;
        if bytes.get(minutes_end) == Some(&b':') && digits(bytes, minutes_end + 1, 2) {
            let seconds_end = minutes_end + 3;
            if bytes.get(seconds_end) == Some(&b'.') && digits(bytes, seconds_end + 1, 1) {
                let mut end = seconds_end + 1;
                while digits(bytes, end, 1) {
                    end += 1;
                }
                // Fewer fraction digits leave a digit where the pattern needs the end or an offset.
                ends.push(end);
            }
            ends.push(seconds_end);
        }
        ends.push(minutes_end);
    }
    ends
}

fn offset_at(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at) {
        Some(b'Z' | b'z') => Some(at + 1),
        Some(b'+' | b'-')
            if digits(bytes, at + 1, 2)
                && bytes.get(at + 3) == Some(&b':')
                && digits(bytes, at + 4, 2) =>
        {
            Some(at + 6)
        }
        _ => None,
    }
}

fn shape(value: &str) -> Option<Shape<'_>> {
    let bytes = value.as_bytes();
    // Each optional group is tried present first, then absent, the order a backtracking matcher takes.
    let date_choices: Vec<usize> = if date_at(bytes, 0) {
        vec![10, 0]
    } else {
        vec![0]
    };
    for after_date in date_choices {
        let has_date = after_date == 10;
        let mut separators = Vec::new();
        if matches!(bytes.get(after_date), Some(b'T' | b't' | b' ')) {
            separators.push(after_date + 1);
        }
        separators.push(after_date);
        for after_separator in separators {
            let mut times: Vec<Option<usize>> = time_ends(bytes, after_separator)
                .into_iter()
                .map(Some)
                .collect();
            times.push(None);
            for time_end in times {
                let after_time = time_end.unwrap_or(after_separator);
                let mut offsets = Vec::new();
                if let Some(end) = offset_at(bytes, after_time) {
                    offsets.push(Some(end));
                }
                offsets.push(None);
                for offset_end in offsets {
                    let end = offset_end.unwrap_or(after_time);
                    if end == bytes.len() {
                        return Some(Shape {
                            date: has_date.then(|| &value[..10]),
                            hour: time_end.map(|_| &value[after_separator..after_separator + 2]),
                            offset: offset_end.map(|e| &value[after_time..e]),
                        });
                    }
                }
            }
        }
    }
    None
}

/// Whether V8's `new Date(text)` is a valid date for `text` as the constructor of `TomlDate` makes it.
fn date_parses(text: &str) -> bool {
    let bytes = text.as_bytes();
    if !date_at(bytes, 0) {
        return false;
    }
    let number = |at: usize, n: usize| -> u32 { text[at..at + n].parse().unwrap_or(0) };
    let (month, day) = (number(5, 2), number(8, 2));
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    let rest = &text[10..];
    if rest.is_empty() || rest == "Z" || rest == " Z" {
        return true;
    }
    let Some(time) = rest.strip_prefix('T') else {
        return false;
    };
    let tb = time.as_bytes();
    if !(digits(tb, 0, 2) && tb.get(2) == Some(&b':') && digits(tb, 3, 2)) {
        return false;
    }
    let (hour, minute) = (number(11, 2), number(14, 2));
    let mut at = 5;
    let mut second = 0;
    let mut fraction_zero = true;
    if tb.get(at) == Some(&b':') {
        if !digits(tb, at + 1, 2) {
            return false;
        }
        second = number(11 + at + 1, 2);
        at += 3;
        if tb.get(at) == Some(&b'.') {
            let start = at + 1;
            let mut end = start;
            while digits(tb, end, 1) {
                end += 1;
            }
            if end == start {
                return false;
            }
            fraction_zero = tb[start..end].iter().all(|b| *b == b'0');
            at = end;
        }
    }
    let offset = &time[at..];
    let offset_ok = match offset.as_bytes() {
        b"Z" => true,
        [b'+' | b'-', ..] if offset.len() == 6 => {
            digits(offset.as_bytes(), 1, 2)
                && offset.as_bytes()[3] == b':'
                && digits(offset.as_bytes(), 4, 2)
                && offset[1..3].parse::<u32>().is_ok_and(|h| h <= 23)
                && offset[4..6].parse::<u32>().is_ok_and(|m| m <= 59)
        }
        _ => false,
    };
    if !offset_ok || minute > 59 || second > 59 {
        return false;
    }
    hour < 24 || (hour == 24 && minute == 0 && second == 0 && fraction_zero)
}

/// `new TomlDate(value).isValid()`.
pub fn is_valid_toml_date(value: &str) -> bool {
    let Some(shape) = shape(value) else {
        return false;
    };
    let has_date = shape.date.is_some();
    let has_time = shape.hour.is_some();
    let mut date = if has_date {
        value.to_string()
    } else {
        format!("0000-01-01T{value}")
    };
    if has_time && date.as_bytes().get(10) == Some(&b' ') {
        date = date.replacen(' ', "T", 1);
    }
    if shape
        .hour
        .is_some_and(|h| h.parse::<u32>().is_ok_and(|h| h > 23))
    {
        return false;
    }
    date = date.to_uppercase();
    if shape.offset.is_none() && has_time {
        date.push('Z');
    }
    date_parses(&date) && (has_date || has_time)
}

#[cfg(test)]
mod tests {
    use super::is_valid_toml_date as valid;

    #[test]
    fn the_forms_toml_defines_are_dates() {
        for value in [
            "1979-05-27T07:32:00Z",
            "1979-05-27T00:32:00-07:00",
            "1979-05-27T00:32:00.999999-07:00",
            "1979-05-27 07:32:00Z",
            "1979-05-27T07:32:00",
            "1979-05-27",
            "07:32:00",
            "00:32:00.999999",
            "2023-02-31",
            "1979-05-27t07:32z",
        ] {
            assert!(valid(value), "{value}");
        }
    }

    #[test]
    fn what_date_refuses_is_not_a_date() {
        for value in [
            "2023-13-01",
            "2023-00-01",
            "2023-01-32",
            "24:00:01",
            "25:00",
            "07:60",
            "07:32:60",
            "1979-05-27T07:32:00+24:00",
            "1979-05-27+01:00",
            "T",
            "Z",
            "2020-01-01T",
            "abc",
        ] {
            assert!(!valid(value), "{value}");
        }
    }
}
