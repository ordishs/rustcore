use std::io::Read;

#[test]
fn advertising_posts_payload() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    unsafe {
        std::env::set_var("advertisingURL", format!("http://{addr}/ad"));
        std::env::set_var("advertisingInterval", "1s");
    }

    let _ = rustcore::config::config(); // triggers beacon

    listener.set_nonblocking(false).unwrap();
    let (mut stream, _) = listener.accept().unwrap(); // beacon fires after ~1s
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap();
    let req = String::from_utf8_lossy(&buf[..n]);
    assert!(req.starts_with("POST /ad"));
    assert!(req.contains("serviceName"));
}
