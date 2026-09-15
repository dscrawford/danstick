//! Parse a MAME -listxml dump and print the table, for comparing against the
//! Python. `cargo run --release --example dump_mame -- path/to/MAME.xml`
fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_mame <xml>");
    let text = std::fs::read_to_string(&path).expect("read the dump");
    let started = std::time::Instant::now();
    let table = padmap_core::titles::parse_mame_xml(&text);
    eprintln!("rust parse only: {} ms", started.elapsed().as_millis());
    println!("{}", padmap_core::titles::to_json(&table));
}
