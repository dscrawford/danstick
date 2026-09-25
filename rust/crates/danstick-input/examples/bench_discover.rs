//! What a scan costs, and where.
use danstick_input::pad::{discover, Filter};
use std::time::{Duration, Instant};

fn best(label: &str, mut f: impl FnMut()) {
    f();
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed());
    }
    println!("{label:44} {:6.1} ms", best.as_secs_f64() * 1000.0);
}

fn main() {
    let plain = Filter {
        include_undriven: false,
        ..Filter::default()
    };
    best("discover(), no Steam Controller probe", || {
        std::hint::black_box(discover(plain).expect("discovery"));
    });
    best("discover(), default", || {
        std::hint::black_box(discover(Filter::default()).expect("discovery"));
    });

    let nodes: Vec<std::path::PathBuf> = std::fs::read_dir("/dev/input")
        .expect("/dev/input")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().contains("/event"))
        .collect();
    println!("\n  over {} event nodes:", nodes.len());
    best("  reading two sysfs capability files each", || {
        for node in &nodes {
            let caps = std::path::Path::new("/sys/class/input")
                .join(node.file_name().expect("a device node has a name"))
                .join("device/capabilities");
            for leaf in ["abs", "key"] {
                std::hint::black_box(std::fs::read_to_string(caps.join(leaf)).ok());
            }
        }
    });
    best("  opening each instead (what this replaced)", || {
        for node in &nodes {
            std::hint::black_box(evdev::Device::open(node).ok());
        }
    });
}
