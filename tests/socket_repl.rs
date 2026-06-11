use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;

#[test]
fn repl_session() {
    let _logger = rustcore::log("repltest");
    let path = "/tmp/gocore/REPLTEST.sock";

    // listener starts on a thread; retry connect briefly
    let mut stream = None;
    for _ in 0..50 {
        if let Ok(s) = UnixStream::connect(path) {
            stream = Some(s);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let stream = stream.expect("socket should be listening");
    let mut w = stream.try_clone().unwrap();
    let r = BufReader::new(stream);

    w.write_all(b"config set foo bar\nconfig get foo\nloglevel DEBUG\nstatus\nquit\n")
        .unwrap();

    let mut all = String::new();
    let mut buf = [0u8; 65536];
    use std::io::Read;
    loop {
        match r.get_ref().read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => all.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
    }

    assert!(all.contains("Welcome to rustcore for repltest"));
    assert!(all.contains("Created new setting: foo=bar"));
    assert!(all.contains("foo=bar"));
    assert!(all.contains("Log level set to DEBUG"));
    assert!(all.contains("Log level: DEBUG"));
}
