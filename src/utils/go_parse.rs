/// Mirrors Go time.ParseDuration: "300ms", "1.5h", "2h45m", units ns/us/µs/μs/ms/s/m/h.
pub fn parse_go_duration(s: &str) -> Result<std::time::Duration, String> {
    let orig = s;
    let mut rest = s;
    let neg = rest.starts_with('-');
    if neg || rest.starts_with('+') {
        rest = &rest[1..];
    }
    if rest == "0" {
        return Ok(std::time::Duration::ZERO);
    }
    if rest.is_empty() {
        return Err(format!("invalid duration {orig:?}"));
    }
    if neg {
        return Err(format!("negative duration {orig:?} not supported"));
    }

    let mut total_nanos = 0f64;
    while !rest.is_empty() {
        let num_len = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .ok_or_else(|| format!("missing unit in duration {orig:?}"))?;
        if num_len == 0 {
            return Err(format!("invalid duration {orig:?}"));
        }
        let value: f64 = rest[..num_len]
            .parse()
            .map_err(|_| format!("invalid duration {orig:?}"))?;
        rest = &rest[num_len..];

        let (unit_nanos, unit_len) = if rest.starts_with("ns") {
            (1.0, 2)
        } else if rest.starts_with("us") {
            (1e3, 2)
        } else if rest.starts_with("µs") {
            (1e3, "µs".len())
        } else if rest.starts_with("μs") {
            (1e3, "μs".len())
        } else if rest.starts_with("ms") {
            (1e6, 2)
        } else if rest.starts_with('s') {
            (1e9, 1)
        } else if rest.starts_with('m') {
            (6e10, 1)
        } else if rest.starts_with('h') {
            (3.6e12, 1)
        } else {
            return Err(format!("unknown unit in duration {orig:?}"));
        };
        rest = &rest[unit_len..];
        total_nanos += value * unit_nanos;
    }

    Ok(std::time::Duration::from_nanos(total_nanos as u64))
}

/// Mirrors Go strconv.ParseBool.
pub fn parse_go_bool(s: &str) -> Result<bool, String> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!("invalid bool {s:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn durations() {
        assert_eq!(parse_go_duration("0").unwrap(), Duration::ZERO);
        assert_eq!(
            parse_go_duration("500ms").unwrap(),
            Duration::from_millis(500)
        );
        assert_eq!(parse_go_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(
            parse_go_duration("1h30m").unwrap(),
            Duration::from_secs(5400)
        );
        assert_eq!(
            parse_go_duration("1.5h").unwrap(),
            Duration::from_secs(5400)
        );
        assert_eq!(parse_go_duration("2µs").unwrap(), Duration::from_micros(2));
        assert!(parse_go_duration("5x").is_err());
        assert!(parse_go_duration("").is_err());
        assert!(parse_go_duration("m").is_err());
    }

    #[test]
    fn bools() {
        assert!(parse_go_bool("true").unwrap());
        assert!(parse_go_bool("1").unwrap());
        assert!(!parse_go_bool("F").unwrap());
        assert!(parse_go_bool("yes").is_err());
    }
}
