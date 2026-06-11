#[test]
fn global_config_loads_fixture_files() {
    unsafe {
        std::env::set_var("SETTINGS_CONTEXT", "live");
    }
    std::env::set_current_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
    )
    .unwrap();

    let c = rustcore::config::config();
    assert_eq!(c.context(), "live");
    assert_eq!(c.get("url").unwrap(), "https://www.server.com");
    assert_eq!(c.get("local").unwrap(), "overridden"); // settings_local.conf wins
    assert_eq!(c.get("name").unwrap(), "Simon");

    // env beats files
    unsafe {
        std::env::set_var("name", "FromEnv");
    }
    assert_eq!(c.get("name").unwrap(), "FromEnv");
    unsafe {
        std::env::remove_var("name");
    }

    // alternative context
    let dev = rustcore::config::config_for_context("dev");
    assert_eq!(dev.get("url").unwrap(), "http://localhost:8080");
}
