//! OpenStreetMap's `opening_hours` in its common forms, as the hours of a week a place is open:
//! rules separated by `;`, each an optional weekday selector (`Mo-Fr`, `Sa,Su`) and time spans
//! (`12:00-15:00,19:00-02:00`, past midnight into the next day) or `off`; a later rule replaces an
//! earlier one on the days it names; `24/7`. Months, weeks, holidays (`PH`, `SH`), sunrise and
//! sunset and anything else make the value unreadable: the caller keeps its defaults.

/// Open hours of a week: `[weekday][hour]`, Monday first; an hour counts when the place is open at
/// its middle.
pub type WeekHours = [[bool; 24]; 7];

const DAYS: [&str; 7] = ["mo", "tu", "we", "th", "fr", "sa", "su"];

fn day_index(text: &str) -> Option<usize> {
    DAYS.iter().position(|day| *day == text)
}

/// The weekdays a selector names (`Mo-Fr,Su`); `None` when unreadable.
fn days(selector: &str) -> Option<[bool; 7]> {
    let mut named = [false; 7];
    for part in selector.split(',') {
        match part.split_once('-') {
            Some((from, to)) => {
                let (from, to) = (day_index(from)?, day_index(to)?);
                let mut day = from;
                loop {
                    named[day] = true;
                    if day == to {
                        break;
                    }
                    day = (day + 1) % 7;
                }
            }
            None => named[day_index(part)?] = true,
        }
    }
    Some(named)
}

/// Minutes after midnight of `HH:MM` (24:00 and later allowed).
fn minutes(text: &str) -> Option<u32> {
    let (hours, minutes) = text.split_once(':')?;
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours <= 48 && minutes < 60).then_some(hours * 60 + minutes)
}

/// The spans of a rule (`12:00-15:00,19:00-02:00`) as (start, end) minutes, the end past the start.
fn spans(text: &str) -> Option<Vec<(u32, u32)>> {
    text.split(',')
        .map(|span| {
            let (from, to) = span.split_once('-')?;
            let (from, mut to) = (minutes(from)?, minutes(to.trim_end_matches('+'))?);
            if to <= from {
                to += 24 * 60;
            }
            Some((from, to))
        })
        .collect()
}

/// The week's open hours of an `opening_hours` value; `None` when unreadable.
pub fn parse(value: &str) -> Option<WeekHours> {
    let value = value.trim().to_ascii_lowercase();
    if value == "24/7" {
        return Some([[true; 24]; 7]);
    }
    // Each day's spans (minutes from that day's midnight), replaced rule by rule.
    let mut by_day: [Vec<(u32, u32)>; 7] = Default::default();
    let mut any = false;
    for rule in value
        .split(';')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
    {
        let rule = rule.replace(", ", ",");
        let mut words = rule.split_whitespace();
        let first = words.next()?;
        let (named, times) = if first.as_bytes()[0].is_ascii_digit() {
            ([true; 7], first.to_string())
        } else {
            let named = days(first)?;
            let times: Vec<&str> = words.collect();
            (named, times.join(""))
        };
        let spans = match times.as_str() {
            "off" | "closed" => Vec::new(),
            times => spans(times)?,
        };
        for (day, chosen) in named.iter().enumerate() {
            if *chosen {
                by_day[day] = spans.clone();
            }
        }
        any = true;
    }
    if !any {
        return None;
    }
    let mut week = [[false; 24]; 7];
    for (day, spans) in by_day.iter().enumerate() {
        for &(from, to) in spans {
            for hour in 0..48u32 {
                let middle = hour * 60 + 30;
                if middle >= from && middle < to {
                    let (day, hour) = ((day + hour as usize / 24) % 7, hour as usize % 24);
                    week[day][hour] = true;
                }
            }
        }
    }
    Some(week)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(week: &WeekHours, day: usize) -> Vec<usize> {
        (0..24).filter(|&hour| week[day][hour]).collect()
    }

    #[test]
    fn common_values_read_as_their_hours() {
        let bar = parse("Mo-Th 18:00-02:00; Fr-Sa 18:00-03:00; Su off").unwrap();
        assert_eq!(open(&bar, 0), [18, 19, 20, 21, 22, 23]);
        assert_eq!(
            open(&bar, 1),
            [0, 1, 18, 19, 20, 21, 22, 23],
            "Monday's night runs on"
        );
        assert_eq!(open(&bar, 5), [0, 1, 2, 18, 19, 20, 21, 22, 23]);
        assert_eq!(
            open(&bar, 6),
            [0, 1, 2],
            "Saturday's night into a closed Sunday"
        );
        let restaurant = parse("Tu-Su 12:00-15:00, 19:00-23:00").unwrap();
        assert!(open(&restaurant, 0).is_empty());
        assert_eq!(open(&restaurant, 2), [12, 13, 14, 19, 20, 21, 22]);
        assert_eq!(parse("24/7").unwrap(), [[true; 24]; 7]);
        let shop = parse("09:00-18:00").unwrap();
        assert_eq!(open(&shop, 6), (9..18).collect::<Vec<_>>());
        let replaced = parse("Mo-Su 10:00-22:00; Su 12:00-20:00").unwrap();
        assert_eq!(open(&replaced, 6), (12..20).collect::<Vec<_>>());
    }

    #[test]
    fn unreadable_values_keep_the_defaults() {
        for value in [
            "",
            "Oct-Apr: Mo-Su 10:00-18:00",
            "sunrise-sunset",
            "Mo-Fr 9-17",
            "PH off",
        ] {
            assert!(parse(value).is_none(), "{value}");
        }
    }
}
