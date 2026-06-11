# rustcore — Design Specification

Date: 2026-06-11
Status: Approved

## Purpose

rustcore is a complete Rust rewrite of [gocore](https://github.com/ordishs/gocore) (~3,000 lines of Go): a microservice toolkit providing configuration, logging, a Unix-socket admin REPL, realtime statistics with an embedded web UI, and supporting utilities.

## Goals and constraints

1. **Full feature parity** with gocore, including the advertising beacon, gcli equivalent, and gocore-format equivalent.
2. **Full file/wire compatibility**: reads the same `settings.conf` / `settings_local.conf` / `settings_test.conf` / `.env` files with identical precedence and `SETTINGS_CONTEXT` fallback semantics; decrypts the same `*EHE*` values (identical key derivation and AES-256-GCM scheme); uses the same `/tmp/gocore/{PACKAGENAME}.sock` sockets so one CLI manages Go and Rust services alike.
3. **Std threads only** — no async runtime. Minimal dependency footprint.
4. **Idiomatic Rust API**: `Option`/`Result` returns, a generic `get_parsed<T>` instead of Go's per-width getter family, closures instead of listener interfaces, `#[track_caller]` instead of `runtime.Caller`.
5. Own git repo at `~/dev/rust/rustcore`. MIT licensed (same as gocore).

Out of scope: `cli/encrypt.go` (empty `main` in Go — nothing to port), `gcli/build` artifacts, `.history`, examples/server jQuery assets beyond what the stats UI needs.

## Repo layout

```
rustcore/
├── Cargo.toml                 (lib "rustcore" + bins "rcli", "rustcore-format")
├── README.md, LICENSE, .gitignore
├── src/
│   ├── lib.rs                 (re-exports; set_info/set_address/get_package_name/
│   │                           get_version/get_commit/get_address; add_app_payload_fn)
│   ├── config.rs
│   ├── logger.rs
│   ├── socket.rs
│   ├── stats.rs
│   ├── sampler.rs
│   ├── utils/
│   │   ├── mod.rs
│   │   ├── human_time.rs      (human_time)
│   │   ├── human_time_unit.rs (human_time_unit, _with_colour, _html)
│   │   ├── secure_settings.rs (encrypt, decrypt_setting — *EHE* scheme)
│   │   ├── split_args.rs
│   │   ├── outbound_ip.rs     (get_outbound_ip)
│   │   └── regex_match.rs     (is_regex_match)
│   └── bin/
│       ├── rcli.rs
│       └── rustcore-format.rs
├── embed/
│   ├── css/statistics.css     (copied from gocore)
│   └── js/chili-1.8b.js       (copied from gocore)
├── examples/
│   ├── config.rs
│   ├── logger.rs
│   └── server.rs
└── tests/                     (integration tests; fixtures in tests/fixtures/)
```

## Dependencies

| Crate | Purpose |
|---|---|
| `regex` | trace/sample matching, `${var}` interpolation, `*EHE*` scanning |
| `aes-gcm`, `sha2`, `hex` | byte-identical `*EHE*` crypto |
| `dotenvy` | `.env` loading (`SETTINGS_ENV_FILE` override) |
| `url` | `get_url` |
| `chrono` | timestamp formatting (`2006-01-02 15:04:05.000` → `%Y-%m-%d %H:%M:%S%.3f`, RFC3339 start time) |
| `serde`, `serde_json` | advertising payload, app payload functions |
| `ureq` | advertising HTTP POST (500 ms timeout) |
| `tiny_http` | stats web server |
| `rand` | GCM nonces |
| dev: `serial_test`, `tempfile` | env-mutating tests, fixture dirs |

ANSI colours are written directly (no dependency). Thousands separators are a small local function.

## Component design

### Config (`config.rs`)

- `config() -> &'static Configuration` — `LazyLock` global, initialised once (Go `sync.Once` equivalent). `config_for_context(ctx) -> &'static Configuration` returns/creates cached alternative-context copies sharing the loaded conf map.
- Initialisation order:
  1. Load `.env` via dotenvy; path from `SETTINGS_ENV_FILE` (default `.env`); warn on failure.
  2. `SETTINGS_CONTEXT` env (default `"dev"`), `SETTINGS_APPLICATION` env (optional).
  3. Load files into one map, later files overwrite (with an INFO log naming file:line, old and new values): `settings.conf` (warn if missing, fatal on other read errors), `settings_test.conf` (silent if missing, INFO log when loaded), `settings_local.conf` (warn if missing).
- **File discovery** (identical to Go): resolve from the executable's directory, then repeatedly go up one parent until found or filesystem root; if still not found, repeat the same walk from the current working directory.
- **Line parsing** (identical): strip `#` comments, split on first `=`, trim key and value, strip one pair of surrounding double quotes when value length > 2.
- **Lookup** (`get`): env var wins (with interpolation + decryption); else `findValue` walks `key.<context>` → parents → bare `key`, at each step preferring `<candidate>.<app>` when an application is set; else default; `${var}` interpolation loops until no tokens remain (unknown keys → `{UNKNOWN}`); `*EHE*` values transparently decrypted; `*EHE*` prefix stripped from returned values; every `get` records into the `requests` map (for `requested()`).
- API:
  - `get(&self, key) -> Option<String>`; `get_or(&self, key, default) -> String`
  - `get_parsed<T: FromStr>(&self, key) -> Result<Option<T>, ConfigError>` (ints of all widths, floats — replaces Go's 24 getter variants)
  - `get_bool(&self, key, default: bool) -> bool` (unparseable → false, like Go)
  - `get_duration(&self, key) -> Result<Option<Duration>, ConfigError>` — **Go duration syntax** (`ns`, `us`/`µs`, `ms`, `s`, `m`, `h`; e.g. `1h30m`, `500ms`) via a hand-written parser
  - `get_url(&self, key) -> Result<Option<Url>, ConfigError>` — decrypts embedded `*EHE*` tokens before parsing
  - `get_multi(&self, key, sep) -> Option<Vec<String>>` (items trimmed)
  - `set(&self, key, value) -> Option<String>` / `unset(&self, key) -> Option<String>` — return old value, notify listeners
  - `add_listener(&self, f: Box<dyn Fn(&str, &str) + Send + Sync>) -> ListenerId`; `remove_listener(&self, id)` (IDs instead of Go's interface-pointer comparison)
  - `get_all() -> HashMap<String, String>` (env overlay applied, `_SETTINGS_CONTEXT` included)
  - `stats() -> String` (CMDLINE, SETTINGS_ENV, resolved SETTINGS with `[.context]` annotations, `*EHE*` masked as `********************`)
  - `requested() -> String` (sorted `key=value` lines of everything asked for)
  - `context() -> &str`
- **Advertising beacon**: if `advertisingURL` resolves non-empty, spawn a thread that sleeps 1 s then POSTs the JSON payload every `advertisingInterval` (Go-duration, default `1m`): `executable, serviceName, loggers (registry names), version, commit, context, application, settingsFile, testSettingsFile, localSettingsFile, host, address, startTime (RFC3339 UTC), appPayload` (from registered `add_app_payload_fn(key, fn() -> serde_json::Value)` functions). 500 ms HTTP timeout; failures logged as warnings, loop continues.
- Package info globals in `lib.rs`: `set_info(name, version, commit)`, `set_address(addr)`, getters defaulting to `"Unknown"` — `RwLock<Option<String>>` statics.

### Logger (`logger.rs`)

- `Logger::new(package_name) -> &'static Logger` — global registry (`RwLock<HashMap<String, &'static Logger>>`, leaked allocations mirror Go's process-lifetime loggers); same name → same instance. Optional explicit level: `Logger::with_level(name, Level)`.
- Level from `logLevel` setting (default `INFO`); `Level` enum DEBUG/INFO/WARN/ERROR/FATAL/PANIC with `FromStr`/`Display`.
- Methods (all `#[track_caller]`): `debug/info/warn/error/fatal/panic(args: impl Display)` and `debugf!`-style via plain methods taking `String` (callers use `format!`); `error_with_stack` appends `std::backtrace::Backtrace`.
- Output line: `outputFormat` from `logger_output_format` (default `| %-20s| %-5s| %s |`) applied by a tiny printf-subset formatter (`%s`, `%-Ns`); fields: `file:line`, package name, 5-char-padded level (ANSI-coloured: blue/green/yellow/red/cyan/magenta when colour enabled). Timestamps (`logger_show_timestamps`, default true) in UTC `YYYY-MM-DD HH:MM:SS.mmm` on the trace/sample copy; stdout copy via a std logger prefix.
- Level filtering identical to Go: a line below the level still fans out to trace sockets and samplers (Go's `loggingNecessary` semantics), and trace matching tests the regex against both the rendered line and the lowercased level name.
- `fatal*`: log (cyan), close socket listener, `exit(1)`. `panic*`: log (magenta), close socket, `panic!`.
- On construction the logger starts its socket listener (below); failure to bind is fatal, as in Go.

### Socket REPL (`socket.rs`) + `rcli`

- `UnixListener` bound at `{socketDIR}/{PACKAGENAME_UPPER}.sock` (`socketDIR` setting, default `/tmp/gocore`); pre-existing socket file removed; dir created with permissive mode; "Socket created. Connect with: nc -U …" logged when `logger_show_socket_info` (default true). Listener thread accepts; one thread per connection; socket file removed on listener drop.
- Same protocol: welcome banner, prompt `rustcore::{name}> `, commands `config show|requested|get <k>|set <k> <v>|set k=v|unset <k>`, `loglevel <LEVEL>`, `trace on|off|clear`, `sample on <regex>|off|clear`, `status`, `help`, `quit`/`exit`. Argument splitting via `split_args` (quote-aware). Identical response texts.
- Trace registry: per-logger map of connection → regex string; writes that fail remove the connection.
- `rcli` binary: flags `-socketDir` (default `/tmp/gocore`), `-packageName`, `-keepAlive`; auto-selects the only `*.sock` if one exists, errors otherwise; sends the command (quoting multi-word args), then `quit` (or pipes stdin when `-keepAlive`), streaming responses to stdout. Plain `std::env::args` parsing (flag-style, Go-compatible invocations).

### Stats (`stats.rs`)

- `Stat` tree: `root_stat() -> &'static Stat` (key `"root"`, `ignore_child_updates = true`); `new_stat(key)` / `stat.new_stat(key)` get-or-create children (`RwLock<HashMap<String, Arc<Stat>>>`). Optional bool option = `ignore_child_updates`, as in Go.
- Aggregates under a `Mutex`: first/last/min/max/total durations, count, first/last times. `process_time` updates self then walks parents unless the parent ignores child updates. Durations above `gocore_stats_reported_time_threshold` (Go-duration setting, default `5m`) are logged and discarded.
- `add_time(start: SystemTime) -> SystemTime`: computes duration, **enqueues** to a global `mpsc::Sender`; a single consumer thread applies `process_time`. (Replaces Go's hand-rolled lock-free queue with identical externally-visible behaviour: non-blocking producers, single consumer.) Negative durations logged and dropped.
- `add_ranges(&self, ranges: &[i64])` creates bucket children (`"1,000 - 5,000"`, `"5,000 -"`); `add_time_for_range(start, sample_size)` routes synchronously to the matching bucket (as Go does); no-match logged.
- `hide_total(bool)`, `reset()` (recursive).
- HTTP: `start_stats_server(addr)` — `tiny_http` server on a thread; routes `{prefix}stats` (HTML page), `{prefix}reset` (reset + 303 redirect), `{prefix}css/*`, `{prefix}js/*` and anything else from embedded assets (`include_bytes!`), 404 fallback mirroring Go's. `stats_prefix` setting (default `/`), normalised to leading+trailing `/`. The HTML page is byte-equivalent in structure to Go's (same tablesorter setup, columns: item/count/average/first/last/min/max/total/first run/last run, drill-down links via `?key=a,b`, reset button, server-started header with `human_time_unit_html`, thousands separators).
- For embedding in other servers: `render_stats_page(keys_param: &str) -> String` and `serve_embedded_asset(path) -> Option<(&'static str, &'static [u8])>` are public — covers gocore's `RegisterStatsHandlers(mux...)` use case in a framework-agnostic way.

### Sampler (`sampler.rs`)

- `Sampler::new(id, filename, regex) -> Result<Sampler>`: creates file, spawns writer thread consuming an `mpsc::Receiver`; `write(&self, s)` is non-blocking-safe after stop (send error ignored); `stop()` closes the channel (thread closes file); `Display` matches Go's description strings. Attached to loggers via the REPL `sample` command and `logger.add_sampler`.

### Utils (`utils/`)

- `human_time(Duration) -> String` — pluralised `"2 days 3 hours 4 minutes 5 seconds"`, `"0 seconds"` floor; same tier logic as Go.
- `human_time_unit_with_colour(Duration) -> (String, &'static str)`, `human_time_unit`, `human_time_unit_html` — same thresholds (`d/h` red, `m` orange, `s` blue, `ms` green, `µs` black, `ns` grey) and `%.2f` formatting.
- `secure_settings::{encrypt(plaintext) -> Result<String>, decrypt_setting(&str) -> Result<String>}` — key = SHA-256 of the identical hardcoded phrase from the Go source; AES-256-GCM; nonce ‖ ciphertext, hex-encoded, `*EHE*` prefix; `decrypt_setting` passes non-`*EHE*` strings through and returns `*EHE*` + plaintext on success (matching Go, verified against the ciphertext fixture in `SecureSettings_test.go`).
- `split_args(&str) -> Result<Vec<String>>` — space-delimited, double-quote-aware (CSV-with-space semantics matching Go's `encoding/csv`).
- `get_outbound_ip() -> io::Result<IpAddr>` — UDP connect to `8.8.8.8:80`, read local addr.
- `is_regex_match(pattern, text) -> bool` — false on invalid pattern.

### rustcore-format binary

Functional port of `cmd/gocore-format`: parses a settings file into `Setting{key, group, comments, variants[]}` (variant = optionally-commented `key[.context]=value` with trailing comment), groups by base key, sorts groups and variants, aligns `=` within groups, preserves comment blocks, writes to stdout or back via `-w` (tmp file + rename). Ported tests included.

## Error handling

- `ConfigError` enum (`thiserror`-style manual impl, no extra dep): `Parse { key, value, source }`, `InvalidDuration(String)`, `InvalidUrl(...)`, `MissingValue(key)`, `Crypto(String)`, `Io(...)`.
- Library never panics outside `panic*` log methods and poisoned-lock unwraps. Missing settings files warn; unreadable settings files abort at first `config()` (Go parity).

## Testing

- Port all Go tests: config precedence, context fallback (incl. `live.es` → `live`, `stage.eu.red` → bare), `${var}` interpolation incl. nested (`varcity`), app-specific keys, set/unset/listeners, `requested`, `stats` masking; logger level parsing and registry; socket REPL session (connect over a temp socket, drive `config get/set`, `loglevel`, `status`, `quit`); stat tree aggregation, ranges, reset, threshold discard; `human_time*` table tests (Go's loop over 15 magnitudes); `split_args`; **EHE round-trip plus the exact Go ciphertext fixture** (`*EHE*8f7d…` → `*EHE*42`) proving cross-language compatibility; `rustcore-format` golden tests.
- Integration test loading a fixture copy of gocore's own `settings.conf`/`settings_local.conf` asserting identical resolved values.
- Env-mutating tests serialised with `serial_test`.
- CI entry point: `cargo test` (full suite), `cargo clippy`, `cargo fmt --check` via a `Makefile` mirroring gocore's.

## Compatibility notes / accepted deviations

- Logger formatting strings, file:line, level names, colours: identical. `#[track_caller]` yields the immediate caller, matching Go's `runtime.Caller(2)` intent.
- Prompt says `rustcore::name>` instead of `gocore::name>` (deliberate, cosmetic).
- Go's per-width `GetInt8/…/TryGetUint64` family is replaced by generic `get_parsed<T>`; semantics (default handling, found flag → `Option`, parse error → `Err`) preserved.
- Listener removal is by `ListenerId` token rather than pointer equality.
- The stats HTML page references the same CDN jQuery/tablesorter URLs and embedded chili/statistics.css assets.
