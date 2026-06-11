pub mod config;
pub mod logger;
pub mod sampler;
pub mod socket;
pub mod utils;

pub use logger::{log, Level, Logger};

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, RwLock};

static PACKAGE_NAME: RwLock<Option<String>> = RwLock::new(None);
static VERSION: RwLock<Option<String>> = RwLock::new(None);
static COMMIT: RwLock<Option<String>> = RwLock::new(None);
static ADDRESS: RwLock<Option<String>> = RwLock::new(None);

type PayloadFn = Box<dyn Fn() -> serde_json::Value + Send + Sync>;
static APP_PAYLOAD_FNS: LazyLock<RwLock<HashMap<String, PayloadFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub(crate) static SHOW_STD_TIMESTAMP: AtomicBool = AtomicBool::new(true);

pub fn set_info(name: &str, version: &str, commit: &str) {
    *PACKAGE_NAME.write().unwrap() = Some(name.to_string());
    *VERSION.write().unwrap() = Some(version.to_string());
    *COMMIT.write().unwrap() = Some(commit.to_string());
}

pub fn set_address(addr: &str) {
    *ADDRESS.write().unwrap() = Some(addr.to_string());
}

fn read_or_unknown(slot: &RwLock<Option<String>>) -> String {
    slot.read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "Unknown".to_string())
}

pub fn get_package_name() -> String {
    read_or_unknown(&PACKAGE_NAME)
}

pub fn get_version() -> String {
    read_or_unknown(&VERSION)
}

pub fn get_commit() -> String {
    read_or_unknown(&COMMIT)
}

pub fn get_address() -> String {
    read_or_unknown(&ADDRESS)
}

pub fn add_app_payload_fn(key: &str, f: impl Fn() -> serde_json::Value + Send + Sync + 'static) {
    APP_PAYLOAD_FNS
        .write()
        .unwrap()
        .insert(key.to_string(), Box::new(f));
}

pub(crate) fn app_payloads() -> serde_json::Map<String, serde_json::Value> {
    APP_PAYLOAD_FNS
        .read()
        .unwrap()
        .iter()
        .map(|(k, f)| (k.clone(), f()))
        .collect()
}

/// Internal stderr logger matching Go's `log.Printf` default format.
pub(crate) fn stdlog(msg: &str) {
    if SHOW_STD_TIMESTAMP.load(Ordering::Relaxed) {
        eprintln!(
            "{} {}",
            chrono::Local::now().format("%Y/%m/%d %H:%M:%S"),
            msg
        );
    } else {
        eprintln!("{msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_info_defaults_and_set() {
        assert_eq!(get_version(), "Unknown");
        set_info("myservice", "1.2.3", "abc123");
        assert_eq!(get_package_name(), "myservice");
        assert_eq!(get_version(), "1.2.3");
        assert_eq!(get_commit(), "abc123");
        set_address("1.2.3.4:80");
        assert_eq!(get_address(), "1.2.3.4:80");
    }
}
