fn main() {
    let logger = rustcore::log("example");
    logger.debug("debug message");
    logger.info(format!("info message with {}", 42));
    logger.warn("warn message");
    logger.error("error message");
    println!("Connect with: cargo run --bin rcli -- -packageName EXAMPLE -keepAlive trace on");
    std::thread::sleep(std::time::Duration::from_secs(60));
}
