use std::time::Duration;

pub fn human_time_unit_with_colour(d: Duration) -> (String, &'static str) {
    let mut remaining = d.as_nanos() as f64;

    let days = (remaining / 1e9 / 86400.0) as i64;
    remaining -= days as f64 * 1e9 * 86400.0;

    let hours = (remaining / 1e9 / 3600.0) as i64;
    remaining -= hours as f64 * 1e9 * 3600.0;

    let minutes = (remaining / 1e9 / 60.0) as i64;
    remaining -= minutes as f64 * 1e9 * 60.0;

    let seconds = (remaining / 1e9) as i64;

    if days > 0 {
        (format!("{days}d{hours}h{minutes}m{seconds}s"), "red")
    } else if hours > 0 {
        (format!("{hours}h{minutes}m{seconds}s"), "red")
    } else if minutes > 0 {
        (format!("{minutes}m{seconds}s"), "orange")
    } else if remaining > 1e9 {
        (format!("{:.2}s", remaining / 1e9), "blue")
    } else if remaining > 1e6 {
        (format!("{:.2}ms", remaining / 1e6), "green")
    } else if remaining > 1e3 {
        (format!("{:.2}µs", remaining / 1e3), "black")
    } else {
        (format!("{}ns", remaining as i64), "grey")
    }
}

pub fn human_time_unit(d: Duration) -> String {
    human_time_unit_with_colour(d).0
}

pub fn human_time_unit_html(d: Duration) -> String {
    let (s, colour) = human_time_unit_with_colour(d);
    format!("<span style='color: {colour}'>{s}</span>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_magnitudes() {
        assert_eq!(human_time_unit(Duration::from_nanos(3)), "3ns");
        assert_eq!(human_time_unit(Duration::from_nanos(3_455)), "3.46µs");
        assert_eq!(human_time_unit(Duration::from_nanos(3_455_555)), "3.46ms");
        assert_eq!(
            human_time_unit(Duration::from_nanos(3_455_555_000)),
            "3.46s"
        );
        assert_eq!(human_time_unit(Duration::from_secs(65)), "1m5s");
        assert_eq!(human_time_unit(Duration::from_secs(3665)), "1h1m5s");
        assert_eq!(human_time_unit(Duration::from_secs(90065)), "1d1h1m5s");
    }

    #[test]
    fn html_wraps_with_colour() {
        assert_eq!(
            human_time_unit_html(Duration::from_secs(65)),
            "<span style='color: orange'>1m5s</span>"
        );
    }
}
