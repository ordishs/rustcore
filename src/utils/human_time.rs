use std::time::Duration;

pub fn human_time(d: Duration) -> String {
    let secs = d.as_secs();
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = ((secs % 86400) % 3600) / 60;
    let seconds = ((secs % 86400) % 3600) % 60;

    fn unit(n: u64, name: &str) -> String {
        if n > 1 {
            format!("{} {}s", n, name)
        } else {
            format!("{} {}", n, name)
        }
    }

    if days > 0 {
        return format!(
            "{} {} {} {}",
            unit(days, "day"),
            unit(hours, "hour"),
            unit(minutes, "minute"),
            unit(seconds, "second")
        );
    }
    if hours > 0 {
        return format!(
            "{} {} {}",
            unit(hours, "hour"),
            unit(minutes, "minute"),
            unit(seconds, "second")
        );
    }
    if minutes > 0 {
        return format!("{} {}", unit(minutes, "minute"), unit(seconds, "second"));
    }
    if seconds > 0 {
        return unit(seconds, "second");
    }
    "0 seconds".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_all_tiers() {
        assert_eq!(human_time(Duration::ZERO), "0 seconds");
        assert_eq!(human_time(Duration::from_secs(1)), "1 second");
        assert_eq!(human_time(Duration::from_secs(2)), "2 seconds");
        assert_eq!(human_time(Duration::from_secs(61)), "1 minute 1 second");
        assert_eq!(
            human_time(Duration::from_secs(3600)),
            "1 hour 0 minute 0 second"
        );
        assert_eq!(
            human_time(Duration::from_secs(90061)),
            "1 day 1 hour 1 minute 1 second"
        );
        assert_eq!(
            human_time(Duration::from_secs(2 * 86400 + 2 * 3600 + 120 + 2)),
            "2 days 2 hours 2 minutes 2 seconds"
        );
    }
}
