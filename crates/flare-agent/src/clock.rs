//! human_for: the duration words (Task 12; M6 adds the rest).

/// A duration in words for a sentence: rounded down, coarse on purpose, and "a few seconds" under ten.
pub fn human_for(ms: u64) -> String {
    let s = ms / 1000;
    if s < 10 {
        return "a few seconds".to_string();
    }
    if s < 60 {
        return plural(s, "second");
    }
    let m = s / 60;
    if m < 60 {
        return plural(m, "minute");
    }
    let h = m / 60;
    let rest = m % 60;
    if rest == 0 {
        plural(h, "hour")
    } else {
        format!("{} {}", plural(h, "hour"), plural(rest, "minute"))
    }
}

fn plural(n: u64, unit: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{n} {unit}{s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_for_a_few_seconds_under_ten() {
        assert_eq!(human_for(0), "a few seconds");
        assert_eq!(human_for(9_400), "a few seconds");
    }

    #[test]
    fn human_for_counts_seconds_minutes_hours() {
        assert_eq!(human_for(10_000), "10 seconds");
        assert_eq!(human_for(59_900), "59 seconds");
        assert_eq!(human_for(60_000), "1 minute");
        assert_eq!(human_for(125_000), "2 minutes");
        assert_eq!(human_for(3_600_000), "1 hour");
        assert_eq!(human_for(3_900_000), "1 hour 5 minutes");
        assert_eq!(human_for(7_200_000), "2 hours");
    }

    #[test]
    fn human_for_rounds_down() {
        assert_eq!(human_for(119_999), "1 minute");
    }
}
