const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// 1024-based. KB without decimals; MB and above with one decimal.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    match bytes {
        1 => return "1 byte".into(),
        b if b < 1024 => return format!("{b} bytes"),
        _ => {}
    }
    let mut v = bytes as f64 / 1024.0;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{} KB", v.round() as u64) } else { format!("{v:.1} {}", UNITS[u]) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

/// Days since 1970-01-01 (Howard Hinnant's days_from_civil).
fn day_number(t: LocalTime) -> i64 {
    let (m, d) = (t.month as i64, t.day as i64);
    let y = t.year as i64 - (m <= 2) as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn format_modified(t: LocalTime, today: LocalTime) -> String {
    match day_number(today) - day_number(t) {
        0 => format!("Today, {:02}:{:02}", t.hour, t.minute),
        1 => format!("Yesterday, {:02}:{:02}", t.hour, t.minute),
        _ => format!("{} {}, {}", MONTHS[(t.month.clamp(1, 12) - 1) as usize], t.day, t.year),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lt(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> LocalTime {
        LocalTime { year, month, day, hour, minute }
    }

    #[test]
    fn size_strings() {
        for (b, s) in [(0, "0 bytes"), (1, "1 byte"), (900, "900 bytes"), (4096, "4 KB"), (1_258_291, "1.2 MB"), (5_368_709_120, "5.0 GB")] {
            assert_eq!(format_size(b), s, "{b}");
        }
    }

    #[test]
    fn date_today_yesterday_other() {
        let today = lt(2026, 11, 17, 12, 0);
        assert_eq!(format_modified(lt(2026, 11, 17, 9, 14), today), "Today, 09:14");
        assert_eq!(format_modified(lt(2026, 11, 16, 18, 2), today), "Yesterday, 18:02");
        assert_eq!(format_modified(lt(2026, 11, 2, 8, 0), today), "Nov 2, 2026");
        assert_eq!(format_modified(lt(2026, 12, 31, 23, 5), lt(2027, 1, 1, 0, 1)), "Yesterday, 23:05");
        assert_eq!(format_modified(lt(2024, 2, 29, 1, 0), lt(2024, 3, 1, 1, 0)), "Yesterday, 01:00");
    }
}
