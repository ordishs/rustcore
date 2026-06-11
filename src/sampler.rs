use std::fs::File;
use std::io::Write;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

pub struct Sampler {
    pub id: String,
    pub filename: String,
    pub regex: String,
    tx: Mutex<Option<Sender<String>>>,
}

impl Sampler {
    pub fn new(id: &str, filename: &str, regex: &str) -> std::io::Result<Arc<Sampler>> {
        let file = File::create(filename)?;
        let (tx, rx) = channel::<String>();

        let sampler = Arc::new(Sampler {
            id: id.to_string(),
            filename: filename.to_string(),
            regex: regex.to_string(),
            tx: Mutex::new(Some(tx)),
        });

        let weak = Arc::downgrade(&sampler);
        std::thread::spawn(move || {
            let mut file = file;
            for msg in rx {
                if file.write_all(msg.as_bytes()).is_err() {
                    if let Some(s) = weak.upgrade() {
                        crate::stdlog(&format!("Sampler {s} failed to write to file"));
                        s.stop();
                    }
                    break;
                }
            }
            // rx closed -> file dropped/closed here
        });

        Ok(sampler)
    }

    /// Non-blocking-safe: writes after stop are silently dropped (Go recovers a panic here).
    pub fn write(&self, s: &str) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(s.to_string());
        }
    }

    pub fn stop(&self) {
        *self.tx.lock().unwrap() = None; // drops Sender, closes channel
    }
}

impl std::fmt::Display for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let abs = std::fs::canonicalize(&self.filename)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| self.filename.clone());
        if self.regex.is_empty() {
            write!(f, "Sampler {}: writing all logs to {}", self.id, abs)
        } else {
            write!(
                f,
                "Sampler {}: writing logs that match {:?} to {}",
                self.id, self.regex, abs
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.log");
        let s = Sampler::new("s1", path.to_str().unwrap(), "ERROR").unwrap();
        s.write("line one\n");
        s.write("line two\n");
        s.stop();
        // writer thread drains before closing; poll briefly
        for _ in 0..50 {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            if content == "line one\nline two\n" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "line one\nline two\n"
        );
        s.write("after stop\n"); // must not panic
        assert!(s.to_string().contains("s1"));
    }
}
