use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;

#[test]
fn repl_session() {
    let _logger = rustcore::log("repltest");
    let path = "/tmp/rustcore/REPLTEST.sock";

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

    // TestHandleConfig cases via the REPL:
    //
    //   "config set bob2 simon says"       -> args ["config","set","bob2","simon","says"]
    //                                         len 5 >= 4 -> key="bob2", value="simon says"
    //                                         -> Created new setting: bob2=simon says
    //
    //   "config set bob2 simon says again" -> args ["config","set","bob2","simon","says","again"]
    //                                         key="bob2", value="simon says again"
    //                                         -> Updated setting: bob2 simon says -> simon says again
    //
    //   "config set bob3=hello"            -> args ["config","set","bob3=hello"]
    //                                         len 3, contains '=' -> key="bob3", value="hello"
    //                                         -> Created new setting: bob3=hello
    w.write_all(
        b"config set foo bar\nconfig get foo\nloglevel DEBUG\nstatus\n\
          config set bob2 simon says\n\
          config set bob2 simon says again\n\
          config set bob3=hello\n\
          quit\n",
    )
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

    // --- original assertions ---
    assert!(all.contains("Welcome to rustcore for repltest"));
    assert!(all.contains("Created new setting: foo=bar"));
    assert!(all.contains("foo=bar"));
    assert!(all.contains("Log level set to DEBUG"));
    assert!(all.contains("Log level: DEBUG"));

    // --- TestHandleConfig parity ---
    // "config set bob2 simon says" -> new key
    assert!(
        all.contains("Created new setting: bob2=simon says"),
        "TestHandleConfig: expected 'Created new setting: bob2=simon says'\ngot: {all}"
    );
    // "config set bob2 simon says again" -> update existing key
    assert!(
        all.contains("Updated setting: bob2 simon says -> simon says again"),
        "TestHandleConfig: expected 'Updated setting: bob2 simon says -> simon says again'\ngot: {all}"
    );
    // "config set bob3=hello" -> key=value form
    assert!(
        all.contains("Created new setting: bob3=hello"),
        "TestHandleConfig: expected 'Created new setting: bob3=hello'\ngot: {all}"
    );
}
