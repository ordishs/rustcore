use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, RwLock};

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

type Listener = Box<dyn Fn(&str, &str) + Send + Sync>;

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

    pub fn get_parsed<T: std::str::FromStr>(&self, key: &str) -> Result<Option<T>, ConfigError>
    where
        T::Err: fmt::Display,
    {
        match self.get(key) {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(None),
            Some(s) => s.parse::<T>().map(Some).map_err(|e| ConfigError::Parse {
                key: key.to_string(),
                value: s,
                message: e.to_string(),
            }),
        }
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            None => default,
            Some(s) if s.is_empty() => default,
            Some(s) => crate::utils::parse_go_bool(&s).unwrap_or(false),
        }
    }

    pub fn get_duration(&self, key: &str) -> Result<Option<std::time::Duration>, ConfigError> {
        match self.get(key) {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(None),
            Some(s) => crate::utils::parse_go_duration(&s)
                .map(Some)
                .map_err(ConfigError::InvalidDuration),
        }
    }

    pub fn get_multi(&self, key: &str, sep: &str) -> Option<Vec<String>> {
        match self.get(key) {
            None => None,
            Some(s) if s.is_empty() => None,
            Some(s) => Some(s.split(sep).map(|i| i.trim().to_string()).collect()),
        }
    }

    pub fn get_url(&self, key: &str) -> Result<Option<url::Url>, ConfigError> {
        let Some(mut s) = self.get(key) else {
            return Ok(None);
        };
        if s.is_empty() {
            return Ok(None);
        }
        static RE: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(r"\*EHE\*[a-zA-Z0-9]+").unwrap());
        let tokens: Vec<String> = RE.find_iter(&s).map(|m| m.as_str().to_string()).collect();
        for token in tokens {
            if let Ok(decrypted) = crate::utils::secure_settings::decrypt_setting(&token) {
                let plain = decrypted
                    .strip_prefix("*EHE*")
                    .unwrap_or(&decrypted)
                    .to_string();
                s = s.replacen(&token, &plain, 1);
            }
        }
        url::Url::parse(&s)
            .map(Some)
            .map_err(|e| ConfigError::InvalidUrl(format!("{s}: {e}")))
    }

    pub fn set(&self, key: &str, value: &str) -> Option<String> {
        let old = {
            let mut confs = self.confs.write().unwrap();
            confs.insert(key.to_string(), value.to_string())
        };
        for (_, l) in self.listeners.read().unwrap().iter() {
            l(key, value);
        }
        old
    }

    pub fn unset(&self, key: &str) -> Option<String> {
        let old = {
            let mut confs = self.confs.write().unwrap();
            confs.remove(key)
        };
        for (_, l) in self.listeners.read().unwrap().iter() {
            l(key, "");
        }
        old
    }

    pub fn add_listener(&self, f: impl Fn(&str, &str) + Send + Sync + 'static) -> u64 {
        let id = self.next_listener_id.fetch_add(1, Ordering::SeqCst);
        self.listeners.write().unwrap().push((id, Box::new(f)));
        id
    }

    pub fn remove_listener(&self, id: u64) {
        self.listeners.write().unwrap().retain(|(i, _)| *i != id);
    }

    pub fn get_all(&self) -> HashMap<String, String> {
        let confs = self.confs.read().unwrap();
        let mut m = HashMap::new();
        m.insert("_SETTINGS_CONTEXT".to_string(), self.context.clone());
        for (k, v) in confs.iter() {
            m.insert(k.clone(), std::env::var(k).unwrap_or_else(|_| v.clone()));
        }
        m
    }

    pub fn stats(&self) -> String {
        static MASK_RE: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(r"\*EHE\*[a-zA-Z0-9]+").unwrap());

        let mut out = String::from("\nCMDLINE\n-------\n");
        for (i, arg) in std::env::args().enumerate() {
            out.push_str(&format!("{i:2}: {arg}\n"));
        }

        out.push_str("\nSETTINGS_ENV\n------------\nContext:     ");
        if self.context != "dev" {
            out.push_str(&self.context);
        } else {
            out.push_str("Not set (dev)");
        }
        out.push_str("\nApplication: ");
        if !self.app.is_empty() {
            out.push_str(&self.app);
        } else {
            out.push_str("Not set");
        }
        out.push_str("\n\nSETTINGS\n--------\n");

        let mut base_keys: Vec<String> = {
            let confs = self.confs.read().unwrap();
            let set: std::collections::HashSet<String> = confs
                .keys()
                .map(|k| k.split('.').next().unwrap_or(k).to_string())
                .collect();
            set.into_iter().collect()
        };
        base_keys.sort();

        for k in base_keys {
            let (v, _, key_used) = self.get_internal(&k, None);
            let v = MASK_RE.replace_all(&v, "********************");
            let context = key_used.replacen(&k, "", 1);
            if !context.is_empty() {
                out.push_str(&format!("{k}[{context}]={v}\n"));
            } else {
                out.push_str(&format!("{k}={v}\n"));
            }
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

#[cfg(test)]
mod typed_tests {
    use super::*;
    use std::time::Duration;

    fn cfg() -> Configuration {
        let mut m = std::collections::HashMap::new();
        for (k, v) in [
            ("number", "5042"),
            ("float", "3.14"),
            ("flag", "true"),
            ("badflag", "notabool"),
            ("badnum", "xyz"),
            ("timeout", "1h30m"),
            ("multi", "simon, peter, paul"),
            ("dburl", "postgres://user:pass@localhost:5432/db"),
        ] {
            m.insert(k.to_string(), v.to_string());
        }
        Configuration::new_with(m, "dev", "")
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn numbers() {
        let c = cfg();
        assert_eq!(c.get_parsed::<i32>("number").unwrap(), Some(5042));
        assert_eq!(c.get_parsed::<u64>("number").unwrap(), Some(5042));
        assert_eq!(c.get_parsed::<f64>("float").unwrap(), Some(3.14));
        assert_eq!(c.get_parsed::<i32>("missing").unwrap(), None);
        assert!(c.get_parsed::<i32>("badnum").is_err());
        assert!(c.get_parsed::<u8>("number").is_err()); // 5042 overflows u8
    }

    #[test]
    fn bools() {
        let c = cfg();
        assert!(c.get_bool("flag", false));
        assert!(!c.get_bool("badflag", true)); // unparseable -> false (Go parity)
        assert!(c.get_bool("missing", true)); // default
        assert!(!c.get_bool("missing", false));
    }

    #[test]
    fn durations() {
        let c = cfg();
        assert_eq!(
            c.get_duration("timeout").unwrap(),
            Some(Duration::from_secs(5400))
        );
        assert_eq!(c.get_duration("missing").unwrap(), None);
    }

    #[test]
    fn multi() {
        let c = cfg();
        assert_eq!(
            c.get_multi("multi", ",").unwrap(),
            vec!["simon", "peter", "paul"]
        );
        assert_eq!(c.get_multi("missing", ","), None);
    }

    #[test]
    fn urls() {
        let c = cfg();
        let u = c.get_url("dburl").unwrap().unwrap();
        assert_eq!(u.scheme(), "postgres");
        assert_eq!(u.host_str(), Some("localhost"));
        assert_eq!(c.get_url("missing").unwrap(), None);
    }
}

static GLOBAL: LazyLock<Configuration> = LazyLock::new(init_global);
static ALT_CONFIGS: LazyLock<Mutex<HashMap<String, &'static Configuration>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn init_global() -> Configuration {
    let env_file = std::env::var("SETTINGS_ENV_FILE").unwrap_or_else(|_| ".env".to_string());
    if std::path::Path::new(&env_file).exists() && dotenvy::from_filename(&env_file).is_err() {
        stdlog("WARN: failed to loading .env file");
    }

    let context = std::env::var("SETTINGS_CONTEXT").unwrap_or_else(|_| "dev".to_string());
    let app = std::env::var("SETTINGS_APPLICATION").unwrap_or_default();

    let mut confs = HashMap::new();

    let settings_file = match process_file(&mut confs, "settings.conf") {
        Ok(Some(p)) => p.display().to_string(),
        Ok(None) => {
            stdlog("WARN: No config file 'settings.conf'");
            "NOT FOUND".to_string()
        }
        Err(e) => {
            stdlog(&format!(
                "FATAL: Failed to read config file 'settings.conf' - [{e}]"
            ));
            std::process::exit(1);
        }
    };

    let test_settings_file = match process_file(&mut confs, "settings_test.conf") {
        Ok(Some(p)) => {
            let p = p.display().to_string();
            stdlog(&format!("INFO: Loaded test config file '{p}'"));
            p
        }
        _ => "NOT FOUND".to_string(),
    };

    let local_settings_file = match process_file(&mut confs, "settings_local.conf") {
        Ok(Some(p)) => p.display().to_string(),
        Ok(None) => {
            stdlog("WARN: No local config file 'settings_local.conf'");
            "NOT FOUND".to_string()
        }
        Err(e) => {
            stdlog(&format!("FATAL: Failed to read local config - [{e}]"));
            std::process::exit(1);
        }
    };

    let mut c = Configuration::new_with(confs, &context, &app);
    c.settings_file = settings_file;
    c.test_settings_file = test_settings_file;
    c.local_settings_file = local_settings_file;

    start_advertising(&c);

    c
}

pub fn config() -> &'static Configuration {
    &GLOBAL
}

pub fn config_for_context(ctx: &str) -> &'static Configuration {
    let global = config();
    if ctx.is_empty() || ctx == global.context {
        return global;
    }
    let mut map = ALT_CONFIGS.lock().unwrap();
    if let Some(c) = map.get(ctx) {
        return c;
    }
    let copy = Configuration::new_with(global.confs.read().unwrap().clone(), ctx, &global.app);
    let leaked: &'static Configuration = Box::leak(Box::new(copy));
    map.insert(ctx.to_string(), leaked);
    leaked
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AdvertPayload {
    executable: String,
    service_name: String,
    loggers: Vec<String>,
    version: String,
    commit: String,
    context: String,
    application: String,
    settings_file: String,
    test_settings_file: String,
    local_settings_file: String,
    host: String,
    address: String,
    start_time: String,
    app_payload: serde_json::Map<String, serde_json::Value>,
}

fn start_advertising(c: &Configuration) {
    let Some(url) = c.get("advertisingURL").filter(|u| !u.is_empty()) else {
        return;
    };
    let interval_str = c.get_or("advertisingInterval", "1m");
    stdlog(&format!(
        "Advertising service every {interval_str} to {url:?}"
    ));

    let interval = crate::utils::parse_go_duration(&interval_str)
        .unwrap_or(std::time::Duration::from_secs(60));
    let start_time = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let host = hostname();
    let executable = std::env::args().next().unwrap_or_default();
    let context = c.context.clone();
    let application = c.app.clone();
    let settings_file = c.settings_file.clone();
    let test_settings_file = c.test_settings_file.clone();
    let local_settings_file = c.local_settings_file.clone();

    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        loop {
            let payload = AdvertPayload {
                executable: executable.clone(),
                service_name: crate::get_package_name(),
                loggers: logger_names(),
                version: crate::get_version(),
                commit: crate::get_commit(),
                context: context.clone(),
                application: application.clone(),
                settings_file: settings_file.clone(),
                test_settings_file: test_settings_file.clone(),
                local_settings_file: local_settings_file.clone(),
                host: host.clone(),
                address: crate::get_address(),
                start_time: start_time.clone(),
                app_payload: crate::app_payloads(),
            };
            let agent = ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_millis(500))
                .build();
            if let Err(e) = agent.post(&url).send_json(&payload) {
                stdlog(&format!("Advertising ERROR {e}"));
            }
            std::thread::sleep(interval);
        }
    });
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

fn logger_names() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod mutation_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn set_unset_and_listeners() {
        let c = Configuration::new_with(Default::default(), "dev", "");
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        c.add_listener(move |_k, _v| {
            h.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(c.set("k", "v1"), None);
        assert_eq!(c.set("k", "v2"), Some("v1".to_string()));
        assert_eq!(c.get("k").unwrap(), "v2");
        assert_eq!(c.unset("k"), Some("v2".to_string()));
        assert_eq!(c.get("k"), None);
        assert_eq!(hits.load(Ordering::SeqCst), 3); // two sets + one unset

        let id = c.add_listener(|_, _| {});
        c.remove_listener(id);
    }

    #[test]
    fn stats_masks_secrets_and_annotates_context() {
        let mut m = std::collections::HashMap::new();
        m.insert("name".to_string(), "Simon".to_string());
        m.insert("name.live".to_string(), "Liam".to_string());
        m.insert(
            "secret".to_string(),
            "*EHE*8f7d64a1f1cefb44fe280d40bfe056ebd3aff457dd551ab8edf5d213cf9c".to_string(),
        );
        let c = Configuration::new_with(m, "live", "");
        let s = c.stats();
        assert!(s.contains("name[.live]=Liam"));
        assert!(!s.contains("8f7d64a1")); // ciphertext masked
        assert!(s.contains("Context:     live"));
    }

    #[test]
    fn get_all_includes_context_meta() {
        let mut m = std::collections::HashMap::new();
        m.insert("a".to_string(), "1".to_string());
        let c = Configuration::new_with(m, "dev", "");
        let all = c.get_all();
        assert_eq!(all.get("_SETTINGS_CONTEXT").unwrap(), "dev");
        assert_eq!(all.get("a").unwrap(), "1");
    }
}
