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

use std::sync::atomic::AtomicU64;
use std::sync::{LazyLock, Mutex, RwLock};

type Listener = Box<dyn Fn(&str, &str) + Send + Sync>;

#[allow(dead_code)]
pub struct Configuration {
    confs: RwLock<HashMap<String, String>>,
    context: String,
    app: String,
    requests: Mutex<HashMap<String, String>>,
    listeners: RwLock<Vec<(u64, Listener)>>,
    next_listener_id: AtomicU64,
    pub(crate) settings_file: String,
    pub(crate) test_settings_file: String,
    pub(crate) local_settings_file: String,
}

impl Configuration {
    #[allow(dead_code)]
    pub(crate) fn new_with(confs: HashMap<String, String>, context: &str, app: &str) -> Self {
        Configuration {
            confs: RwLock::new(confs),
            context: context.to_string(),
            app: app.to_string(),
            requests: Mutex::new(HashMap::new()),
            listeners: RwLock::new(Vec::new()),
            next_listener_id: AtomicU64::new(1),
            settings_file: String::new(),
            test_settings_file: String::new(),
            local_settings_file: String::new(),
        }
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    /// Go findValue: walk key.<context> down to bare key, preferring .<app> at each step.
    fn find_value(&self, key: &str) -> Option<(String, String)> {
        let confs = self.confs.read().unwrap();
        let mut k = if self.context.is_empty() {
            key.to_string()
        } else {
            format!("{key}.{}", self.context)
        };
        loop {
            if !self.app.is_empty() {
                let ka = format!("{k}.{}", self.app);
                if let Some(v) = confs.get(&ka) {
                    return Some((v.clone(), ka));
                }
            }
            if let Some(v) = confs.get(&k) {
                return Some((v.clone(), k));
            }
            match k.rfind('.') {
                Some(pos) => k.truncate(pos),
                None => return None,
            }
        }
    }

    fn decrypt(&self, val: &str) -> String {
        crate::utils::secure_settings::decrypt_setting(val).unwrap_or_else(|_| val.to_string())
    }

    fn replace_variables(&self, value: &str) -> String {
        static RE: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(r"\$\{.*?\}").unwrap());
        let mut value = value.to_string();
        loop {
            let matches: Vec<String> = RE
                .find_iter(&value)
                .map(|m| m.as_str().to_string())
                .collect();
            if matches.is_empty() {
                break;
            }
            for m in matches {
                let key = &m[2..m.len() - 1];
                let replacement = self.get(key).unwrap_or_else(|| "{UNKNOWN}".to_string());
                value = value.replacen(&m, &replacement, 1);
            }
        }
        value
    }

    /// Go getInternal: env wins, then conf fallback walk, then default.
    /// Returns (value, found, key_used).
    fn get_internal(&self, key: &str, default: Option<&str>) -> (String, bool, String) {
        if let Ok(env) = std::env::var(key) {
            let v = self.replace_variables(&env);
            return (self.decrypt(&v), true, "ENV".to_string());
        }
        if let Some((ret, key_used)) = self.find_value(key) {
            let v = self.replace_variables(&ret);
            return (self.decrypt(&v), true, key_used);
        }
        let ret = default.unwrap_or("");
        let v = self.replace_variables(ret);
        (self.decrypt(&v), false, "DEFAULT".to_string())
    }

    fn record_request(&self, key: &str, val: &str) {
        self.requests
            .lock()
            .unwrap()
            .insert(key.to_string(), val.to_string());
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let (s, found, _) = self.get_internal(key, None);
        let val = s.strip_prefix("*EHE*").unwrap_or(&s).to_string();
        self.record_request(key, &val);
        if found {
            Some(val)
        } else {
            None
        }
    }

    pub fn get_or(&self, key: &str, default: &str) -> String {
        let (s, _, _) = self.get_internal(key, Some(default));
        let val = s.strip_prefix("*EHE*").unwrap_or(&s).to_string();
        self.record_request(key, &val);
        val
    }

    pub fn requested(&self) -> String {
        let requests = self.requests.lock().unwrap();
        let mut keys: Vec<&String> = requests.keys().collect();
        keys.sort();
        let mut out = String::new();
        for k in keys {
            out.push_str(&format!("{}={}\n", k, requests[k]));
        }
        out
    }
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

#[cfg(test)]
mod core_tests {
    use super::*;

    fn fixture(context: &str, app: &str) -> Configuration {
        let mut m = std::collections::HashMap::new();
        for (k, v) in [
            ("url", "http://localhost:8080"),
            ("url.live", "https://www.server.com"),
            ("url.live.uk", "https://www.server.co.uk"),
            ("name", "Simon"),
            ("city", "Paris"),
            ("city.dev.gocore", "New York"),
            ("embedded", "${name} lives in ${city}"),
            ("varcity", "The big ${city}"),
            ("embedded2", "${name} lives in ${varcity}"),
            ("missingvar", "hello ${nope}"),
            (
                "secret",
                "*EHE*8f7d64a1f1cefb44fe280d40bfe056ebd3aff457dd551ab8edf5d213cf9c",
            ),
        ] {
            m.insert(k.to_string(), v.to_string());
        }
        Configuration::new_with(m, context, app)
    }

    #[test]
    fn context_fallback() {
        assert_eq!(
            fixture("dev", "").get("url").unwrap(),
            "http://localhost:8080"
        );
        assert_eq!(
            fixture("live", "").get("url").unwrap(),
            "https://www.server.com"
        );
        assert_eq!(
            fixture("live.uk", "").get("url").unwrap(),
            "https://www.server.co.uk"
        );
        assert_eq!(
            fixture("live.es", "").get("url").unwrap(),
            "https://www.server.com"
        );
        assert_eq!(
            fixture("stage.eu.red", "").get("url").unwrap(),
            "http://localhost:8080"
        );
    }

    #[test]
    fn app_specific_keys_win() {
        assert_eq!(fixture("dev", "gocore").get("city").unwrap(), "New York");
        assert_eq!(fixture("dev", "").get("city").unwrap(), "Paris");
    }

    #[test]
    fn missing_key() {
        let c = fixture("dev", "");
        assert_eq!(c.get("nope"), None);
        assert_eq!(c.get_or("nope", "fallback"), "fallback");
    }

    #[test]
    fn variable_interpolation() {
        let c = fixture("dev", "");
        assert_eq!(c.get("embedded").unwrap(), "Simon lives in Paris");
        assert_eq!(c.get("embedded2").unwrap(), "Simon lives in The big Paris");
        assert_eq!(c.get("missingvar").unwrap(), "hello {UNKNOWN}");
    }

    #[test]
    fn ehe_value_is_decrypted_and_prefix_stripped() {
        assert_eq!(fixture("dev", "").get("secret").unwrap(), "42");
    }

    #[test]
    fn requested_records_lookups() {
        let c = fixture("dev", "");
        c.get("name");
        c.get("city");
        let r = c.requested();
        assert_eq!(r, "city=Paris\nname=Simon\n");
    }
}
