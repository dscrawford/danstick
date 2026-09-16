//! The assignment journey through a real daemon, with real presses.
//!
//! A socket harness can call `begin`; nothing about it proves that holding a
//! button on a pad the daemon has grabbed claims a slot, that holding again
//! confirms, or that the files a launch reads are then on disk. This drives
//! the real `padmap-rs serve` with a uinput pad it creates and owns.
//!
//! Safe on a live machine, by construction: its own runtime, config and
//! profile directories; `PADMAP_ONLY_DEVICE` restricts discovery to the test
//! pad, so no real controller is grabbed; and the pad's signature is written
//! into the *live* daemon's `prompted` file first and removed afterwards --
//! creating a joystick node is not a neutral act while a daemon is watching
//! for unfamiliar controllers.
//!
//! Skips, rather than fails, where /dev/uinput is not writable.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode,
    UinputAbsSetup,
};
use serde_json::Value;

const PAD_VID: u16 = 0x1209;
const FIRST_KEY: u16 = 0x130;
const KEY_COUNT: u16 = 16;

/// Each test gets its own pad name and product id, and its own
/// `PADMAP_ONLY_DEVICE` token, so two running at once cannot see each other's
/// controller -- which they otherwise do, the daemon reporting "2 pad(s)".
#[derive(Clone, Copy)]
struct PadId {
    name: &'static str,
    pid: u16,
    only: &'static str,
}

const JOURNEY: PadId = PadId {
    name: "PADMAP RSTESTJOURNEY",
    pid: 0x0003,
    only: "RSTESTJOURNEY",
};
const HOSTILE: PadId = PadId {
    name: "PADMAP RSTESTHOSTILE",
    pid: 0x0004,
    only: "RSTESTHOSTILE",
};

fn signature(id: PadId) -> String {
    format!("{PAD_VID:04x}:{:04x}:{}", id.pid, id.name)
}

fn uinput_writable() -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok()
}

/// Tell the live daemon we have already been asked about this model.
struct LiveGuard {
    path: PathBuf,
    added: bool,
    signature: String,
}

impl LiveGuard {
    fn new(id: PadId) -> LiveGuard {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
        let path = Path::new(&base).join("padmap").join("prompted");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let mut added = false;
        if !existing.lines().any(|line| line.trim() == signature(id)) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&path, format!("{existing}{}\n", signature(id))).is_ok() {
                added = true;
            }
        }
        LiveGuard {
            path,
            added,
            signature: signature(id),
        }
    }
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        if !self.added {
            return;
        }
        let Ok(existing) = std::fs::read_to_string(&self.path) else {
            return;
        };
        let kept: String = existing
            .lines()
            .filter(|line| !line.trim().is_empty() && line.trim() != self.signature)
            .map(|line| format!("{line}\n"))
            .collect();
        let _ = std::fs::write(&self.path, kept);
    }
}

struct TestPad {
    device: VirtualDevice,
}

impl TestPad {
    fn new(id: PadId) -> TestPad {
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in FIRST_KEY..FIRST_KEY + KEY_COUNT {
            keys.insert(KeyCode::new(code));
        }
        let stick = AbsInfo::new(128, 0, 255, 0, 0, 0);
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        let device = VirtualDevice::builder()
            .expect("uinput")
            .name(id.name)
            .input_id(InputId::new(BusType::BUS_USB, PAD_VID, id.pid, 1))
            .with_keys(&keys)
            .expect("keys")
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, stick))
            .expect("x")
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, stick))
            .expect("y")
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0X, hat))
            .expect("hat x")
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0Y, hat))
            .expect("hat y")
            .build()
            .expect("a virtual pad");
        std::thread::sleep(Duration::from_millis(500));
        TestPad { device }
    }

    fn emit(&mut self, kind: u16, code: u16, value: i32) {
        let events = [
            InputEvent::new(kind, code, value),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ];
        self.device.emit(&events).expect("emit");
    }

    fn hold(&mut self, code: u16, seconds: f64) {
        self.emit(EventType::KEY.0, code, 1);
        std::thread::sleep(Duration::from_secs_f64(seconds));
        self.emit(EventType::KEY.0, code, 0);
    }

    fn tap(&mut self, code: u16) {
        self.hold(code, 0.1);
    }

    fn push_hat_right(&mut self) {
        self.emit(EventType::ABSOLUTE.0, 0x10, 1);
        std::thread::sleep(Duration::from_millis(150));
        self.emit(EventType::ABSOLUTE.0, 0x10, 0);
        std::thread::sleep(Duration::from_millis(150));
    }
}

struct Daemon {
    child: Child,
    sock: UnixStream,
    events: Vec<Value>,
    buffer: Vec<u8>,
    runtime: PathBuf,
    profiles: PathBuf,
}

impl Daemon {
    fn start(root: &Path, id: PadId) -> Daemon {
        let runtime = root.join("run");
        let config = root.join("config");
        let profiles = root.join("devices");
        for dir in [&runtime, &config, &profiles] {
            std::fs::create_dir_all(dir).expect("mkdir");
        }
        let child = Command::new(env!("CARGO_BIN_EXE_padmap-rs"))
            .arg("serve")
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_DATA_HOME", root.join("data"))
            .env("PADMAP_PROFILE_DIR", &profiles)
            .env("PADMAP_SDL_DB", root.join("sdl_controllers.txt"))
            .env("PADMAP_ONLY_DEVICE", id.only)
            .env("PADMAP_NO_AUTOSETUP", "1")
            .env("PADMAP_CEMU_DIR", root.join("cemu"))
            .env("PADMAP_ARES_SETTINGS", root.join("nowhere/ares.bml"))
            .env("PADMAP_RYUJINX_CONFIG", root.join("nowhere/Config.json"))
            .env("RUST_LOG", "info,padmap_daemon=debug")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn the daemon");
        let socket = runtime.join("padmap").join("padmap.sock");
        let mut sock = None;
        for _ in 0..50 {
            if let Ok(stream) = UnixStream::connect(&socket) {
                sock = Some(stream);
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let sock = sock.expect("the daemon never came up");
        sock.set_read_timeout(Some(Duration::from_millis(100)))
            .expect("timeout");
        let mut daemon = Daemon {
            child,
            sock,
            events: Vec::new(),
            buffer: Vec::new(),
            runtime,
            profiles,
        };
        daemon.pump(0.5);
        daemon
    }

    fn pump(&mut self, seconds: f64) {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        let mut chunk = [0u8; 65536];
        while Instant::now() < deadline {
            match self.sock.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => self.buffer.extend_from_slice(&chunk[..count]),
                Err(_) => continue,
            }
            while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                if let Ok(value) = serde_json::from_slice::<Value>(&line[..line.len() - 1]) {
                    self.events.push(value);
                }
            }
        }
    }

    fn send(&mut self, message: Value) {
        let mut bytes = serde_json::to_vec(&message).expect("json");
        bytes.push(b'\n');
        self.sock.write_all(&bytes).expect("send");
        self.pump(0.4);
    }

    fn last(&self, name: &str) -> Option<&Value> {
        self.events
            .iter()
            .rev()
            .find(|event| event["event"] == name)
    }

    fn wait_for(
        &mut self,
        name: &str,
        predicate: impl Fn(&Value) -> bool,
        seconds: f64,
    ) -> Option<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < deadline {
            self.pump(0.2);
            if let Some(found) = self
                .events
                .iter()
                .rev()
                .find(|e| e["event"] == name && predicate(e))
            {
                return Some(found.clone());
            }
        }
        None
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // SIGTERM, so the teardown path runs and the grabs are released.
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
    }
}

#[test]
fn a_session_claims_confirms_and_writes_what_a_launch_reads() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(JOURNEY);
    let root = std::env::temp_dir().join(format!("padmap-daemon-journey-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut pad = TestPad::new(JOURNEY);
    let mut daemon = Daemon::start(&root, JOURNEY);

    // The greeting.
    let state = daemon
        .last("state")
        .expect("a state event on connect")
        .clone();
    assert_eq!(state["state"], "idle");
    assert_eq!(state["identity"], "mirror");
    assert!(state["pid"].as_u64().is_some());

    // Opening a session sees the one pad.
    daemon.send(serde_json::json!({"cmd": "begin", "players": 2}));
    let pads = daemon.wait_for("pads", |_| true, 5.0).expect("pads");
    assert_eq!(pads["count"], 1);
    daemon
        .wait_for("state", |e| e["state"] == "assigning", 5.0)
        .expect("assigning");

    // A tap does not claim; a hold does.
    pad.tap(FIRST_KEY);
    daemon.pump(0.6);
    assert!(daemon.last("claim").is_none(), "a tap claimed a slot");
    pad.hold(FIRST_KEY, 0.6);
    let claim = daemon
        .wait_for("claim", |_| true, 5.0)
        .expect("a hold claims the first slot");
    assert_eq!(claim["player"], 1);
    assert_eq!(claim["configured"], false);
    assert_eq!(claim["name"], JOURNEY.name);

    // Holding again confirms: accepted, ready, and the files exist.
    pad.hold(FIRST_KEY + 1, 1.1);
    let accepted = daemon
        .wait_for("accepted", |_| true, 5.0)
        .expect("a second hold confirms");
    assert_eq!(accepted["players"][0]["player"], 1);
    let ready = daemon
        .wait_for("state", |e| e["state"] == "ready", 5.0)
        .expect("ready after accept");
    assert_eq!(ready["players"][0]["name"], JOURNEY.name);

    let state_dir = daemon.runtime.join("padmap");
    let assignments: Value = serde_json::from_str(
        &std::fs::read_to_string(state_dir.join("assignments.json")).expect("assignments"),
    )
    .expect("json");
    assert_eq!(assignments[0]["player"], 1);
    assert_eq!(assignments[0]["name"], JOURNEY.name);
    let launch = std::fs::read_to_string(state_dir.join("launch.cfg")).expect("launch.cfg");
    // The index arithmetic (and so which slots get `--nodevice`) is corpus
    // tested; PADMAP_ONLY_DEVICE hides the clone from the enumeration here.
    // This proves the wiring: the override is written for all sixteen slots.
    assert!(launch.contains("input_player16_joypad_index"), "{launch}");
    assert!(launch.contains("config_save_on_exit = \"false\""));
    // launch.args is written even when it is empty.
    assert!(state_dir.join("launch.args").is_file(), "no launch.args");
    let autoconfig = state_dir
        .join("autoconfig")
        .join("udev")
        .join("padmap Player 1.cfg");
    let profile = std::fs::read_to_string(&autoconfig).expect("an autoconfig profile");
    assert!(
        profile.contains("input_device = \"padmap Player 1\""),
        "{profile}"
    );
    let sdl = std::fs::read_to_string(root.join("sdl_controllers.txt")).expect("the SDL database");
    assert!(sdl.contains("padmap Player 1"), "{sdl}");
    let mapping = daemon.last("sdl_mapping").expect("sdl_mapping");
    assert_eq!(mapping["lines"].as_array().map(Vec::len), Some(1));

    // What a launcher sees, without connecting to the daemon at all: the
    // seated player, the node its clone is on, and the GUID a mapping is
    // filed under -- the same three facts the socket would have given it.
    let listed = Command::new(env!("CARGO_BIN_EXE_padmap-rs"))
        .args(["list", "--json"])
        .env("XDG_RUNTIME_DIR", &daemon.runtime)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("PADMAP_PROFILE_DIR", &daemon.profiles)
        .env("PADMAP_ONLY_DEVICE", JOURNEY.only)
        .output()
        .expect("list --json");
    assert!(listed.status.success(), "{listed:?}");
    let entries: Value = serde_json::from_slice(&listed.stdout).expect("valid JSON");
    let entries = entries.as_array().expect("an array");
    assert_eq!(entries.len(), 1, "{entries:?}");
    let seated = &entries[0];
    assert_eq!(seated["player"], 1);
    assert_eq!(seated["controller"]["name"], JOURNEY.name);
    assert_eq!(seated["controller"]["configured"], false, "not mapped yet");
    // The clone's node, which is the thing a launcher binds.
    let node = seated["virtual"]["node"]
        .as_str()
        .expect("the clone's node")
        .to_owned();
    assert!(node.starts_with("/dev/input/event"), "{node}");
    // ...and it is the GUID the SDL line was written under, so a mapping
    // registered from this output is one SDL will actually look up.
    let guid = seated["virtual"]["guid"].as_str().expect("guid");
    assert!(
        mapping["lines"][0]
            .as_str()
            .expect("line")
            .starts_with(guid),
        "list says {guid}, the SDL line says {}",
        mapping["lines"][0]
    );
    // This pad has no gyro, and says so rather than leaving the field out.
    assert_eq!(seated["controller"]["motion"], false);
    assert!(seated["controller"]["motion_node"].is_null());
    // A clone exists and is named for the player.
    let clones: BTreeSet<String> = std::fs::read_dir("/sys/class/input")
        .expect("sysfs")
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("name")).ok())
        .map(|name| name.trim().to_owned())
        .collect();
    assert!(
        clones.contains("padmap Player 1"),
        "no clone among {clones:?}"
    );

    // Choosing a console, then walking the wizard, from the pad.
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "begin", "players": 2}));
    daemon
        .wait_for("state", |e| e["state"] == "assigning", 5.0)
        .expect("assigning");
    pad.hold(FIRST_KEY, 0.6);
    let claim = daemon
        .wait_for("claim", |e| e["configured"] == false, 5.0)
        .expect("claimed again");
    assert_eq!(claim["player"], 1);
    daemon.send(serde_json::json!({"cmd": "choose_layout", "player": 1}));
    let choice = daemon.last("layout_choice").expect("the picker").clone();
    assert_eq!(choice["active"], true);
    assert_eq!(choice["kind"], "layout");
    let choices: Vec<String> = choice["choices"]
        .as_array()
        .expect("choices")
        .iter()
        .map(|c| c["id"].as_str().expect("id").to_owned())
        .collect();
    assert!(choices.contains(&"snes".to_owned()), "{choices:?}");
    let target = choices.iter().position(|id| id == "snes").expect("snes");
    let start = choice["index"].as_u64().expect("index") as usize;
    let steps = (target + choices.len() - start) % choices.len();
    for _ in 0..steps {
        pad.push_hat_right();
        daemon.pump(0.2);
    }
    assert_eq!(
        daemon.last("layout_choice").expect("moved")["chosen"],
        "snes"
    );
    // Holding accepts and opens the wizard on that layout.
    pad.hold(FIRST_KEY + 2, 1.1);
    let walking = daemon
        .wait_for("mapping", |e| e["done"] == false, 5.0)
        .expect("the wizard opened");
    assert_eq!(walking["layout"]["id"], "snes");
    let total = walking["total"].as_u64().expect("total") as u16;
    for step in 0..total {
        std::thread::sleep(Duration::from_millis(450));
        pad.tap(FIRST_KEY + step % KEY_COUNT);
        daemon.pump(0.2);
    }
    let finished = daemon
        .wait_for("mapping", |e| e["done"] == true, 5.0)
        .expect("the wizard finished");
    assert_eq!(finished["stored"], true, "{finished}");
    let stored: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::fs::read_dir(&daemon.profiles)
                .expect("profiles")
                .flatten()
                .next()
                .expect("a profile")
                .path(),
        )
        .expect("read"),
    )
    .expect("json");
    let buttons = &stored["mappings"][""]["buttons"];
    assert!(buttons.get("a").is_some(), "{stored}");
    assert_eq!(stored["mappings"][""]["layout"], "snes");

    // Now the pad is configured, and the state says so.
    daemon.send(serde_json::json!({"cmd": "status"}));
    let state = daemon.last("state").expect("state").clone();
    assert_eq!(state["players"][0]["configured"], true, "{state}");
    assert_eq!(state["players"][0]["mappings"], serde_json::json!([""]));

    // Cancel releases the pads and republishing resumes.
    daemon.send(serde_json::json!({"cmd": "cancel"}));
    let state = daemon
        .wait_for("state", |e| e["state"] == "ready", 5.0)
        .expect("ready after cancel");
    assert_eq!(state["state"], "ready");

    drop(daemon);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_malformed_command_is_answered_not_fatal() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(HOSTILE);
    let root = std::env::temp_dir().join(format!("padmap-daemon-hostile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let _pad = TestPad::new(HOSTILE);
    let mut daemon = Daemon::start(&root, HOSTILE);

    daemon.send(serde_json::json!({"cmd": "begin", "players": "lots"}));
    let error = daemon
        .wait_for(
            "error",
            |e| e["message"].as_str().is_some_and(|m| m.contains("begin")),
            3.0,
        )
        .expect("an error reply naming the command");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains("not a number"),
        "{error}"
    );
    daemon.send(serde_json::json!({"cmd": "nonsense"}));
    daemon
        .wait_for(
            "error",
            |e| {
                e["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("unknown command"))
            },
            3.0,
        )
        .expect("unknown command refused");
    // Not JSON at all, then deeply nested, then a real command: the framing
    // resynchronises and the daemon is still there.
    daemon.sock.write_all(b"this is not json\n").expect("send");
    let poison = format!("{}{}\n", "[".repeat(100_000), "]".repeat(100_000));
    daemon.sock.write_all(poison.as_bytes()).expect("send");
    let before = daemon
        .events
        .iter()
        .filter(|e| e["event"] == "state")
        .count();
    daemon.send(serde_json::json!({"cmd": "status"}));
    daemon
        .wait_for("state", |_| true, 5.0)
        .expect("still answering");
    let states = daemon
        .events
        .iter()
        .filter(|e| e["event"] == "state")
        .count();
    assert!(
        states > before,
        "the daemon stopped answering: {} state events",
        states
    );
    assert!(
        matches!(daemon.child.try_wait(), Ok(None)),
        "the daemon exited"
    );

    drop(daemon);
    let _ = std::fs::remove_dir_all(&root);
}
