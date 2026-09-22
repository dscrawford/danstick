//! Real daemon journey: PADMAP_ONLY_DEVICE isolates the test, and signatures go to prompted first.

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
const SLEEPER: PadId = PadId {
    name: "PADMAP RSTESTSLEEPER",
    pid: 0x0005,
    only: "RSTESTSLEEPER",
};
const JOINER: PadId = PadId {
    name: "PADMAP RSTESTJOINER",
    pid: 0x0006,
    only: "RSTESTJOINER",
};
const TUNER: PadId = PadId {
    name: "PADMAP RSTESTTUNER",
    pid: 0x0007,
    only: "RSTESTTUNER",
};
const REBIND: PadId = PadId {
    name: "PADMAP RSTESTREBIND one",
    pid: 0x0008,
    only: "RSTESTREBIND",
};
const MIRRORED: PadId = PadId {
    name: "PADMAP RSTESTMIRROR pad",
    pid: 0x000a,
    only: "RSTESTMIRROR",
};
const UNSEATED: PadId = PadId {
    name: "PADMAP RSTESTUNSEAT",
    pid: 0x000b,
    only: "RSTESTUNSEAT",
};
const FRESH: PadId = PadId {
    name: "PADMAP RSTESTFRESH",
    pid: 0x000c,
    only: "RSTESTFRESH",
};
const KEYSEAT: PadId = PadId {
    name: "PADMAP RSTESTKEYSEAT",
    pid: 0x000e,
    only: "RSTESTKEYSEAT",
};
/// Two pads behind one filter, so the second can appear after the session opens.
const LATE_FIRST: PadId = PadId {
    name: "PADMAP RSTESTLATE one",
    pid: 0x000f,
    only: "RSTESTLATE",
};
const LATE_SECOND: PadId = PadId {
    name: "PADMAP RSTESTLATE two",
    pid: 0x0010,
    only: "RSTESTLATE",
};
/// A seat claimed under a hold longer than the default.
const HOLDER: PadId = PadId {
    name: "PADMAP RSTESTHOLD",
    pid: 0x0012,
    only: "RSTESTHOLD",
};
const BINDER: PadId = PadId {
    name: "PADMAP RSTESTBIND",
    pid: 0x0011,
    only: "RSTESTBIND",
};
/// A Valve-vendor pad beside a Valve-vendor keyboard, the Puck's lizard shape.
const SIBLING: PadId = PadId {
    name: "PADMAP RSTESTSIB pad",
    pid: 0x0f00,
    only: "RSTESTSIB",
};
const SIBLING_KEYBOARD_NAME: &str = "PADMAP RSTESTSIB keyboard";
/// No pad is made for this one; the daemon under test only needs a filter.
const FOLLOWER: PadId = PadId {
    name: "PADMAP RSTESTFOLLOW",
    pid: 0x000d,
    only: "RSTESTFOLLOW",
};
/// Steam's virtual gamepad, by id; the name only has to pass the test filter.
const MIRROR_NAME: &str = "PADMAP RSTESTMIRROR X-Box 360 pad 0";
const MIRROR_VID: u16 = 0x28de;
const MIRROR_PID: u16 = 0x11ff;

fn signature(id: PadId) -> String {
    format!("{PAD_VID:04x}:{:04x}:{}", id.pid, id.name)
}

fn uinput_writable() -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok()
}

struct LiveGuard {
    path: PathBuf,
    added: bool,
    signature: String,
}

impl LiveGuard {
    fn new(id: PadId) -> LiveGuard {
        LiveGuard::for_signature(signature(id))
    }

    fn for_signature(signature: String) -> LiveGuard {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
        let path = Path::new(&base).join("padmap").join("prompted");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let mut added = false;
        if !existing.lines().any(|line| line.trim() == signature) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&path, format!("{existing}{signature}\n")).is_ok() {
                added = true;
            }
        }
        LiveGuard {
            path,
            added,
            signature,
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
        TestPad::with_id(id.name, PAD_VID, id.pid)
    }

    fn with_id(name: &str, vid: u16, pid: u16) -> TestPad {
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in FIRST_KEY..FIRST_KEY + KEY_COUNT {
            keys.insert(KeyCode::new(code));
        }
        let stick = AbsInfo::new(128, 0, 255, 0, 0, 0);
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        let device = VirtualDevice::builder()
            .expect("uinput")
            .name(name)
            .input_id(InputId::new(BusType::BUS_USB, vid, pid, 1))
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
        Daemon::start_with(root, id, &[])
    }

    fn start_with(root: &Path, id: PadId, extra: &[&str]) -> Daemon {
        Daemon::start_with_env(root, id, extra, &[])
    }

    fn start_with_env(root: &Path, id: PadId, extra: &[&str], env: &[(&str, &str)]) -> Daemon {
        let runtime = root.join("run");
        let config = root.join("config");
        let profiles = root.join("devices");
        for dir in [&runtime, &config, &profiles] {
            std::fs::create_dir_all(dir).expect("mkdir");
        }
        let child = Command::new(env!("CARGO_BIN_EXE_padmap-rs"))
            .arg("serve")
            .args(extra)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_DATA_HOME", root.join("data"))
            .env("PADMAP_PROFILE_DIR", &profiles)
            .env("PADMAP_SDL_DB", root.join("sdl_controllers.txt"))
            .env("PADMAP_ONLY_DEVICE", id.only)
            .env("PADMAP_NO_AUTOSETUP", "1")
            .env("PADMAP_DSU_PORT", "0")
            .env("PADMAP_CEMU_DIR", root.join("cemu"))
            .env("PADMAP_ARES_SETTINGS", root.join("nowhere/ares.bml"))
            .env("PADMAP_RYUJINX_CONFIG", root.join("nowhere/Config.json"))
            .env("RUST_LOG", "info,padmap_daemon=debug")
            .envs(env.iter().copied())
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

impl Daemon {
    /// Tap a button until the wizard says it bound something, tapping again if
    /// it does not: a tap that lands inside the capture gap after the previous
    /// binding is ignored on purpose, and under load that window is not ours to
    /// time.
    fn tap_until_bound(&mut self, pad: &mut TestPad, code: u16, index: u64) -> Value {
        for attempt in 0..4 {
            pad.tap(code);
            if let Some(event) = self.wait_for("mapping", |e| e["index"] == index, 3.0) {
                return event;
            }
            eprintln!("tap {attempt} bound nothing; tapping again");
            std::thread::sleep(Duration::from_millis(600));
        }
        panic!("tapping never bound control {index}");
    }

    /// Hold a button until a `claim` matching `wanted` arrives, holding again
    /// if it does not: a freshly made pad is grabbed by Steam for a moment, and
    /// under a full parallel run how long that moment lasts is not ours to say.
    fn hold_until_claimed(&mut self, pad: &mut TestPad, wanted: impl Fn(&Value) -> bool) -> Value {
        for attempt in 0..4 {
            pad.hold(FIRST_KEY, 0.6);
            if let Some(claim) = self.wait_for("claim", &wanted, 3.0) {
                return claim;
            }
            eprintln!("hold {attempt} claimed nothing; holding again");
            std::thread::sleep(Duration::from_millis(700));
        }
        panic!("holding a button never took a seat");
    }

    /// Open seating and hold a button until the pad is `player` and published.
    fn seat_by_hold_as(&mut self, pad: &mut TestPad, player: u64) {
        self.pump(1.5);
        self.events.clear();
        self.send(serde_json::json!({"cmd": "seating", "open": true, "players": 4}));
        let claim = self.hold_until_claimed(pad, |e| e["name"] != "Keyboard");
        assert_eq!(claim["player"], player);
        self.wait_for("state", |e| e["state"] == "ready", 5.0)
            .expect("ready after the seat was taken");
    }

    /// Open seating and hold a button until the pad is player 1 and published.
    ///
    /// A pad that appeared a moment ago is not readable by anybody yet when
    /// Steam is running: it grabs every new joystick briefly to look at it,
    /// and a hold made under that grab reaches nobody. Measured here at a
    /// little over a second; the seating test above survives it only because
    /// it holds once while seating is still closed.
    fn seat_by_hold(&mut self, pad: &mut TestPad) {
        self.pump(1.5);
        self.events.clear();
        self.send(serde_json::json!({"cmd": "seating", "open": true, "players": 4}));
        let claim = self.hold_until_claimed(pad, |_| true);
        assert_eq!(claim["player"], 1);
        let ready = self
            .wait_for("state", |e| e["state"] == "ready", 5.0)
            .expect("ready after the seat was taken");
        assert_eq!(ready["players"][0]["published"], true);
    }

    /// Whether the daemon process has exited within `seconds`.
    fn exited_within(&mut self, seconds: f64) -> bool {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
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

    let state = daemon
        .last("state")
        .expect("a state event on connect")
        .clone();
    assert_eq!(state["state"], "idle");
    assert_eq!(state["identity"], "mirror");
    assert!(state["pid"].as_u64().is_some());

    daemon.send(serde_json::json!({"cmd": "begin", "players": 2}));
    let pads = daemon.wait_for("pads", |_| true, 5.0).expect("pads");
    assert_eq!(pads["count"], 1);
    daemon
        .wait_for("state", |e| e["state"] == "assigning", 5.0)
        .expect("assigning");

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
    assert!(launch.contains("input_player16_joypad_index"), "{launch}");
    assert!(launch.contains("config_save_on_exit = \"false\""));
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
    let node = seated["virtual"]["node"]
        .as_str()
        .expect("the clone's node")
        .to_owned();
    assert!(node.starts_with("/dev/input/event"), "{node}");
    let guid = seated["virtual"]["guid"].as_str().expect("guid");
    assert!(
        mapping["lines"][0]
            .as_str()
            .expect("line")
            .starts_with(guid),
        "list says {guid}, the SDL line says {}",
        mapping["lines"][0]
    );
    assert_eq!(seated["controller"]["motion"], false);
    assert!(seated["controller"]["motion_node"].is_null());
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

    daemon.send(serde_json::json!({"cmd": "status"}));
    let state = daemon.last("state").expect("state").clone();
    assert_eq!(state["players"][0]["configured"], true, "{state}");
    assert_eq!(state["players"][0]["mappings"], serde_json::json!([""]));

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

/// One controller that is switched off must not cost the others theirs.
#[test]
fn a_sleeping_pad_does_not_unpublish_the_others() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(SLEEPER);
    let root = std::env::temp_dir().join(format!("padmap-sleeper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("run/padmap")).expect("mkdir");
    let pad = TestPad::new(SLEEPER);
    let present = std::fs::read_dir("/sys/class/input")
        .expect("sysfs")
        .flatten()
        .find(|entry| {
            std::fs::read_to_string(entry.path().join("device/name"))
                .map(|name| name.trim() == SLEEPER.name)
                .unwrap_or(false)
        })
        .map(|entry| format!("/dev/input/{}", entry.file_name().to_string_lossy()))
        .expect("the test pad has a node");

    let assignments = serde_json::json!([
        {"player": 1, "path": present, "name": SLEEPER.name,
         "phys": "", "vid": PAD_VID, "pid": SLEEPER.pid},
        {"player": 2, "path": "/dev/input/event9999", "name": "Xbox Wireless Controller",
         "phys": "50:2e:91:08:d7:d1", "vid": 1118, "pid": 654}
    ]);
    std::fs::write(
        root.join("run/padmap/assignments.json"),
        serde_json::to_string_pretty(&assignments).expect("json"),
    )
    .expect("seed the assignments");

    let mut daemon = Daemon::start(&root, SLEEPER);
    let state = daemon
        .wait_for("state", |e| e["state"] == "ready", 10.0)
        .unwrap_or_else(|| {
            panic!(
                "the daemon never went ready; one sleeping pad took the roster down: {:?}",
                daemon.last("state")
            )
        });

    // The sleeping pad's seat survives a save.
    daemon.send(serde_json::json!({"cmd": "status"}));
    daemon.pump(0.5);
    let saved: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("run/padmap/assignments.json")).expect("assignments"),
    )
    .expect("json");
    let seats: Vec<u64> = saved
        .as_array()
        .expect("array")
        .iter()
        .map(|entry| entry["player"].as_u64().expect("player"))
        .collect();
    assert_eq!(seats, vec![1, 2], "the sleeping pad lost its seat: {saved}");

    let players = state["players"].as_array().expect("players");
    let seats: Vec<u64> = players
        .iter()
        .map(|p| p["player"].as_u64().expect("player"))
        .collect();
    assert_eq!(seats, vec![1], "only live seats are drawn from claims");

    assert_eq!(players[0]["published"], true, "{state}");
    let clones: BTreeSet<String> = std::fs::read_dir("/sys/class/input")
        .expect("sysfs")
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("name")).ok())
        .map(|name| name.trim().to_owned())
        .collect();
    assert!(
        clones.contains("padmap Player 1"),
        "player 1 was not republished because player 2 was asleep: {clones:?}"
    );

    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_pad_can_take_a_free_seat_without_a_session() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(JOINER);
    let root = std::env::temp_dir().join(format!("padmap-seating-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut pad = TestPad::new(JOINER);
    let mut daemon = Daemon::start(&root, JOINER);

    let state = daemon.wait_for("state", |_| true, 5.0).expect("a greeting");
    assert_eq!(state["state"], "idle");
    assert_eq!(state["players"].as_array().map(Vec::len), Some(0));

    pad.hold(FIRST_KEY, 0.6);
    daemon.pump(0.8);
    assert!(
        daemon.last("claim").is_none(),
        "a seat was taken while seating was closed"
    );

    // A freshly made pad is grabbed by Steam for a moment (see seat_by_hold);
    // under a full parallel run the hold above no longer covers that window.
    daemon.pump(1.5);
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "seating", "open": true, "players": 4}));
    let claim = daemon.hold_until_claimed(&mut pad, |_| true);
    assert_eq!(claim["player"], 1);
    assert_eq!(claim["name"], JOINER.name);

    assert!(
        !daemon
            .events
            .iter()
            .any(|event| event["event"] == "state" && event["state"] == "assigning"),
        "a session was opened behind the scenes"
    );

    let ready = daemon
        .wait_for("state", |e| e["state"] == "ready", 5.0)
        .expect("ready after the seat was taken");
    assert_eq!(ready["players"][0]["player"], 1);
    assert_eq!(ready["players"][0]["published"], true);
    let clones: BTreeSet<String> = std::fs::read_dir("/sys/class/input")
        .expect("sysfs")
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("name")).ok())
        .map(|name| name.trim().to_owned())
        .collect();
    assert!(clones.contains("padmap Player 1"), "{clones:?}");
    let state_dir = daemon.runtime.join("padmap");
    assert!(state_dir.join("assignments.json").is_file());
    assert!(state_dir
        .join("autoconfig/udev/padmap Player 1.cfg")
        .is_file());

    // A seated pad is being *played with*.
    daemon.events.clear();
    pad.hold(FIRST_KEY + 1, 0.8);
    daemon.pump(1.0);
    assert!(
        daemon.last("claim").is_none(),
        "a pad that already held a seat claimed another"
    );

    daemon.send(serde_json::json!({"cmd": "seating", "open": false}));
    daemon.events.clear();
    pad.hold(FIRST_KEY, 0.6);
    daemon.pump(0.8);
    assert!(daemon.last("claim").is_none());

    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn how_long_a_hold_takes_to_claim_a_seat_can_be_set() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(HOLDER);
    let root = std::env::temp_dir().join(format!("padmap-hold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut pad = TestPad::new(HOLDER);
    let mut daemon = Daemon::start_with_env(&root, HOLDER, &[], &[("PADMAP_HOLD_SECONDS", "1.5")]);
    daemon.wait_for("state", |_| true, 5.0).expect("a greeting");

    // A freshly made pad is grabbed by Steam for a moment (see seat_by_hold).
    daemon.pump(1.5);
    daemon.events.clear();
    // No `hold` field: the length is the environment's, and asking for seating
    // without one must not reset it.
    daemon.send(serde_json::json!({"cmd": "seating", "open": true, "players": 4}));

    // A second is a claim at the default and is not one at 1.5s. The progress
    // events are what say the daemon saw the hold: without them this would
    // pass just as well for a pad nobody was reading.
    pad.hold(FIRST_KEY, 1.0);
    daemon.pump(0.5);
    let seen: Vec<f64> = daemon
        .events
        .iter()
        .filter(|event| event["event"] == "progress")
        .filter_map(|event| event["frac"].as_f64())
        .collect();
    assert!(!seen.is_empty(), "the daemon never saw the hold at all");
    assert!(
        daemon.last("claim").is_none(),
        "a second took a seat under a hold of one and a half"
    );
    let highest = seen.iter().copied().fold(0.0_f64, f64::max);
    assert!(
        highest < 1.0,
        "progress reached {highest} in a second of a 1.5s hold"
    );
    assert!(
        highest > 0.4,
        "progress only reached {highest}; the hold was barely read"
    );

    // Opening again with a length of its own replaces it, no restart needed.
    daemon.events.clear();
    daemon.send(serde_json::json!({
        "cmd": "seating", "open": true, "players": 4, "hold": 0.25
    }));
    let claim = daemon.hold_until_claimed(&mut pad, |_| true);
    assert_eq!(claim["player"], 1);
    assert_eq!(claim["name"], HOLDER.name);
    daemon
        .wait_for("state", |e| e["state"] == "ready", 5.0)
        .expect("ready after the seat was taken");

    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_pad_cannot_take_a_seat_that_does_not_exist() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(JOINER);
    let root = std::env::temp_dir().join(format!("padmap-seating-full-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("run/padmap")).expect("mkdir");
    let mut pad = TestPad::new(JOINER);

    let assignments = serde_json::json!([
        {"player": 1, "path": "/dev/input/event9998", "name": "Someone Else",
         "phys": "elsewhere", "vid": 1, "pid": 2}
    ]);
    std::fs::write(
        root.join("run/padmap/assignments.json"),
        serde_json::to_string_pretty(&assignments).expect("json"),
    )
    .expect("seed");

    let mut daemon = Daemon::start(&root, JOINER);
    daemon.send(serde_json::json!({"cmd": "seating", "open": true, "players": 1}));
    daemon.events.clear();
    pad.hold(FIRST_KEY, 0.8);
    daemon.pump(1.2);
    assert!(
        daemon.last("claim").is_none(),
        "a pad took a seat that was already held by an absent controller"
    );

    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_tune_command_is_saved_and_applied_live() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(TUNER);
    let root = std::env::temp_dir().join(format!("padmap-tune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut pad = TestPad::new(TUNER);
    let mut daemon = Daemon::start(&root, TUNER);

    daemon.events.clear();
    daemon.send(serde_json::json!({
        "cmd": "tune", "signature": signature(TUNER), "deadzone": 0.25, "debounce_ms": 30
    }));
    let tuned = daemon
        .wait_for("tuned", |_| true, 5.0)
        .expect("a tuned event");
    assert_eq!(tuned["player"], 0, "not seated yet");
    assert_eq!(tuned["signature"], signature(TUNER));
    assert_eq!(tuned["tuning"]["deadzone"]["0"], 0.25);
    assert_eq!(tuned["tuning"]["deadzone"]["1"], 0.25);
    assert!(tuned["tuning"]["deadzone"].get("16").is_none(), "{tuned}");
    assert_eq!(tuned["tuning"]["debounce_ms"], 30);

    let profiles = root.join("devices");
    let stored: Vec<Value> = std::fs::read_dir(&profiles)
        .expect("profiles dir")
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    let mine = stored
        .iter()
        .find(|profile| profile["signature"] == signature(TUNER))
        .expect("a profile for the tuned pad");
    assert_eq!(mine["tuning"]["debounce_ms"], 30, "{mine}");

    daemon.send(serde_json::json!({"cmd": "seating", "open": true, "players": 4}));
    pad.hold(FIRST_KEY, 0.6);
    daemon
        .wait_for("state", |e| e["state"] == "ready", 5.0)
        .expect("seated and published");
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "tune", "player": 1, "reset": true, "debounce_ms": 10}));
    let tuned = daemon
        .wait_for("tuned", |_| true, 5.0)
        .expect("a second tuned event");
    assert_eq!(tuned["player"], 1);
    assert!(
        tuned["tuning"].get("deadzone").is_none(),
        "reset dropped it: {tuned}"
    );
    assert_eq!(tuned["tuning"]["debounce_ms"], 10);
    assert!(
        !daemon
            .events
            .iter()
            .any(|e| e["event"] == "controller" && e["action"] == "removed"),
        "tuning a seated pad took it off the air"
    );
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "status"}));
    let state = daemon.wait_for("state", |_| true, 3.0).expect("a state");
    assert_eq!(state["state"], "ready");
    assert_eq!(state["players"][0]["published"], true);

    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "tune", "player": 1, "deadzone": "lots"}));
    let error = daemon.wait_for("error", |_| true, 3.0).expect("a refusal");
    assert!(
        error["message"].as_str().unwrap_or("").contains("deadzone"),
        "{error}"
    );
    daemon.send(serde_json::json!({"cmd": "tune", "player": 3, "debounce_ms": 5}));
    let error = daemon
        .wait_for(
            "error",
            |e| e["message"].as_str().unwrap_or("").contains("player 3"),
            3.0,
        )
        .expect("no such player");
    assert!(error["message"]
        .as_str()
        .unwrap_or("")
        .contains("no controller"));

    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}

// Request: finishing a rebind from the pad, and doing it with no session open.
#[test]
fn a_seated_pad_is_rebound_and_finished_from_the_pad_with_no_session() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(REBIND);
    let root = std::env::temp_dir().join(format!("padmap-rebind-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut pad = TestPad::new(REBIND);
    let mut daemon = Daemon::start(&root, REBIND);

    // Seat the pad through a session, then accept: the daemon grabs and keeps
    // it, so it is genuinely seated when the session closes.
    daemon.send(serde_json::json!({"cmd": "begin", "players": 1}));
    daemon
        .wait_for("state", |e| e["state"] == "assigning", 6.0)
        .expect("assigning");
    pad.hold(FIRST_KEY, 0.6);
    daemon
        .wait_for("claim", |e| e["player"] == 1, 6.0)
        .expect("a hold claims the seat");
    pad.hold(FIRST_KEY + 1, 1.1);
    daemon
        .wait_for("accepted", |_| true, 6.0)
        .expect("a second hold confirms and closes the session");
    daemon
        .wait_for("state", |e| e["state"] == "ready", 6.0)
        .expect("ready, and no session open");
    std::thread::sleep(Duration::from_millis(300));

    // Rebinding needs no session: map is legal with the daemon idle-but-ready.
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "map", "player": 1, "layout": "snes"}));
    let walking = daemon
        .wait_for("mapping", |e| e["done"] == false, 6.0)
        .expect("the wizard opened without a session");
    assert_eq!(walking["player"], 1);
    assert_eq!(walking["layout"]["id"], "snes");
    assert_eq!(
        walking["captured"],
        serde_json::json!({}),
        "nothing stored yet"
    );
    assert!(
        !daemon
            .events
            .iter()
            .any(|e| e["event"] == "state" && e["state"] == "assigning"),
        "a session was opened behind the scenes"
    );
    assert!(
        daemon.last("pads").is_none(),
        "a session announced its pads"
    );

    // Bind two controls by tapping. Each press is reported as it happens,
    // press and release, in the terms the profile will use for it.
    daemon.events.clear();
    daemon.tap_until_bound(&mut pad, FIRST_KEY + 4, 1);
    let inputs: Vec<&Value> = daemon
        .events
        .iter()
        .filter(|e| e["event"] == "input")
        .collect();
    assert!(
        inputs
            .iter()
            .any(|e| e["kind"] == "button" && e["index"] == 4 && e["value"] == 1),
        "the press was not reported: {inputs:?}"
    );
    assert!(
        inputs
            .iter()
            .any(|e| e["kind"] == "button" && e["index"] == 4 && e["value"] == 0),
        "the release was not reported: {inputs:?}"
    );
    assert_eq!(inputs[0]["player"], 1);
    let two = daemon.tap_until_bound(&mut pad, FIRST_KEY + 5, 2);
    assert_eq!(two["captured"].as_object().map(|c| c.len()), Some(2));
    std::thread::sleep(Duration::from_millis(400));

    // Long-hold A: the finish ring fills, and holding on ends the run with no release.
    daemon.events.clear();
    pad.emit(EventType::KEY.0, FIRST_KEY + 2, 1);
    daemon.pump(1.2);
    let filling = daemon
        .last("finish")
        .expect("a finish ring is drawn")
        .clone();
    let fraction = filling["frac"].as_f64().expect("frac");
    assert!(
        fraction > 0.2 && fraction < 0.95,
        "half-filled ring, got {filling}"
    );
    assert_eq!(filling["player"], 1);
    let finished = daemon
        .wait_for("mapping", |e| e["done"] == true, 6.0)
        .expect("the hold finished the wizard without a release");
    assert_eq!(finished["stored"], true, "{finished}");
    assert!(
        daemon
            .events
            .iter()
            .any(|e| e["event"] == "finish" && e["frac"] == 1.0),
        "the ring is shown full before the wizard closes"
    );
    pad.emit(EventType::KEY.0, FIRST_KEY + 2, 0);
    daemon.pump(0.3);

    // Exactly what was bound is on disk; the daemon never left ready.
    let stored: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::fs::read_dir(&daemon.profiles)
                .expect("profiles")
                .flatten()
                .find(|entry| entry.file_name().to_string_lossy().contains("one"))
                .expect("player 1's profile")
                .path(),
        )
        .expect("read"),
    )
    .expect("json");
    let buttons = stored["mappings"][""]["buttons"]
        .as_object()
        .expect("buttons");
    assert_eq!(
        buttons.len(),
        2,
        "an early finish keeps what was bound: {stored}"
    );
    let state = daemon.last("state").expect("state").clone();
    assert_eq!(state["state"], "ready", "no session was ever opened");
    assert_eq!(state["players"][0]["configured"], true);

    // A second run is seeded from the stored capture and guards it.
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "map", "player": 1, "layout": "snes"}));
    let seeded = daemon
        .wait_for("mapping", |e| e["done"] == false, 6.0)
        .expect("the wizard opened again");
    assert_eq!(
        seeded["captured"].as_object().map(|c| c.len()),
        Some(2),
        "the run is seeded from the stored capture: {seeded}"
    );
    pad.tap(FIRST_KEY + 5);
    let conflict = daemon
        .wait_for("mapping", |e| e["conflict"] != "", 6.0)
        .expect("the second control's stored input is defended across runs");
    assert_eq!(conflict["index"], 0, "a refused press does not advance");
    daemon.send(serde_json::json!({"cmd": "cancel"}));
    daemon
        .wait_for("mapping", |e| e["done"] == true, 6.0)
        .expect("cancel closes the wizard");
    daemon
        .wait_for("state", |e| e["state"] == "ready", 6.0)
        .expect("still ready, still no session");

    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}

// Request: one physical controller is one pad -- Steam's mirror is dropped, and said so.
#[test]
fn steams_mirror_is_listed_as_dropped_beside_the_pad_it_mirrors() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guards = (
        LiveGuard::new(MIRRORED),
        LiveGuard::for_signature(format!("{MIRROR_VID:04x}:{MIRROR_PID:04x}:{MIRROR_NAME}")),
    );
    let root = std::env::temp_dir().join(format!("padmap-mirror-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let list = |json: bool| -> String {
        let mut args = vec!["list"];
        if json {
            args.push("--json");
        }
        let output = Command::new(env!("CARGO_BIN_EXE_padmap-rs"))
            .args(&args)
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("PADMAP_PROFILE_DIR", root.join("devices"))
            .env("PADMAP_ONLY_DEVICE", MIRRORED.only)
            .output()
            .expect("list");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    let mirror = TestPad::with_id(MIRROR_NAME, MIRROR_VID, MIRROR_PID);
    let real = TestPad::new(MIRRORED);
    std::thread::sleep(Duration::from_millis(300));

    let entries: Value = serde_json::from_str(&list(true)).expect("valid JSON");
    let entries = entries.as_array().expect("an array");
    assert_eq!(entries.len(), 2, "{entries:?}");
    let kept: Vec<&Value> = entries.iter().filter(|e| e["dropped"].is_null()).collect();
    assert_eq!(kept.len(), 1, "only the real pad is offered: {entries:?}");
    assert_eq!(kept[0]["controller"]["name"], MIRRORED.name);
    let dropped = entries
        .iter()
        .find(|e| !e["dropped"].is_null())
        .expect("the mirror is listed, not vanished");
    assert_eq!(dropped["controller"]["name"], MIRROR_NAME);
    assert_eq!(dropped["controller"]["vid"], "28de");
    assert_eq!(dropped["controller"]["pid"], "11ff");
    assert!(dropped["player"].is_null() && dropped["virtual"].is_null());
    assert!(
        dropped["dropped"]
            .as_str()
            .expect("a reason")
            .contains("mirrors"),
        "{dropped}"
    );
    let prose = list(false);
    assert!(prose.contains("Left out, on purpose:"), "{prose}");
    assert!(prose.contains(MIRROR_NAME), "{prose}");

    drop(real);
    std::thread::sleep(Duration::from_millis(500));
    let entries: Value = serde_json::from_str(&list(true)).expect("valid JSON");
    let entries = entries.as_array().expect("an array");
    assert_eq!(
        entries.len(),
        1,
        "alone, the mirror is a controller: {entries:?}"
    );
    assert!(entries[0]["dropped"].is_null());
    assert_eq!(entries[0]["controller"]["name"], MIRROR_NAME);
    assert!(
        !list(false).contains("Left out"),
        "nothing dropped when nothing is mirrored"
    );

    drop(mirror);
    let _ = std::fs::remove_dir_all(&root);
}

/// `unseat` frees the seat, stops the clone and forgets the seat on disk --
/// and leaves seating open, so the same hold takes the seat straight back.
#[test]
fn unseat_drops_the_seat_and_the_next_hold_takes_it_again() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(UNSEATED);
    let root = std::env::temp_dir().join(format!("padmap-unseat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut pad = TestPad::new(UNSEATED);
    let mut daemon = Daemon::start(&root, UNSEATED);
    daemon.seat_by_hold(&mut pad);
    let state_dir = daemon.runtime.join("padmap");
    let sdl_db = root.join("sdl_controllers.txt");
    assert_eq!(sdl_mappings_in(&sdl_db), Some(1), "one clone published");

    // Unseating a player nobody holds is an error, not a silent no-op.
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "unseat", "player": 3}));
    assert!(
        daemon.last("error").is_some(),
        "unseating an empty seat was accepted"
    );

    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "unseat", "player": 1}));
    let state = daemon
        .wait_for(
            "state",
            |e| e["players"].as_array().map(Vec::len) == Some(0),
            5.0,
        )
        .expect("a state with nobody seated");
    assert_eq!(state["state"], "idle");
    // The freed pad is re-announced as unconfigured right after; the removal
    // is the event before that one.
    assert!(
        daemon
            .events
            .iter()
            .any(|e| e["event"] == "controller" && e["action"] == "removed" && e["player"] == 1),
        "no removal was announced for player 1"
    );
    assert!(
        !daemon
            .events
            .iter()
            .any(|event| event["event"] == "state" && event["state"] == "assigning"),
        "unseat opened a session"
    );
    let saved = std::fs::read_to_string(state_dir.join("assignments.json")).expect("saved");
    let saved: Value = serde_json::from_str(&saved).expect("json");
    assert_eq!(
        saved.as_array().map(Vec::len),
        Some(0),
        "the seat survived on disk"
    );
    // Other journeys publish a "padmap Player 1" of their own, so the clone's
    // absence is read from this daemon's consumer files, not from sysfs.
    assert_eq!(
        sdl_mappings_in(&sdl_db),
        Some(0),
        "the clone outlived the seat"
    );

    // Seating was not closed by any of that: the next hold is player 1 again.
    daemon.events.clear();
    let claim = daemon.hold_until_claimed(&mut pad, |_| true);
    assert_eq!(claim["player"], 1);
    daemon
        .wait_for("state", |e| e["state"] == "ready", 5.0)
        .expect("ready again");

    // And with no player named, everybody goes.
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "unseat"}));
    let state = daemon
        .wait_for(
            "state",
            |e| e["players"].as_array().map(Vec::len) == Some(0),
            5.0,
        )
        .expect("nobody seated after unseat with no player");
    assert_eq!(state["state"], "idle");

    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}

/// The same daemon, restarted: plain brings yesterday's seat back, `--fresh` does not.
#[test]
fn a_fresh_daemon_starts_with_nobody_seated() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(FRESH);
    let root = std::env::temp_dir().join(format!("padmap-fresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut pad = TestPad::new(FRESH);

    let mut daemon = Daemon::start(&root, FRESH);
    daemon.seat_by_hold(&mut pad);
    drop(daemon);

    // Restoring a real pad republishes it before the first client is greeted.
    let mut daemon = Daemon::start(&root, FRESH);
    let state = daemon.wait_for("state", |_| true, 5.0).expect("a greeting");
    assert_eq!(
        state["players"].as_array().map(Vec::len),
        Some(1),
        "a plain restart forgot the seat: {state}"
    );
    assert_eq!(state["following"], Value::Null);
    drop(daemon);

    let mut daemon = Daemon::start_with(&root, FRESH, &["--fresh"]);
    let state = daemon.wait_for("state", |_| true, 5.0).expect("a greeting");
    assert_eq!(
        state["players"].as_array().map(Vec::len),
        Some(0),
        "--fresh restored the seat: {state}"
    );
    assert_eq!(state["state"], "idle");
    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}

/// `--follow PID` ends the daemon when that pid is gone, and says so in `state`.
#[test]
fn a_daemon_that_follows_a_pid_ends_when_it_does() {
    let root = std::env::temp_dir().join(format!("padmap-follow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut leader = Command::new("sleep")
        .arg("60")
        .stdout(Stdio::null())
        .spawn()
        .expect("a process to follow");
    let pid = leader.id().to_string();

    let mut daemon = Daemon::start_with(&root, FOLLOWER, &["--follow", &pid]);
    let state = daemon.last("state").expect("a greeting").clone();
    assert_eq!(state["following"], leader.id());
    // Seating open and no client connected: the shape of a game in progress.
    daemon.send(serde_json::json!({"cmd": "seating", "open": true, "players": 4}));
    assert!(
        !daemon.exited_within(1.0),
        "the daemon ended while its pid was alive"
    );

    leader.kill().expect("kill");
    let _ = leader.wait();
    assert!(
        daemon.exited_within(3.0),
        "the daemon outlived the pid it follows"
    );
    assert!(
        !daemon.runtime.join("padmap").join("padmap.sock").exists(),
        "the socket was left behind"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A pid that is already gone at startup is not waited for.
#[test]
fn following_a_pid_that_is_already_gone_ends_at_once() {
    let root = std::env::temp_dir().join(format!("padmap-follow-gone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut gone = Command::new("true").spawn().expect("a short process");
    let pid = gone.id().to_string();
    let _ = gone.wait();

    let mut daemon = Daemon::start_with(&root, FOLLOWER, &["--follow", &pid]);
    assert!(
        daemon.exited_within(3.0),
        "the daemon waited for a pid that never existed"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Mapping lines in an SDL controller db, header comments aside.
fn sdl_mappings_in(path: &Path) -> Option<usize> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(
        text.lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
            .count(),
    )
}

/// `seat_keyboard` seats the keyboard as the next player with no device behind
/// it; a pad seated after it takes the seat after, and every consumer says so.
#[test]
fn the_keyboard_takes_a_seat_by_command_and_a_pad_sits_after_it() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(KEYSEAT);
    let root = std::env::temp_dir().join(format!("padmap-keyseat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut pad = TestPad::new(KEYSEAT);
    let mut daemon = Daemon::start(&root, KEYSEAT);
    let state_dir = daemon.runtime.join("padmap");
    let gcpad = root.join("config/dolphin-emu/GCPadNew.ini");

    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "seat_keyboard"}));
    let claim = daemon
        .wait_for("claim", |e| e["name"] == "Keyboard", 5.0)
        .expect("the keyboard took a seat");
    assert_eq!(claim["player"], 1);
    assert_eq!(claim["icon"], "keyboard");
    let state = daemon
        .wait_for(
            "state",
            |e| e["players"].as_array().map(Vec::len) == Some(1),
            5.0,
        )
        .expect("a state with the keyboard seated");
    assert_eq!(state["players"][0]["player"], 1);
    assert_eq!(state["players"][0]["keyboard"], true);
    assert_eq!(state["players"][0]["name"], "Keyboard");
    let saved = std::fs::read_to_string(state_dir.join("assignments.json")).expect("saved");
    assert!(
        saved.contains("\"keyboard\""),
        "the seat was not saved: {saved}"
    );
    let ini = std::fs::read_to_string(&gcpad).expect("Dolphin config");
    assert!(
        ini.contains("[GCPad1]\nDevice = XInput2/0/Virtual core pointer\n"),
        "{ini}"
    );

    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "seat_keyboard"}));
    assert!(
        daemon.last("error").is_some(),
        "the keyboard was seated twice"
    );

    // A pad seated after the keyboard is player 2.
    daemon.seat_by_hold_as(&mut pad, 2);
    let state = daemon.last("state").expect("state").clone();
    let players = state["players"].as_array().expect("players");
    assert_eq!(players.len(), 2, "{state}");
    assert_eq!(players[0]["keyboard"], true);
    assert_eq!(players[1]["player"], 2);
    assert_eq!(players[1]["published"], true);
    let ini = std::fs::read_to_string(&gcpad).expect("Dolphin config");
    assert!(
        ini.contains("[GCPad1]\nDevice = XInput2/0/Virtual core pointer\n"),
        "{ini}"
    );
    assert!(
        ini.contains("[GCPad2]\nDevice = SDL/0/padmap Player 2\n"),
        "{ini}"
    );
    let launch = std::fs::read_to_string(state_dir.join("launch.cfg")).expect("launch.cfg");
    assert!(
        !launch.contains("input_player1_b = \"nul\""),
        "the keyboard is player 1, so RetroArch's own defaults stand: {launch}"
    );

    // Unseating the keyboard frees seat 1; the pad keeps seat 2.
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "unseat", "player": 1}));
    let state = daemon
        .wait_for(
            "state",
            |e| e["players"].as_array().map(Vec::len) == Some(1),
            5.0,
        )
        .expect("one player left");
    assert_eq!(state["players"][0]["player"], 2);
    let saved = std::fs::read_to_string(state_dir.join("assignments.json")).expect("saved");
    assert!(
        !saved.contains("\"keyboard\""),
        "the keyboard seat survived on disk"
    );

    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}

/// A pad switched on during a session joins it: `pads` says so, a hold on it
/// claims a seat, and when it goes away `pads` says that too.
#[test]
fn a_pad_switched_on_during_a_session_can_take_a_seat() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _first = LiveGuard::new(LATE_FIRST);
    let _second = LiveGuard::new(LATE_SECOND);
    let root = std::env::temp_dir().join(format!("padmap-late-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let _one = TestPad::new(LATE_FIRST);
    let mut daemon = Daemon::start(&root, LATE_FIRST);
    daemon.pump(1.5);

    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "begin", "players": 2}));
    daemon
        .wait_for("state", |e| e["state"] == "assigning", 6.0)
        .expect("a session");
    assert_eq!(
        daemon.last("pads").map(|e| e["count"].clone()),
        Some(1.into())
    );

    // The second pad is switched on now, with the session open.
    let mut two = TestPad::new(LATE_SECOND);
    daemon
        .wait_for("pads", |e| e["count"] == 2, 6.0)
        .expect("the late pad was admitted to the session");
    daemon.pump(1.5);
    let claim = daemon.hold_until_claimed(&mut two, |e| e["name"] == LATE_SECOND.name);
    assert_eq!(claim["player"], 1);

    // Switched off again: the session says so, and does not fall over.
    daemon.events.clear();
    drop(two);
    daemon
        .wait_for("pads", |e| e["count"] == 1, 6.0)
        .expect("the departed pad was counted out");
    daemon.send(serde_json::json!({"cmd": "cancel"}));
    daemon
        .wait_for("state", |e| e["state"] != "assigning", 6.0)
        .expect("the session ended cleanly");

    drop(daemon);
    let _ = std::fs::remove_dir_all(&root);
}

/// A uinput keyboard: letters and Enter, so it classifies as one.
fn test_keyboard(name: &str, vid: u16) -> VirtualDevice {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in KeyCode::KEY_Q.0..=KeyCode::KEY_M.0 {
        keys.insert(KeyCode::new(code));
    }
    keys.insert(KeyCode::KEY_A);
    keys.insert(KeyCode::KEY_ENTER);
    let device = VirtualDevice::builder()
        .expect("uinput")
        .name(name)
        .input_id(InputId::new(BusType::BUS_USB, vid, 0x0f01, 1))
        .with_keys(&keys)
        .expect("keys")
        .build()
        .expect("a virtual keyboard");
    std::thread::sleep(Duration::from_millis(500));
    device
}

/// Whether the node can be grabbed by us, i.e. nobody else holds it.
fn grabbable(path: &Path) -> bool {
    let Ok(mut device) = evdev::Device::open(path) else {
        return false;
    };
    let ok = device.grab().is_ok();
    let _ = device.ungrab();
    ok
}

/// A seated pad's keyboard sibling is held by padmap, and released with the seat.
#[test]
fn a_seated_pads_keyboard_sibling_is_held_and_released_with_the_seat() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::for_signature(format!(
        "{:04x}:{:04x}:{}",
        padmap_input::siblings::VALVE_VID,
        SIBLING.pid,
        SIBLING.name
    ));
    let root = std::env::temp_dir().join(format!("padmap-sibling-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut keyboard = test_keyboard(SIBLING_KEYBOARD_NAME, padmap_input::siblings::VALVE_VID);
    let keyboard_node = keyboard
        .enumerate_dev_nodes_blocking()
        .expect("nodes")
        .flatten()
        .find(|path| path.to_string_lossy().contains("/dev/input/event"))
        .expect("the keyboard's node");
    let mut pad = TestPad::with_id(SIBLING.name, padmap_input::siblings::VALVE_VID, SIBLING.pid);
    let mut daemon = Daemon::start(&root, SIBLING);
    daemon.pump(1.5);
    assert!(
        grabbable(&keyboard_node),
        "nobody should hold the keyboard yet"
    );

    // Seating open: the pad is a candidate, so its keyboard is already held.
    daemon.send(serde_json::json!({"cmd": "seating", "open": true, "players": 4}));
    daemon.pump(1.5);
    assert!(
        !grabbable(&keyboard_node),
        "the candidate pad's keyboard is not held"
    );

    daemon.hold_until_claimed(&mut pad, |_| true);
    daemon.send(serde_json::json!({"cmd": "seating", "open": false}));
    daemon.pump(1.0);
    assert!(
        !grabbable(&keyboard_node),
        "the seated pad's keyboard is not held"
    );

    daemon.send(serde_json::json!({"cmd": "unseat"}));
    daemon
        .wait_for(
            "state",
            |e| e["players"].as_array().map(Vec::len) == Some(0),
            5.0,
        )
        .expect("unseated");
    daemon.pump(1.0);
    assert!(
        grabbable(&keyboard_node),
        "the keyboard was not released with the seat"
    );

    drop(daemon);
    drop(pad);
    drop(keyboard);
    let _ = std::fs::remove_dir_all(&root);
}

/// The only profile the daemon wrote, as JSON.
fn only_profile(dir: &Path) -> Value {
    let path = std::fs::read_dir(dir)
        .expect("the profile directory")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .expect("a profile was written");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json")
}

/// `bind` captures one press onto one control; with `add`, beside the binding
/// it already has, which is stored as a list whose first entry is unchanged.
#[test]
fn one_control_is_bound_on_its_own_and_can_take_a_second_input() {
    if !uinput_writable() {
        eprintln!("skipped: /dev/uinput is not writable");
        return;
    }
    let _guard = LiveGuard::new(BINDER);
    let root = std::env::temp_dir().join(format!("padmap-bind-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut pad = TestPad::new(BINDER);
    let mut daemon = Daemon::start(&root, BINDER);
    daemon.seat_by_hold(&mut pad);

    // A control nobody has heard of is refused, not guessed at.
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "bind", "player": 1, "control": "nonsense"}));
    assert!(
        daemon.last("error").is_some(),
        "an unknown control was accepted"
    );

    // Bind B on its own: one press, one control, stored.
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "bind", "player": 1, "control": "b"}));
    daemon
        .wait_for(
            "mapping",
            |e| e["done"] == false && e["control"] == "b",
            5.0,
        )
        .expect("the wizard asked for b alone");
    pad.tap(FIRST_KEY + 1);
    daemon
        .wait_for("mapping", |e| e["done"] == true, 5.0)
        .expect("one press ended the run");
    let profile = only_profile(&daemon.profiles);
    let first = profile["buttons"]["b"].clone();
    assert_eq!(
        first["kind"], "button",
        "one input is one object: {profile}"
    );
    assert!(!first.is_array());

    // And now a second input for the same control.
    daemon.events.clear();
    daemon.send(serde_json::json!({"cmd": "bind", "player": 1, "control": "b", "add": true}));
    daemon
        .wait_for(
            "mapping",
            |e| e["done"] == false && e["control"] == "b",
            5.0,
        )
        .expect("the wizard asked for b again");
    pad.tap(FIRST_KEY + 3);
    daemon
        .wait_for("mapping", |e| e["done"] == true, 5.0)
        .expect("the second press ended the run");
    let profile = only_profile(&daemon.profiles);
    let listed = profile["buttons"]["b"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("b should now be a list: {profile}"));
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0], first, "the first input is exactly what it was");
    assert_ne!(listed[1], first, "and the second is a different input");

    drop(daemon);
    drop(pad);
    let _ = std::fs::remove_dir_all(&root);
}
