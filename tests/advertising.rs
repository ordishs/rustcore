use std::io::Read;
use std::time::{Duration, Instant};

#[test]
fn advertising_posts_payload() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    unsafe {
        std::env::set_var("advertisingURL", format!("http://{addr}/ad"));
        std::env::set_var("advertisingInterval", "1s");
    }

    let _ = rustcore::config::config(); // triggers beacon

    // Poll accept with a 15s deadline so the test fails clearly instead of hanging
    // or missing the beacon when the process is slow to start.
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    panic!("advertising beacon did not connect within 15s");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("accept error: {e}"),
        }
    };

    // Switch back to blocking with a read timeout so we never hang
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap();
    let req = String::from_utf8_lossy(&buf[..n]);
    assert!(req.starts_with("POST /ad"), "expected POST /ad, got: {req}");
    assert!(
        req.contains("serviceName"),
        "payload missing serviceName: {req}"
    );
}
