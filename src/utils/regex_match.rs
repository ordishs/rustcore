pub fn is_regex_match(pattern: &str, text: &str) -> bool {
    match regex::Regex::new(pattern) {
        Ok(re) => re.is_match(text),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_regex_match;

    #[test]
    fn matches() {
        assert!(is_regex_match("foo.*", "foobar"));
        assert!(!is_regex_match("^bar", "foobar"));
        assert!(!is_regex_match("(", "anything")); // invalid pattern -> false
    }
}
