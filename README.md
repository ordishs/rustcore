# rustcore

rustcore is a Rust library of tools for configuring microservices and providing logging. It includes an embedded web front end for statistics and the ability to modify configuration settings without restarting the microservice itself.

It is a complete, file-and-wire-compatible rewrite of [gocore](https://github.com/ordishs/gocore). It reads the same settings files, uses the same context-fallback logic, the same `*EHE*` encrypted values, and shares the same `/tmp/gocore` Unix socket directory — so a single `rcli` (or the Go `gcli`) can administer both Go and Rust services running side by side.

The library is split into a few components:

* Config — structured, context-aware settings with typed getters
* Logger — levelled logging with regex-based realtime tracing
* Stats — realtime instrumentation with an embedded HTML web UI
* Utils — time formatting, crypto, argument parsing
* rcli — command-line admin client for any running rustcore/gocore service
* rustcore-format — settings file formatter


## Config

Config reads settings from two files and the environment. All settings are in the form `key=value`. The order of precedence is:

1. Environment variable
2. `settings_local.conf`
3. `settings.conf`

`settings_local.conf` is normally stored alongside the binary. `settings.conf` can live higher in the directory tree so settings are shared across multiple services. Both files are discovered by walking up from the executable directory, then from the current working directory.

### Getters

```rust
use rustcore::config::config;

// String
let val: Option<String> = config().get("key");
let val: String = config().get_or("key", "default");

// Typed parse (any T: FromStr)
let port: Option<i32> = config().get_parsed::<i32>("port")?;
let timeout: Option<Duration> = config().get_duration("timeout")?;
let flag: bool = config().get_bool("feature_enabled", false);
let endpoint: Option<url::Url> = config().get_url("database_url")?;
let items: Option<Vec<String>> = config().get_multi("tags", ",");
```

Examples:

```rust
// returns None if not set, Some("value") if set
let db = rustcore::config::config().get("database_url");

// returns 5432 if "database_port" is not set
let port = rustcore::config::config().get_parsed::<u16>("database_port")?.unwrap_or(5432);

// returns false if "happy" is not set or unparseable
if rustcore::config::config().get_bool("happy", false) {
    println!("I'm happy");
} else {
    println!("I'm sad");
}
```

### SETTINGS_CONTEXT

There is a concept of `SETTINGS_CONTEXT` set via the environment variable of the same name, defaulting to `"dev"`.

The general principle is to keep all application settings organised together so differences are visible in one place and the same value can be used across multiple contexts.

The way context works is best explained with an example. Given a `settings.conf`:

```conf
url=http://localhost:8080
url.live=https://www.server.com
url.live.uk=https://www.server.co.uk
```

and a program:

```rust
fn main() {
    if let Ok(Some(url)) = rustcore::config::config().get_url("url") {
        println!("URL is {url}");
    }
}
```

the value returned depends on `SETTINGS_CONTEXT`:

```
cargo run                           =>  http://localhost:8080
SETTINGS_CONTEXT=live cargo run     =>  https://www.server.com
SETTINGS_CONTEXT=live.uk cargo run  =>  https://www.server.co.uk
SETTINGS_CONTEXT=live.es cargo run  =>  https://www.server.com
```

The last case works because there is no `url.live.es`, so rustcore walks up the context hierarchy until it finds a match:

```
SETTINGS_CONTEXT=stage.eu.red cargo run  =>  http://localhost:8080
```

because rustcore tries:

1. `url.stage.eu.red` — not found
2. `url.stage.eu` — not found
3. `url.stage` — not found
4. `url` — `http://localhost:8080`

### SETTINGS_APPLICATION

Setting `SETTINGS_APPLICATION=myapp` enables per-application overrides. A key `url.live.myapp` takes precedence over `url.live` when both the context and app match.

### .env and SETTINGS_ENV_FILE

If a `.env` file is present in the working directory it is loaded before the settings files (via `dotenvy`). The path can be overridden with `SETTINGS_ENV_FILE`.

### Encrypted settings (*EHE*)

Values starting with `*EHE*` are AES-256-GCM encrypted. Use `rustcore-format` or the Go `gocore-format` tool to encrypt a value; the same key phrase is used in both implementations, so encrypted values are fully cross-compatible.

### Stats output

It is useful to log all settings at application startup:

```rust
println!("STATS\n{}\n-------\n", rustcore::config::config().stats());
```

### Running the config example

```
cd examples && cargo run --example config
```

The `examples/` directory contains a `settings.conf`. Running from that directory lets the config discovery walk find it.


## Logger

The rustcore logger is a simple levelled logger with useful features including regex-based realtime trace filtering via the admin socket.

```rust
fn main() {
    let logger = rustcore::log("myservice");
    logger.debug("debug message");
    logger.info(format!("starting on port {}", 8080));
    logger.warn("low disk space");
    logger.error("connection refused");
}
```

Log levels (from lowest to highest): `DEBUG`, `INFO`, `WARN`, `ERROR`, `FATAL`, `PANIC`.

The log level is read from the `logLevel` setting (defaulting to `INFO`). Lines are written to stderr with timestamp, file:line, package name, level, and message.

The logger creates a Unix socket at `/tmp/gocore/MYSERVICE.sock` (the package name uppercased). You can connect to it with:

```
nc -U /tmp/gocore/MYSERVICE.sock
```

or use `rcli`:

```
cargo run --bin rcli -- -packageName MYSERVICE status
```

### rcli admin commands

```
status                      show log level, package, settings
config get KEY              print the current value of a config key
config set KEY VALUE        change a config value at runtime (no restart needed)
config stats                print all settings with context annotations
trace on [REGEX]            start streaming log lines to this connection (optional filter)
trace off                   stop trace stream
loglevel DEBUG|INFO|...     change the log level at runtime
```

Example — raise trace verbosity live:

```
cargo run --bin rcli -- -packageName MYSERVICE -keepAlive trace on
```

The same `rcli` binary works against gocore services over the same socket protocol.

### Running the logger example

In one terminal:

```
cd examples && cargo run --example logger
```

In another:

```
./target/debug/rcli -packageName EXAMPLE status
./target/debug/rcli -packageName EXAMPLE -keepAlive trace on
```

When done, kill the logger process; the socket at `/tmp/gocore/EXAMPLE.sock` is cleaned up automatically.


## Stats

Stats provides a tree of named counters with timing distributions (count, min, max, average, first, last, total) visualised in an embedded HTML web UI.

```rust
use rustcore::{new_stat, current_time, start_stats_server};

fn main() {
    let stat = new_stat("my-operation");
    std::thread::spawn(move || loop {
        let start = current_time();
        do_work();
        stat.add_time(start);
    });
    // blocking; call from a spawned thread to run alongside other work
    start_stats_server("localhost:9009");
}
```

Navigate to `http://localhost:9009/stats` for the live HTML dashboard.

To embed the stats page in your own HTTP handler:

```rust
use rustcore::stats::{render_stats_page, serve_embedded_asset};

let html = render_stats_page("");           // drill-down key path, "" = root
let (ct, body) = serve_embedded_asset("css/statistics.css").unwrap();
```

### Running the server example

```
cd examples && cargo run --example server
```

Then:

```
curl localhost:9009/stats
```


## rustcore-format

`rustcore-format` is a settings file formatter. It sorts and aligns `settings.conf` entries, supports `# @group:` / `# @endgroup` markers for logical grouping, and aligns `=` signs within groups.

```
cargo run --bin rustcore-format -- settings.conf
cargo run --bin rustcore-format -- -w settings.conf   # write back in place
```

The formatter is compatible with `gocore-format` and produces identical output for identical input.


## Compatibility with gocore

rustcore is designed to be a drop-in companion to gocore:

- Reads the same `settings.conf` / `settings_local.conf` files
- Same `SETTINGS_CONTEXT` fallback algorithm
- Same `SETTINGS_APPLICATION` per-app overrides
- Same `*EHE*` AES-256-GCM encrypted values (same key, same wire format)
- Same `/tmp/gocore/NAME.sock` Unix socket protocol — `rcli` and `gcli` are interchangeable
- Same advertising beacon POST payload format


## Quick start

```toml
# Cargo.toml
[dependencies]
rustcore = { path = "../rustcore" }
```

```rust
use rustcore::config::config;
use rustcore::{log, new_stat, current_time, start_stats_server};

fn main() {
    rustcore::set_info("myservice", env!("CARGO_PKG_VERSION"), "");

    println!("{}", config().stats());

    let logger = log("myservice");
    logger.info("service starting");

    let stat = new_stat("request");
    std::thread::spawn(move || loop {
        let start = current_time();
        std::thread::sleep(std::time::Duration::from_millis(10));
        stat.add_time(start);
    });

    start_stats_server("0.0.0.0:9009");
}
```
