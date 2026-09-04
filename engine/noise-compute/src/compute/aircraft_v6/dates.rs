//! Date / time string helpers shared between airborne and cruise top-flight
//! emit paths. Airborne carries `date_id` (days since 2020-01-01) directly
//! from the arrow row; cruise only has `start_unix` from `flight_id::unpack`.

const DAYS_1970_TO_2020: i64 = 18_262;

/// Format `date_id` (days since 2020-01-01) as ISO `YYYY-MM-DD`.
pub fn date_from_id(date_id: i16) -> String {
    let mut rem = date_id as i32;
    let mut y = 2020i32;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
        let yd = if leap { 366 } else { 365 };
        if rem < yd {
            break;
        }
        rem -= yd;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
    let mdays: [i32; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0usize;
    while m < 12 && rem >= mdays[m] {
        rem -= mdays[m];
        m += 1;
    }
    format!("{:04}-{:02}-{:02}", y, m + 1, rem + 1)
}

pub fn date_from_unix(unix: u32) -> String {
    let days = (unix as i64 / 86_400) - DAYS_1970_TO_2020;
    if days < 0 || days > i16::MAX as i64 {
        return String::new();
    }
    date_from_id(days as i16)
}

pub fn time_from_unix(unix: u32) -> String {
    let secs_in_day = unix % 86_400;
    let h = secs_in_day / 3600;
    let m = (secs_in_day % 3600) / 60;
    let s = secs_in_day % 60;
    format!("{h:02}:{m:02}:{s:02}")
}
