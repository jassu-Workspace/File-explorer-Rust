const MONTH_NAMES: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Status line: "12 items · 3 selected · 4.2 MB" (size of the selected files, when any have one).
pub fn selection_summary(items: usize, selected: usize, selected_bytes: u64) -> String {
    let mut s = if items == 1 { "1 item".to_string() } else { format!("{items} items") };
    if selected > 0 {
        s.push_str(&format!(" · {selected} selected"));
        if selected_bytes > 0 {
            s.push_str(&format!(" · {}", format_size(selected_bytes)));
        }
    }
    s
}

/// Finder-like size groups: (rank, label).
pub fn size_group(is_dir: bool, bytes: u64) -> (i64, &'static str) {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        _ if is_dir => (0, "Folders"),
        0 => (1, "Zero"),
        b if b < 16 * KB => (2, "Tiny (< 16 KB)"),
        b if b < MB => (3, "Small (16 KB – 1 MB)"),
        b if b < 128 * MB => (4, "Medium (1 – 128 MB)"),
        b if b < 1024 * MB => (5, "Large (128 MB – 1 GB)"),
        _ => (6, "Huge (> 1 GB)"),
    }
}

/// Finder-like date groups, newest first: Today, Yesterday, Previous 7/30 Days, month names this year, then years.
pub fn date_group(t: LocalTime, today: LocalTime) -> (i64, String) {
    let days = day_number(today) - day_number(t);
    match days {
        ..=0 => (0, "Today".into()),
        1 => (1, "Yesterday".into()),
        2..=7 => (2, "Previous 7 Days".into()),
        8..=30 => (3, "Previous 30 Days".into()),
        _ if t.year == today.year => (4 + (today.month as i64 - t.month as i64), MONTH_NAMES[(t.month.clamp(1, 12) - 1) as usize].into()),
        _ => (100 + (today.year as i64 - t.year as i64), t.year.to_string()),
    }
}

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
    fn status_summary() {
        assert_eq!(selection_summary(12, 0, 0), "12 items");
        assert_eq!(selection_summary(1, 0, 0), "1 item");
        assert_eq!(selection_summary(12, 3, 4_404_019), "12 items · 3 selected · 4.2 MB");
        assert_eq!(selection_summary(12, 1, 0), "12 items · 1 selected");
    }

    #[test]
    fn size_groups() {
        assert_eq!(size_group(true, 0), (0, "Folders"));
        assert_eq!(size_group(false, 0), (1, "Zero"));
        assert_eq!(size_group(false, 10_000), (2, "Tiny (< 16 KB)"));
        assert_eq!(size_group(false, 500_000), (3, "Small (16 KB – 1 MB)"));
        assert_eq!(size_group(false, 50 << 20), (4, "Medium (1 – 128 MB)"));
        assert_eq!(size_group(false, 500 << 20), (5, "Large (128 MB – 1 GB)"));
        assert_eq!(size_group(false, 5 << 30), (6, "Huge (> 1 GB)"));
    }

    #[test]
    fn date_groups_like_finder() {
        let today = lt(2026, 11, 18, 12, 0);
        assert_eq!(date_group(lt(2026, 11, 18, 1, 0), today), (0, "Today".to_string()));
        assert_eq!(date_group(lt(2026, 11, 17, 1, 0), today), (1, "Yesterday".to_string()));
        assert_eq!(date_group(lt(2026, 11, 12, 1, 0), today), (2, "Previous 7 Days".to_string()));
        assert_eq!(date_group(lt(2026, 10, 25, 1, 0), today), (3, "Previous 30 Days".to_string()));
        assert_eq!(date_group(lt(2026, 9, 2, 1, 0), today), (6, "September".to_string()), "months this year, newest first");
        assert_eq!(date_group(lt(2024, 3, 2, 1, 0), today), (102, "2024".to_string()), "older years by year");
        assert!(date_group(lt(2026, 9, 2, 1, 0), today).0 < date_group(lt(2025, 12, 2, 1, 0), today).0);
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
