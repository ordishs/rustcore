fn main() {
    let stat = rustcore::new_stat("example-op");
    std::thread::spawn(move || loop {
        let start = rustcore::current_time();
        std::thread::sleep(std::time::Duration::from_millis(50));
        stat.add_time(start);
    });
    rustcore::start_stats_server("localhost:9009");
}
