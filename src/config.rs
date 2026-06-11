use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::stdlog;

#[derive(Debug)]
pub enum ConfigError {
    Parse {
        key: String,
        value: String,
        message: String,
    },
    InvalidDuration(String),
    InvalidUrl(String),
    Crypto(String),
    Io(std::io::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Parse {
                key,
                value,
                message,
            } => {
                write!(f, "failed to parse {value:?} for key {key:?}: {message}")
            }
            ConfigError::InvalidDuration(s) => write!(f, "invalid duration: {s}"),
            ConfigError::InvalidUrl(s) => write!(f, "invalid url: {s}"),
            ConfigError::Crypto(s) => write!(f, "crypto error: {s}"),
            ConfigError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Parse settings lines into the map (Go processFile parsing semantics).
#[allow(dead_code)]
fn parse_into(m: &mut HashMap<String, String>, path: &Path, content: &str) {
    for (line_num, raw) in content.split('\n').enumerate() {
        if raw.is_empty() {
            continue;
        }
        let line = raw.split('#').next().unwrap_or("");
        let Some(pos) = line.find('=') else { continue };
        let key = line[..pos].trim().to_string();
        let mut value = line[pos + 1..].trim().to_string();
        if value.len() > 2 && value.starts_with('"') && value.ends_with('"') {
            value = value[1..value.len() - 1].to_string();
        }
        if let Some(old) = m.get(&key) {
            stdlog(&format!(
                "INFO: {}:{} is replacing {:?}: {:?} -> {:?}",
                path.display(),
                line_num + 1,
                key,
                old,
                value
            ));
        }
        m.insert(key, value);
    }
}

/// Walk from start_dir upward to the root looking for filename; parse into m when found.
/// Ok(Some(path)) when found, Ok(None) when not found anywhere, Err on a read error
/// that is not NotFound (Go aborts in that case).
#[allow(dead_code)]
fn find_and_parse(
    start_dir: &Path,
    filename: &str,
    m: &mut HashMap<String, String>,
) -> Result<Option<PathBuf>, std::io::Error> {
    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join(filename);
        match std::fs::read_to_string(&candidate) {
            Ok(content) => {
                parse_into(m, &candidate, &content);
                return Ok(Some(candidate));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if !dir.pop() {
            return Ok(None);
        }
    }
}

/// Full Go discovery: executable dir upward, then cwd upward.
#[allow(dead_code)]
fn process_file(
    m: &mut HashMap<String, String>,
    filename: &str,
) -> Result<Option<PathBuf>, std::io::Error> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if let Some(found) = find_and_parse(exe_dir, filename, m)? {
                return Ok(Some(found));
            }
        }
    }
    let cwd = std::env::current_dir()?;
    find_and_parse(&cwd, filename, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines() {
        let mut m = std::collections::HashMap::new();
        parse_into(
            &mut m,
            std::path::Path::new("test.conf"),
            "key1=value1\nkey2 = value2 # comment\n# full comment\nquoted=\"hello world\"\nempty=\nnoequals\nkey1=replaced",
        );
        assert_eq!(m.get("key1").unwrap(), "replaced");
        assert_eq!(m.get("key2").unwrap(), "value2");
        assert_eq!(m.get("quoted").unwrap(), "hello world");
        assert_eq!(m.get("empty").unwrap(), "");
        assert!(!m.contains_key("noequals"));
        assert!(!m.contains_key("# full comment"));
    }

    #[test]
    fn quote_stripping_edge_cases() {
        let mut m = std::collections::HashMap::new();
        parse_into(&mut m, std::path::Path::new("t"), "a=\"x\"\nb=\"\"");
        assert_eq!(m.get("a").unwrap(), "x"); // len 3 > 2 -> stripped
        assert_eq!(m.get("b").unwrap(), "\"\""); // len 2, not stripped (Go parity)
    }

    #[test]
    fn discovers_file_walking_up() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a/b/c");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.path().join("settings.conf"), "k=v\n").unwrap();

        let mut m = std::collections::HashMap::new();
        let found = find_and_parse(&sub, "settings.conf", &mut m)
            .unwrap()
            .unwrap();
        assert_eq!(m.get("k").unwrap(), "v");
        assert!(found.ends_with("settings.conf"));
    }
}
