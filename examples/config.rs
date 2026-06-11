fn main() {
    let c = rustcore::config::config();
    println!("STATS\n{}\n-------\n", c.stats());
    if let Ok(Some(url)) = c.get_url("url") {
        println!("URL is {url}");
    }
}
