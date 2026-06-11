use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::Location;
use std::sync::{LazyLock, Mutex, RwLock};

use crate::config::config;
use crate::sampler::Sampler;
use crate::stdlog;
use crate::utils::is_regex_match;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
    Fatal = 4,
    Panic = 5,
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Fatal => "FATAL",
            Level::Panic => "PANIC",
        })
    }
}

impl std::str::FromStr for Level {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_uppercase().as_str() {
            "INFO" => Level::Info,
            "WARN" => Level::Warn,
            "ERROR" => Level::Error,
            "FATAL" => Level::Fatal,
            "PANIC" => Level::Panic,
            _ => Level::Debug, // Go parity: unknown -> 0 (DEBUG)
        })
    }
}

fn ansi(s: &str, colour: &str) -> String {
    let code = match colour {
        "blue" => "34",
        "green" => "32",
        "yellow" => "33",
        "red" => "31",
        "cyan" => "36",
        "magenta" => "35",
        _ => return s.to_string(),
    };
    format!("\x1b[{code}m{s}\x1b[0m")
}

/// printf subset: %s with optional - flag and width.
pub(crate) fn sprintf(format: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    let mut ai = 0;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut left = false;
        let mut width = 0usize;
        loop {
            match chars.peek() {
                Some('-') => {
                    left = true;
                    chars.next();
                }
                Some(d) if d.is_ascii_digit() => {
                    width = width * 10 + d.to_digit(10).unwrap() as usize;
                    chars.next();
                }
                Some('s') => {
                    chars.next();
                    break;
                }
                _ => break,
            }
        }
        let arg = args.get(ai).copied().unwrap_or("");
        ai += 1;
        if left {
            out.push_str(&format!("{arg:<width$}"));
        } else {
            out.push_str(&format!("{arg:>width$}"));
        }
    }
    out
}

struct LoggerInner {
    level: Level,
    /// connection id -> (write half, regex filter; empty = match everything via Go semantics)
    trace_sockets: HashMap<u64, (UnixStream, String)>,
    samplers: Vec<std::sync::Arc<Sampler>>,
}

pub struct Logger {
    package_name: String,
    colour: bool,
    show_timestamp: bool,
    inner: Mutex<LoggerInner>,
    pub(crate) listener: Mutex<Option<UnixListener>>,
    pub(crate) socket_path: Mutex<Option<std::path::PathBuf>>,
}

static LOGGERS: LazyLock<RwLock<HashMap<String, &'static Logger>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

static OUTPUT_FORMAT: LazyLock<String> =
    LazyLock::new(|| config().get_or("logger_output_format", "| %-20s| %-5s| %s |"));

pub fn registered_names() -> Vec<String> {
    LOGGERS.read().unwrap().keys().cloned().collect()
}

pub fn log(package_name: &str) -> &'static Logger {
    log_with_level(package_name, None)
}

pub fn log_with_level(package_name: &str, level: Option<Level>) -> &'static Logger {
    if let Some(l) = LOGGERS.read().unwrap().get(package_name) {
        return l;
    }
    let mut map = LOGGERS.write().unwrap();
    if let Some(l) = map.get(package_name) {
        return l;
    }

    let ll = level.unwrap_or_else(|| config().get_or("logLevel", "INFO").parse().unwrap());
    let show_timestamp = config().get_bool("logger_show_timestamps", true);
    if !show_timestamp {
        crate::SHOW_STD_TIMESTAMP.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    let logger: &'static Logger = Box::leak(Box::new(Logger {
        package_name: package_name.to_string(),
        colour: true,
        show_timestamp,
        inner: Mutex::new(LoggerInner {
            level: ll,
            trace_sockets: HashMap::new(),
            samplers: Vec::new(),
        }),
        listener: Mutex::new(None),
        socket_path: Mutex::new(None),
    }));

    if let Err(e) = crate::socket::start_socket_listener(logger) {
        logger.fatal(format!("LOGGER: {e}"));
    }

    map.insert(package_name.to_string(), logger);
    logger
}

impl Logger {
    pub fn package_name(&self) -> &str {
        &self.package_name
    }

    pub fn get_level(&self) -> Level {
        self.inner.lock().unwrap().level
    }

    pub fn set_level(&self, level: Level) {
        self.inner.lock().unwrap().level = level;
    }

    pub fn add_sampler(&self, s: std::sync::Arc<Sampler>) {
        self.inner.lock().unwrap().samplers.push(s);
    }

    // Used by socket.rs (Task 15)
    pub(crate) fn add_trace(&self, id: u64, stream: UnixStream, regex: String) {
        self.inner
            .lock()
            .unwrap()
            .trace_sockets
            .insert(id, (stream, regex));
    }

    pub(crate) fn remove_trace(&self, id: u64) {
        self.inner.lock().unwrap().trace_sockets.remove(&id);
    }

    pub(crate) fn clear_traces(&self) {
        self.inner.lock().unwrap().trace_sockets.clear();
    }

    pub(crate) fn has_trace(&self, id: u64) -> bool {
        self.inner.lock().unwrap().trace_sockets.contains_key(&id)
    }

    pub(crate) fn colour(&self) -> bool {
        self.colour
    }

    pub(crate) fn show_timestamp(&self) -> bool {
        self.show_timestamp
    }

    /// Returns (print, can_return) — Go loggingNecessary.
    fn logging_necessary(&self, ll: Level) -> (bool, bool) {
        let inner = self.inner.lock().unwrap();
        let print = ll >= inner.level;
        if !inner.trace_sockets.is_empty() || !inner.samplers.is_empty() {
            (print, false)
        } else {
            (print, !print)
        }
    }

    fn output(&self, ll: Level, colour: &str, msg: &str, loc: &Location) {
        let (print, can_return) = self.logging_necessary(ll);
        if can_return {
            return;
        }

        let level_plain = format!("{:<5}", ll.to_string());
        let level = if self.colour {
            ansi(&level_plain, colour)
        } else {
            level_plain.clone()
        };

        let file = loc.file().rsplit('/').next().unwrap_or("???");
        let file_line = format!("{}:{}", file, loc.line());

        let mut line = sprintf(&OUTPUT_FORMAT, &[&file_line, &self.package_name, &level]);
        if !msg.is_empty() {
            line.push(' ');
            line.push_str(msg);
        }

        if print {
            stdlog(&line);
        }

        let mut s = String::new();
        if self.show_timestamp {
            s.push_str(
                &chrono::Utc::now()
                    .format("%Y-%m-%d %H:%M:%S%.3f ")
                    .to_string(),
            );
        }
        s.push_str(&line);
        if !s.ends_with('\n') {
            s.push('\n');
        }

        self.send_to_trace(&s, &level_plain);
        self.send_to_sample(&s, &level_plain);
    }

    fn send_to_trace(&self, s: &str, level: &str) {
        let mut inner = self.inner.lock().unwrap();
        let mut dead = Vec::new();
        for (id, (stream, regex)) in inner.trace_sockets.iter_mut() {
            let matches = is_regex_match(regex, s)
                || is_regex_match(&regex.to_lowercase(), &level.to_lowercase());
            if matches && stream.write_all(s.as_bytes()).is_err() {
                dead.push(*id);
            }
        }
        for id in dead {
            inner.trace_sockets.remove(&id);
        }
    }

    fn send_to_sample(&self, s: &str, level: &str) {
        let inner = self.inner.lock().unwrap();
        for sampler in inner.samplers.iter() {
            if is_regex_match(&sampler.regex, s)
                || is_regex_match(&sampler.regex.to_lowercase(), &level.to_lowercase())
            {
                sampler.write(s);
            }
        }
    }

    fn close_socket(&self) {
        *self.listener.lock().unwrap() = None;
        if let Some(p) = self.socket_path.lock().unwrap().take() {
            let _ = std::fs::remove_file(p);
        }
    }

    #[track_caller]
    pub fn debug(&self, msg: impl std::fmt::Display) {
        self.output(Level::Debug, "blue", &msg.to_string(), Location::caller());
    }

    #[track_caller]
    pub fn info(&self, msg: impl std::fmt::Display) {
        self.output(Level::Info, "green", &msg.to_string(), Location::caller());
    }

    #[track_caller]
    pub fn warn(&self, msg: impl std::fmt::Display) {
        self.output(Level::Warn, "yellow", &msg.to_string(), Location::caller());
    }

    #[track_caller]
    pub fn error(&self, msg: impl std::fmt::Display) {
        self.output(Level::Error, "red", &msg.to_string(), Location::caller());
    }

    #[track_caller]
    pub fn error_with_stack(&self, msg: impl std::fmt::Display) {
        let bt = std::backtrace::Backtrace::force_capture();
        self.output(
            Level::Error,
            "red",
            &format!("{msg}\n{bt}"),
            Location::caller(),
        );
    }

    #[track_caller]
    pub fn fatal(&self, msg: impl std::fmt::Display) -> ! {
        self.output(Level::Fatal, "cyan", &msg.to_string(), Location::caller());
        self.close_socket();
        std::process::exit(1);
    }

    #[track_caller]
    pub fn panic(&self, msg: impl std::fmt::Display) -> ! {
        let m = msg.to_string();
        self.output(Level::Panic, "magenta", &m, Location::caller());
        self.close_socket();
        panic!("{m}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_parsing() {
        assert_eq!("INFO".parse::<Level>().unwrap(), Level::Info);
        assert_eq!("debug".parse::<Level>().unwrap(), Level::Debug);
        assert_eq!("nonsense".parse::<Level>().unwrap(), Level::Debug); // Go: unknown -> 0
        assert_eq!(Level::Warn.to_string(), "WARN");
        assert!(Level::Error > Level::Info);
    }

    #[test]
    fn sprintf_subset() {
        assert_eq!(
            sprintf("| %-20s| %-5s| %s |", &["main.rs:10", "pkg", "INFO "]),
            "| main.rs:10          | pkg  | INFO  |"
        );
        assert_eq!(sprintf("%s-%s", &["a", "b"]), "a-b");
        assert_eq!(sprintf("%5s", &["ab"]), "   ab");
    }

    #[test]
    fn registry_returns_same_instance() {
        let a = log("testpkg-registry");
        let b = log("testpkg-registry");
        assert!(std::ptr::eq(a, b));
    }

    #[test]
    fn level_filtering() {
        let l = log("testpkg-filter");
        l.set_level(Level::Warn);
        assert_eq!(l.get_level(), Level::Warn);
        // (print, can_return) semantics
        assert_eq!(l.logging_necessary(Level::Debug), (false, true));
        assert_eq!(l.logging_necessary(Level::Error), (true, false));
    }
}
