use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

fn main() {
    let mut socket_dir = "/tmp/rustcore".to_string();
    let mut package_name = String::new();
    let mut keep_alive = false;
    let mut rest: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-socketDir" => socket_dir = args.next().unwrap_or_default(),
            "-packageName" => package_name = args.next().unwrap_or_default(),
            "-keepAlive" => keep_alive = true,
            _ if arg.starts_with("-socketDir=") => {
                socket_dir = arg["-socketDir=".len()..].to_string()
            }
            _ if arg.starts_with("-packageName=") => {
                package_name = arg["-packageName=".len()..].to_string()
            }
            _ => {
                let a = if arg.contains(' ') {
                    format!("{arg:?}")
                } else {
                    arg
                };
                rest.push(a);
            }
        }
    }

    let addr: PathBuf = if package_name.is_empty() {
        let socks: Vec<PathBuf> = std::fs::read_dir(&socket_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|e| e == "sock").unwrap_or(false))
                    .collect()
            })
            .unwrap_or_default();
        match socks.len() {
            1 => socks.into_iter().next().unwrap(),
            0 => {
                eprintln!("No rustcore processes are running.");
                std::process::exit(1);
            }
            n => {
                eprintln!("There are {n} sockets and no packageName specified.");
                std::process::exit(1);
            }
        }
    } else {
        PathBuf::from(format!("{socket_dir}/{package_name}.sock"))
    };

    let mut stream = match UnixStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let command = rest.join(" ");
    let mut reader = stream.try_clone().expect("clone stream");

    let writer = std::thread::spawn(move || {
        let _ = stream.write_all(format!("{command}\n").as_bytes());
        if keep_alive {
            let mut stdin = std::io::stdin();
            let _ = std::io::copy(&mut stdin, &mut stream);
        } else {
            let _ = stream.write_all(b"quit\n");
        }
        let _ = stream.shutdown(std::net::Shutdown::Write);
    });

    let mut stdout = std::io::stdout();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let _ = stdout.write_all(&buf[..n]);
            }
        }
    }
    let _ = writer.join();
}
