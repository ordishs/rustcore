pub fn split_args(s: &str) -> Result<Vec<String>, String> {
    if s.is_empty() {
        return Ok(vec![String::new()]);
    }

    let mut args = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    let mut in_quotes = false;
    let mut quoted_field = false;

    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' && cur.is_empty() && !quoted_field {
            in_quotes = true;
            quoted_field = true;
        } else if c == ' ' {
            args.push(std::mem::take(&mut cur));
            quoted_field = false;
        } else {
            cur.push(c);
        }
    }

    if in_quotes {
        return Err("unterminated quote".to_string());
    }
    args.push(cur);

    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::split_args;

    #[test]
    fn splits() {
        assert_eq!(split_args("").unwrap(), vec![""]);
        assert_eq!(
            split_args("config get key").unwrap(),
            vec!["config", "get", "key"]
        );
        assert_eq!(
            split_args("set key \"a value\"").unwrap(),
            vec!["set", "key", "a value"]
        );
        assert_eq!(split_args("a  b").unwrap(), vec!["a", "", "b"]);
    }
}
