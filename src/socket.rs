use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::config;
use crate::logger::{Level, Logger};
use crate::stdlog;
use crate::utils::split_args;

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

pub fn start_socket_listener(logger: &'static Logger) -> std::io::Result<()> {
    let socket_dir = config().get_or("socketDIR", "/tmp/gocore");
    if let Err(e) = std::fs::create_dir_all(&socket_dir) {
        stdlog(&format!(
            "ERROR: Unable to make socket directory {socket_dir}: {e}"
        ));
    }

    let socket_path = std::path::PathBuf::from(format!(
        "{}/{}.sock",
        socket_dir,
        logger.package_name().to_uppercase()
    ));
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    *logger.listener.lock().unwrap() = Some(listener.try_clone()?);
    *logger.socket_path.lock().unwrap() = Some(socket_path.clone());

    if config().get_bool("logger_show_socket_info", true) {
        logger.info(format!(
            "Socket created. Connect with: nc -U {}",
            socket_path.display()
        ));
    }

    std::thread::spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    std::thread::spawn(move || handle(logger, stream));
                }
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_file(&socket_path);
    });

    Ok(())
}

fn write_to(stream: &mut UnixStream, s: &str) -> std::io::Result<()> {
    stream.write_all(s.as_bytes())
}

fn handle(logger: &'static Logger, stream: UnixStream) {
    let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::SeqCst);
    let mut w = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let reader = BufReader::new(stream);

    let _ = write_to(
        &mut w,
        &format!("\nWelcome to rustcore for {}\n\n", logger.package_name()),
    );
    let prompt = format!("rustcore::{}> ", logger.package_name());

    let mut lines = reader.lines();
    loop {
        if write_to(&mut w, &prompt).is_err() {
            break;
        }
        let Some(Ok(line)) = lines.next() else { break };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let args = match split_args(&line) {
            Ok(a) => a,
            Err(e) => {
                if write_to(&mut w, &format!("  Cannot split command: {e}\n\n")).is_err() {
                    return;
                }
                continue;
            }
        };

        match args[0].as_str() {
            "config" => handle_config(&mut w, &args),
            "trace" => handle_trace(logger, &mut w, conn_id, &args, "TRACE", None),
            "loglevel" => {
                if args.len() <= 1 {
                    let _ = write_to(
                        &mut w,
                        "  Invalid number of parameters. Use 'help' to see the syntax.\n\n",
                    );
                } else {
                    logger.set_level(args[1].parse::<Level>().unwrap());
                    let _ = write_to(&mut w, &format!("  Log level set to {}\n\n", args[1]));
                }
            }
            "sample" => handle_sample(logger, &mut w, conn_id, &args),
            "status" => {
                let _ = write_to(&mut w, "\nStatus:\n");
                let _ = write_to(&mut w, &format!("  Log level: {}\n", logger.get_level()));
                let _ = write_to(&mut w, &format!("  Package: {}\n", logger.package_name()));
                let _ = write_to(&mut w, "  Colour: true\n");
                let _ = write_to(&mut w, "  Show timestamps: true\n");
                let _ = write_to(&mut w, "\n");
            }
            "quit" | "exit" => return,
            "help" => {
                let _ = write_to(&mut w, HELP);
            }
            _ => {
                if write_to(&mut w, &format!("  Command not found: {line}\n\n")).is_err() {
                    return;
                }
            }
        }
    }
}

fn handle_config(w: &mut UnixStream, args: &[String]) {
    if args.len() <= 1 {
        let _ = write_to(
            w,
            "  Invalid number of parameters. Use 'help' to see the syntax.\n\n",
        );
        return;
    }
    match args[1].as_str() {
        "requested" => {
            let _ = write_to(w, &format!("\n{}\n\n", config().requested()));
        }
        "show" => {
            let _ = write_to(w, &format!("{}\n\n", config().stats()));
        }
        "get" => {
            if args.len() < 3 {
                let _ = write_to(
                    w,
                    "  Invalid number of parameters. Use 'help' to see the syntax.\n\n",
                );
                return;
            }
            match config().get(&args[2]) {
                None => {
                    let _ = write_to(w, "  Not set\n\n");
                }
                Some(v) => {
                    let _ = write_to(w, &format!("  {}={}\n\n", args[2], v));
                }
            }
        }
        "set" => {
            if args.len() < 3 {
                let _ = write_to(
                    w,
                    "  Invalid number of parameters. Use 'help' to see the syntax.\n\n",
                );
                return;
            }
            let (key, value) = if args.len() >= 4 {
                (args[2].clone(), args[3..].join(" "))
            } else if args[2].contains('=') {
                let parts: Vec<&str> = args[2].trim().splitn(2, '=').collect();
                (parts[0].trim().to_string(), parts[1].trim().to_string())
            } else {
                let _ = write_to(
                    w,
                    "  Invalid format. Use either 'set key value' or 'set key=value'\n\n",
                );
                return;
            };
            if key.is_empty() {
                let _ = write_to(w, "  Key cannot be empty\n\n");
                return;
            }
            match config().set(&key, &value) {
                Some(old) if old == value => {
                    let _ = write_to(w, "  No change\n\n");
                }
                Some(old) => {
                    let _ = write_to(w, &format!("  Updated setting: {key} {old} -> {value}\n\n"));
                }
                None => {
                    let _ = write_to(w, &format!("  Created new setting: {key}={value}\n\n"));
                }
            }
        }
        "unset" => {
            if args.len() < 3 {
                let _ = write_to(
                    w,
                    "  Invalid number of parameters. Use 'help' to see the syntax.\n\n",
                );
                return;
            }
            match config().unset(&args[2]) {
                None => {
                    let _ = write_to(w, "  No change\n\n");
                }
                Some(old) => {
                    let _ = write_to(w, &format!("  Removed setting: {}={}\n\n", args[2], old));
                }
            }
        }
        _ => {
            let _ = write_to(w, "  Invalid command. Use 'help' to see the syntax.\n\n");
        }
    }
}

fn handle_trace(
    logger: &'static Logger,
    w: &mut UnixStream,
    conn_id: u64,
    args: &[String],
    context: &str,
    regex: Option<&str>,
) {
    if args.len() <= 1 {
        let _ = write_to(
            w,
            "  Invalid number of parameters. Use 'help' to see the syntax.\n\n",
        );
        return;
    }
    match args[1].as_str() {
        "on" => {
            if !logger.has_trace(conn_id) {
                if let Ok(clone) = w.try_clone() {
                    logger.add_trace(conn_id, clone, regex.unwrap_or("").to_string());
                }
            }
            let _ = write_to(w, &format!("  {context} ON\n\n"));
        }
        "off" => {
            logger.remove_trace(conn_id);
            let _ = write_to(w, &format!("  {context} OFF\n\n"));
        }
        "clear" => {
            logger.clear_traces();
            let _ = write_to(w, &format!("  {context} CLEARED\n\n"));
        }
        _ => {
            let _ = write_to(w, "  Invalid parameter. Use 'help' to see the syntax.\n\n");
        }
    }
}

fn handle_sample(logger: &'static Logger, w: &mut UnixStream, conn_id: u64, args: &[String]) {
    if args.len() <= 1 {
        let _ = write_to(
            w,
            "  Invalid number of parameters. Use 'help' to see the syntax.\n\n",
        );
        return;
    }
    match args[1].as_str() {
        "on" => {
            if args.len() < 3 {
                let _ = write_to(
                    w,
                    "  Invalid number of parameters. Use 'help' to see the syntax.\n\n",
                );
                return;
            }
            // Go parity: sample on registers in the trace map with a regex filter.
            if !logger.has_trace(conn_id) {
                if let Ok(clone) = w.try_clone() {
                    logger.add_trace(conn_id, clone, args[2].clone());
                }
            }
            let _ = write_to(w, "  SAMPLE ON\n\n");
        }
        "off" => {
            logger.remove_trace(conn_id);
            let _ = write_to(w, "  SAMPLE OFF\n\n");
        }
        "clear" => {
            logger.clear_traces();
            let _ = write_to(w, "  SAMPLE CLEARED\n\n");
        }
        _ => {
            let _ = write_to(w, "  Invalid parameter. Use 'help' to see the syntax.\n\n");
        }
    }
}

const HELP: &str = "
Commands:
  config
    show                    Show all configuration settings
    requested              Show all requested configuration settings
    get <key>              Get a configuration setting
    set <key> <value>      Set a configuration setting
    unset <key>            Remove a configuration setting

  loglevel <level>         Set the log level (DEBUG, INFO, WARN, ERROR, FATAL)

  trace
    on                     Turn on tracing for this connection
    off                    Turn off tracing for this connection
    clear                  Clear all trace connections

  sample
    on <regex>             Turn on sampling for this connection with regex filter
    off                    Turn off sampling for this connection
    clear                  Clear all sample connections

  status                   Show current status

  help                     Show this help message

  quit                     Close the connection

";
