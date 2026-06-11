#[test]
fn stats_server_serves_page_assets_and_reset() {
    let s = rustcore::new_stat("http-test");
    let start = rustcore::current_time() - chrono::Duration::milliseconds(3);
    s.add_time(start);

    std::thread::spawn(|| rustcore::start_stats_server("127.0.0.1:19888"));
    std::thread::sleep(std::time::Duration::from_millis(300));

    let page: String = ureq::get("http://127.0.0.1:19888/stats")
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(page.contains("RustCore Statistics"));
    assert!(page.contains("http-test"));

    let css = ureq::get("http://127.0.0.1:19888/css/statistics.css")
        .call()
        .unwrap();
    assert_eq!(css.header("Content-Type").unwrap(), "text/css");

    let resp = ureq::get("http://127.0.0.1:19888/reset").call().unwrap();
    assert!(resp.into_string().unwrap().contains("RustCore Statistics"));

    let notfound = ureq::get("http://127.0.0.1:19888/nope").call();
    assert!(matches!(notfound, Err(ureq::Error::Status(404, _))));
}
