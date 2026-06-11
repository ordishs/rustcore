use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::config::config;
use crate::stdlog;
use crate::utils::human_time_unit_html;

pub(crate) static INIT_TIME: LazyLock<DateTime<Utc>> = LazyLock::new(Utc::now);

static REPORTED_TIME_THRESHOLD: LazyLock<(String, Duration)> = LazyLock::new(|| {
    let s = config().get_or("gocore_stats_reported_time_threshold", "5m");
    let d = crate::utils::parse_go_duration(&s).unwrap_or(Duration::from_secs(300));
    (s, d)
});

pub(crate) static STAT_PREFIX: LazyLock<String> = LazyLock::new(|| {
    let mut p = config().get_or("stats_prefix", "/");
    if !p.starts_with('/') {
        p = format!("/{p}");
    }
    if !p.ends_with('/') {
        p.push('/');
    }
    p
});

pub fn get_stat_prefix() -> String {
    STAT_PREFIX.clone()
}

pub fn current_time() -> DateTime<Utc> {
    Utc::now()
}

#[derive(Default, Clone)]
pub struct Snapshot {
    pub first: Duration,
    pub last: Duration,
    pub min: Duration,
    pub max: Duration,
    pub total: Duration,
    pub count: i64,
    pub first_time: Option<DateTime<Utc>>,
    pub last_time: Option<DateTime<Utc>>,
}

pub struct Stat {
    key: String,
    parent: Option<Arc<Stat>>,
    children: RwLock<HashMap<String, Arc<Stat>>>,
    range_lower: i64,
    range_upper: i64,
    ignore_child_updates: bool,
    hide_total: AtomicBool,
    agg: Mutex<Snapshot>,
}

struct StatItem {
    stat: Arc<Stat>,
    now: DateTime<Utc>,
    duration: Duration,
}

static QUEUE: LazyLock<Sender<StatItem>> = LazyLock::new(|| {
    let (tx, rx) = channel::<StatItem>();
    std::thread::spawn(move || {
        for item in rx {
            item.stat.process_time(item.now, item.duration);
        }
    });
    tx
});

static ROOT: LazyLock<Arc<Stat>> = LazyLock::new(|| {
    Arc::new(Stat {
        key: "root".to_string(),
        parent: None,
        children: RwLock::new(HashMap::new()),
        range_lower: 0,
        range_upper: 0,
        ignore_child_updates: true,
        hide_total: AtomicBool::new(false),
        agg: Mutex::new(Snapshot::default()),
    })
});

pub fn root_stat() -> Arc<Stat> {
    ROOT.clone()
}

pub fn new_stat(key: &str) -> Arc<Stat> {
    ROOT.new_stat(key, false)
}

impl Stat {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn new_stat(self: &Arc<Self>, key: &str, ignore_child_updates: bool) -> Arc<Stat> {
        {
            let children = self.children.read().unwrap();
            if let Some(c) = children.get(key) {
                return c.clone();
            }
        }
        let mut children = self.children.write().unwrap();
        if let Some(c) = children.get(key) {
            return c.clone();
        }
        let child = Arc::new(Stat {
            key: key.to_string(),
            parent: Some(self.clone()),
            children: RwLock::new(HashMap::new()),
            range_lower: 0,
            range_upper: 0,
            ignore_child_updates,
            hide_total: AtomicBool::new(false),
            agg: Mutex::new(Snapshot::default()),
        });
        children.insert(key.to_string(), child.clone());
        child
    }

    pub fn add_ranges(self: &Arc<Self>, ranges: &[i64]) -> Arc<Stat> {
        let mut ranges = ranges.to_vec();
        ranges.sort_unstable();
        let mut children = self.children.write().unwrap();
        for i in 0..ranges.len() {
            let (key, lower, upper) = if i == ranges.len() - 1 {
                (
                    format!("{} -", add_thousands_operator_trim(ranges[i])),
                    ranges[i],
                    -1i64,
                )
            } else {
                (
                    format!(
                        "{} - {}",
                        add_thousands_operator_trim(ranges[i]),
                        add_thousands_operator_trim(ranges[i + 1])
                    ),
                    ranges[i],
                    ranges[i + 1],
                )
            };
            children.entry(key.clone()).or_insert_with(|| {
                Arc::new(Stat {
                    key,
                    parent: Some(self.clone()),
                    children: RwLock::new(HashMap::new()),
                    range_lower: lower,
                    range_upper: upper,
                    ignore_child_updates: false,
                    hide_total: AtomicBool::new(false),
                    agg: Mutex::new(Snapshot::default()),
                })
            });
        }
        drop(children);
        self.clone()
    }

    pub fn hide_total(&self, b: bool) {
        self.hide_total.store(b, Ordering::Relaxed);
    }

    pub(crate) fn is_total_hidden(&self) -> bool {
        self.hide_total.load(Ordering::Relaxed)
    }

    pub fn get_child(&self, key: &str) -> Option<Arc<Stat>> {
        self.children.read().unwrap().get(key).cloned()
    }

    pub fn child_keys(&self) -> Vec<String> {
        self.children.read().unwrap().keys().cloned().collect()
    }

    pub(crate) fn has_children(&self) -> bool {
        !self.children.read().unwrap().is_empty()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.agg.lock().unwrap().clone()
    }

    fn process_time(self: &Arc<Self>, now: DateTime<Utc>, duration: Duration) {
        let (threshold_str, threshold) = &*REPORTED_TIME_THRESHOLD;
        if duration > *threshold {
            stdlog(&format!(
                "Stat: time for {} is greater than {}",
                self.key, threshold_str
            ));
            return;
        }
        {
            let mut agg = self.agg.lock().unwrap();
            agg.last_time = Some(now);
            agg.last = duration;
            if agg.count == 0 {
                agg.first_time = Some(now);
                agg.first = duration;
                agg.min = duration;
                agg.max = duration;
            } else {
                if duration < agg.min {
                    agg.min = duration;
                }
                if duration > agg.max {
                    agg.max = duration;
                }
            }
            agg.total += duration;
            agg.count += 1;
        }
        if let Some(parent) = &self.parent {
            if !parent.ignore_child_updates {
                parent.process_time(now, duration);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn process_time_direct(self: &Arc<Self>, now: DateTime<Utc>, d: Duration) {
        self.process_time(now, d);
    }

    pub fn add_time(self: &Arc<Self>, start: DateTime<Utc>) -> DateTime<Utc> {
        let now = Utc::now();
        let Ok(duration) = (now - start).to_std() else {
            stdlog(&format!("{}: startTime is in the future", self.key));
            return now;
        };
        let _ = QUEUE.send(StatItem {
            stat: self.clone(),
            now,
            duration,
        });
        now
    }

    pub fn add_time_for_range(
        self: &Arc<Self>,
        start: DateTime<Utc>,
        sample_size: i64,
    ) -> DateTime<Utc> {
        let now = Utc::now();
        let Ok(duration) = (now - start).to_std() else {
            stdlog(&format!("{}: startTime is in the future", self.key));
            return now;
        };
        let children = self.children.read().unwrap();
        let mut found = false;
        for child in children.values() {
            if child.range_lower <= sample_size
                && (child.range_upper == -1 || sample_size < child.range_upper)
            {
                child.process_time(now, duration);
                found = true;
                break;
            }
        }
        if !found {
            stdlog(&format!(
                "{}: sampleSize {} does not fit into any range",
                self.key, sample_size
            ));
        }
        now
    }

    pub fn reset(&self) {
        *self.agg.lock().unwrap() = Snapshot::default();
        for child in self.children.read().unwrap().values() {
            child.reset();
        }
    }
}

pub(crate) fn average(total: Duration, count: i64) -> Duration {
    if count == 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos((total.as_nanos() / count as u128) as u64)
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn add_thousands_operator(num: i64) -> String {
    format!("{}\n", add_thousands_operator_trim(num))
}

pub(crate) fn add_thousands_operator_trim(num: i64) -> String {
    let s = num.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if num < 0 {
        format!("-{out}")
    } else {
        out
    }
}

static CSS: &[u8] = include_bytes!("../embed/css/statistics.css");
static JS: &[u8] = include_bytes!("../embed/js/chili-1.8b.js");

pub fn serve_embedded_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    match path {
        "css/statistics.css" => Some(("text/css", CSS)),
        "js/chili-1.8b.js" => Some(("text/javascript", JS)),
        _ => None,
    }
}

pub fn render_stats_page(keys_param: &str) -> String {
    let prefix = &*STAT_PREFIX;
    let mut p = String::new();

    p.push_str(&format!(
        r#"
	<html>
		<head>
			<title>
			RustCore Statistics
			</title>
			<script src="https://cdnjs.cloudflare.com/ajax/libs/jquery/1.4.3/jquery.min.js" integrity="sha512-xqRHwg8Pg0JQ+nne5mBy3SGrGDihpsr5UYuMgIcVj1SMfSKrRJNvu7tFitaK70xDpSsBBIVpTcTGXnmx7/Q2xw==" crossorigin="anonymous" referrerpolicy="no-referrer"></script>
			<script src="https://cdnjs.cloudflare.com/ajax/libs/jquery.tablesorter/2.31.3/js/jquery.tablesorter.min.js" integrity="sha512-qzgd5cYSZcosqpzpn7zF2ZId8f/8CHmFKZ8j7mU4OUXTNRd5g+ZHBPsgKEwoqxCtdQvExE5LprwwPAgoicguNg==" crossorigin="anonymous" referrerpolicy="no-referrer"></script>
			<script src="https://cdnjs.cloudflare.com/ajax/libs/jquery.tablesorter/2.31.3/js/jquery.tablesorter.widgets.min.js" integrity="sha512-dj/9K5GRIEZu+Igm9tC16XPOTz0RdPk9FGxfZxShWf65JJNU2TjbElGjuOo3EhwAJRPhJxwEJ5b+/Ouo+VqZdQ==" crossorigin="anonymous" referrerpolicy="no-referrer"></script>
			<script type='text/javascript' src='{prefix}js/chili-1.8b.js'></script>
			<link rel='stylesheet' href='{prefix}css/statistics.css' type='text/css' media='print, projection, screen' />

			<script type='text/javascript'>

				function convertToNanoseconds(duration) {{
					const timeUnits = {{
						d: 24 * 60 * 60 * 1e9,
						h: 60 * 60 * 1e9,
						m: 60 * 1e9,
						s: 1e9,
						ms: 1e6,
						µs: 1e3,
						ns: 1
					}};

					const regex = /(\d+(\.\d+)?)(d|h|ms|ns|m|µs|s)/g;

					let totalNanoseconds = 0;

					const matches = duration.matchAll(regex);

					for (const match of matches) {{
							const value = parseFloat(match[1]);
							const timeUnit = match[3];
							totalNanoseconds += value * (timeUnits[timeUnit] || 0);
					}}

					return totalNanoseconds;
				}}

				$(document).ready(function() {{
					$.tablesorter.addParser({{
						id: 'timings',
						is: function(s) {{
							return false;
						}},
						format: function(s) {{
							return convertToNanoseconds(s);
						}},
						type: 'numeric'
					}});

					$('#myTable').tablesorter({{
						sortList: [[1,1]],
						debug: false,
						widgets: ['zebra', 'saveSort'],
						headers: {{
							0: {{sorter: 'text'}},
							1: {{sorter: 'number'}},
							2: {{sorter: 'timings'}},
							3: {{sorter: 'timings'}},
							4: {{sorter: 'timings'}},
							5: {{sorter: 'timings'}},
							6: {{sorter: 'timings'}},
							7: {{sorter: 'timings'}},
							8: {{sorter: 'usLongDate'}},
							9: {{sorter: 'usLongDate'}}
						}},
						widgetOptions: {{
							saveSort: true
						}}
					}});
				}})
				</script>
			</head>
	"#
    ));

    p.push_str("<body>\r\n");
    p.push_str("<table width='100%'>\r\n<tr>\r\n");
    p.push_str("<td style='vertical-align:middle;width:50%'>\r\n<h1>\r\nRustCore Statistics\r\n</h1>\r\n</td>\r\n");
    p.push_str("<td align='right' style='vertical-align:middle;width:50%' >\r\n");
    p.push_str(&format!(
        "<form border='0' cellpadding='0' action='{prefix}reset' method='get'>\r\n"
    ));
    p.push_str(&format!(
        "<input type='button' value='Reset Statistics' onClick='window.location.replace(\"reset?key={}\");'>\r\n",
        html_escape(keys_param)
    ));
    p.push_str("</form>\r\n</td>\r\n</tr>\r\n</table>\r\n");

    p.push_str("<table id='myTable' class='tablesorter' border='0' cellpadding='0' cellspacing='1'>\r\n<thead>\r\n<tr>\r\n");
    for th in [
        "<th>Item</th>",
        "<th align='right'>count</th>",
        "<th align='right'>average</th>",
        "<th align='right'>first</th>",
        "<th align='right'>last</th>",
        "<th align='right'>min</th>",
        "<th align='right'>max</th>",
        "<th align='right'>total</th>",
        "<th>first run</th>",
        "<th>last run</th>",
    ] {
        p.push_str(th);
        p.push_str("\r\n");
    }
    p.push_str("</tr>\r\n</thead>\r\n<tbody>\r\n");

    let mut item = root_stat();
    let keys: Vec<&str> = if keys_param.is_empty() {
        Vec::new()
    } else {
        keys_param.split(',').collect()
    };
    let keys_param_link = if keys_param.is_empty() {
        String::new()
    } else {
        format!("{keys_param},")
    };
    for key in keys {
        match item.get_child(key) {
            Some(child) => item = child,
            None => return p,
        }
    }

    let now = Utc::now();
    let uptime = (now - *INIT_TIME).to_std().unwrap_or_default();
    p.push_str(&format!(
        "<h2>Server started: {} [{} ago]</h2>\r\n",
        INIT_TIME.format("%Y-%m-%d %H:%M:%S%.3f"),
        human_time_unit_html(uptime)
    ));

    let children = item.children.read().unwrap();
    let mut child_keys: Vec<&String> = children.keys().collect();
    child_keys.sort();
    for key in child_keys {
        let child = &children[key];
        let snap = child.snapshot();
        p.push_str("<tr>\r\n");
        let escaped_key = html_escape(key);
        if child.has_children() {
            p.push_str(&format!(
                "<td><a href='{prefix}stats?key={}{escaped_key}'>{escaped_key}</a></td>\r\n",
                html_escape(&keys_param_link)
            ));
        } else {
            p.push_str(&format!("<td>{escaped_key}</td>\r\n"));
        }
        p.push_str(&format!(
            "<td align='right'>{}</td>\r\n",
            add_thousands_operator(snap.count)
        ));
        for d in [
            average(snap.total, snap.count),
            snap.first,
            snap.last,
            snap.min,
            snap.max,
        ] {
            p.push_str(&format!(
                "<td align='right'>{}</td>\r\n",
                human_time_unit_html(d)
            ));
        }
        if child.is_total_hidden() {
            p.push_str("<td></td>\r\n");
        } else {
            p.push_str(&format!(
                "<td align='right'>{}</td>\r\n",
                human_time_unit_html(snap.total)
            ));
        }
        for t in [snap.first_time, snap.last_time] {
            let formatted = t
                .map(|t| t.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
                .unwrap_or_else(|| "0001-01-01 00:00:00.000".to_string());
            p.push_str(&format!("<td>{formatted}</td>\r\n"));
        }
        p.push_str("</tr>\r\n");
    }
    drop(children);

    p.push_str("</tbody>\r\n</table>\r\n");
    p.push_str(&format!(
        "<p>Report time: {}</p>\r\n",
        now.format("%Y-%m-%d %H:%M:%S%.3f")
    ));
    p.push_str("<div align='right'><form>\r\n\r\n");
    p.push_str("<input type='button' value='  Back  ' onClick='history.go(-1)'>\r\n");
    p.push_str("</form>\r\n</div>\r\n</body></html>\r\n");

    p
}

pub fn reset_stats(keys_param: &str) {
    let mut item = root_stat();
    if !keys_param.is_empty() {
        for key in keys_param.split(',') {
            match item.get_child(key) {
                Some(c) => item = c,
                None => return,
            }
        }
    }
    item.reset();
}

pub fn start_stats_server(addr: &str) {
    let logger = crate::logger::log("stats");
    let prefix = &*STAT_PREFIX;
    let server = match tiny_http::Server::http(addr) {
        Ok(s) => s,
        Err(e) => logger.panic(format!("Server failed starting. Error: {e}")),
    };
    logger.info(format!(
        "Starting StatsServer on http://{addr}{prefix}stats"
    ));

    for request in server.incoming_requests() {
        handle_request(request);
    }
}

fn handle_request(request: tiny_http::Request) {
    let prefix = &*STAT_PREFIX;
    let url = request.url().to_string();
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (url, String::new()),
    };
    let keys_param = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("key="))
        .unwrap_or("")
        .to_string();

    let html = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap();

    if path == format!("{prefix}stats") {
        let body = render_stats_page(&keys_param);
        let _ = request.respond(tiny_http::Response::from_string(body).with_header(html));
    } else if path == format!("{prefix}reset") {
        reset_stats(&keys_param);
        let location =
            tiny_http::Header::from_bytes(&b"Location"[..], format!("{prefix}stats").as_bytes())
                .unwrap();
        let _ = request.respond(tiny_http::Response::empty(303).with_header(location));
    } else {
        let trimmed = path.strip_prefix(prefix.as_str()).unwrap_or(&path);
        match serve_embedded_asset(trimmed) {
            Some((ct, body)) => {
                let header =
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], ct.as_bytes()).unwrap();
                let _ = request.respond(tiny_http::Response::from_data(body).with_header(header));
            }
            None => {
                let _ = request
                    .respond(tiny_http::Response::from_string("Not found").with_status_code(404));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn tree_and_aggregation() {
        let root = root_stat();
        let s = root.new_stat("test-agg", false);
        let same = root.new_stat("test-agg", false);
        assert!(std::sync::Arc::ptr_eq(&s, &same));

        s.process_time_direct(chrono::Utc::now(), Duration::from_millis(10));
        s.process_time_direct(chrono::Utc::now(), Duration::from_millis(30));
        s.process_time_direct(chrono::Utc::now(), Duration::from_millis(20));

        let snap = s.snapshot();
        assert_eq!(snap.count, 3);
        assert_eq!(snap.min, Duration::from_millis(10));
        assert_eq!(snap.max, Duration::from_millis(30));
        assert_eq!(snap.first, Duration::from_millis(10));
        assert_eq!(snap.last, Duration::from_millis(20));
        assert_eq!(snap.total, Duration::from_millis(60));

        s.reset();
        assert_eq!(s.snapshot().count, 0);
    }

    #[test]
    fn add_time_via_queue() {
        let s = root_stat().new_stat("test-queue", false);
        let start = current_time() - chrono::Duration::milliseconds(5);
        s.add_time(start);
        for _ in 0..100 {
            if s.snapshot().count == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(s.snapshot().count, 1);
    }

    #[test]
    fn ranges() {
        let s = root_stat().new_stat("test-ranges", true);
        s.add_ranges(&[1000, 5000]);
        let start = current_time() - chrono::Duration::milliseconds(1);
        s.add_time_for_range(start, 2500);
        s.add_time_for_range(start, 9999);
        let children = s.child_keys();
        assert!(children.contains(&"1,000 - 5,000".to_string()));
        assert!(children.contains(&"5,000 -".to_string()));
        let bucket = s.get_child("1,000 - 5,000").unwrap();
        assert_eq!(bucket.snapshot().count, 1);
    }

    #[test]
    fn thousands() {
        assert_eq!(add_thousands_operator_trim(1234567), "1,234,567");
        assert_eq!(add_thousands_operator_trim(999), "999");
    }

    #[test]
    fn render_escapes_key_param() {
        let page = render_stats_page("\"><script>");
        assert!(!page.contains("<script>"));
    }
}
