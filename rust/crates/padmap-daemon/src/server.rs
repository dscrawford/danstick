//! The daemon proper: one process, one loop, every long-lived thing.
//! Only command dispatch and the tick are guarded against panics.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use log::{debug, info, warn};
use padmap_core::announce;
use padmap_core::capture::{self, Chooser, MappingRun, Outcome};
use padmap_core::command::{Command, Refused};
use padmap_core::state::{PlayerState, STATE_ASSIGNING, STATE_IDLE, STATE_READY};
use padmap_core::tuning::Request;
use padmap_core::wire::{self, LineReader};
use padmap_core::{emit, scope};
use padmap_input::clone::{self, IdentityMode, Source};
use padmap_input::pad::{self, Pad};
use padmap_input::reactor::{Reactor, Watched};
use padmap_input::republish::Republisher;
use padmap_input::{artefacts, assignments, profiles, runtime, siblings, triton};
use serde_json::Value;

use crate::calibration::{CalibrationRun, Phase, Step};
use crate::confirm::ConfirmHold;
use crate::dsu::Motion as DsuMotion;
use crate::hotplug::{self, Attached};
use crate::publish::{self, Slot};
use crate::seating::Seating;
use crate::session::{Raw, Session};
use crate::{clean, events, now};

pub const ENV_NO_AUTOSETUP: &str = "PADMAP_NO_AUTOSETUP";
pub const ENV_NO_AUTOATTACH: &str = "PADMAP_NO_AUTOATTACH";

pub const PAD_SCAN_SECONDS: f64 = 1.0;
pub const TRITON_SCAN_SECONDS: f64 = 1.0;
/// Client must be connected this long before daemon will grab pads.
pub const AUTOSETUP_CLIENT_SECONDS: f64 = 3.0;
pub const STALE_CHECK_SECONDS: f64 = 2.0;

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("cannot create the directory for {0}: {1}")]
    Directory(PathBuf, std::io::Error),
    #[error("another padmap daemon is already listening on {0}")]
    Occupied(PathBuf),
    #[error("{0} is in the way and is not a socket padmap can remove: {1}")]
    InTheWay(PathBuf, std::io::Error),
    #[error("cannot listen on {0}: {1}")]
    Listen(PathBuf, std::io::Error),
    #[error("cannot create the event loop: {0}")]
    Reactor(rustix::io::Errno),
}

const MAX_PENDING: usize = 8 * 1024 * 1024;

struct Client {
    stream: UnixStream,
    reader: LineReader,
    connected_at: f64,
    /// Bytes written but not yet accepted by the socket, from the front.
    pending: Vec<u8>,
}

struct Modal<T> {
    run: T,
    pad_path: PathBuf,
}

/// One pad opened and grabbed on its own, for a modal flow with no session.
struct Solo {
    pad: Pad,
    source: Source,
    buffer: Vec<evdev::InputEvent>,
}

/// Where a modal flow's pad is open right now.
#[derive(Debug, Clone, Copy)]
enum Opened {
    Session(usize),
    Published(usize),
    Solo,
}

pub struct Server {
    pub socket_path: PathBuf,
    pub state_path: PathBuf,
    pub launch_config_path: PathBuf,
    pub launch_args_path: PathBuf,
    pub prompted_path: PathBuf,

    reactor: Reactor,
    listener: Option<UnixListener>,
    clients: BTreeMap<i32, Client>,

    state: &'static str,
    session: Option<Session>,
    slots_assigned: Vec<Slot>,
    away: Vec<assignments::Assignment>,
    republisher: Option<Republisher>,
    motion: Option<DsuMotion>,
    confirm: ConfirmHold,
    last_progress: f64,
    last_confirm: f64,
    slots: u32,
    icon_overrides: BTreeMap<String, String>,
    seating: Seating,
    calibration: Option<CalibrationRun>,
    mapping: Option<Modal<MappingRun>>,
    choice: Option<Modal<Chooser>>,
    solo: Option<Solo>,
    last_finish: f64,
    /// The last `input` event sent under the wizard; the same one is not sent twice.
    last_input: Option<Value>,
    /// Opens tried on a pad that appeared mid-session; a node is readable a beat after it exists.
    late_attempts: BTreeMap<PathBuf, u32>,
    /// The keyboard and mouse nodes of seated and candidate pads, grabbed beside their joysticks.
    held: siblings::Held,
    /// What `held` was last computed for: the event nodes and the pads that matter.
    held_for: Option<(BTreeSet<String>, Vec<PathBuf>)>,
    scratch: Vec<evdev::InputEvent>,
    pending_scope: String,
    sdl_lines: Vec<String>,
    mode: IdentityMode,

    prompted: BTreeSet<String>,
    prompted_stamp: i128,
    last_scan_signature: Option<(BTreeSet<String>, i128)>,
    last_pad_scan: f64,

    attached: Attached,
    last_attach_nodes: Option<BTreeSet<String>>,
    last_attach_scan: f64,
    last_triton_scan: f64,
    triton_live: BTreeSet<String>,
    triton_present: bool,
    triton_checked_for: Option<BTreeSet<String>>,
    last_stale_check: f64,
    tick_failures: u64,
    /// The keyboard's seat: no device, no clone, bound in every emulator by name.
    keyboard_seat: Option<u32>,
    /// The pid this daemon ends with; None outlives everything, as before.
    follow: Option<u32>,
    last_follow_check: f64,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("socket_path", &self.socket_path)
            .field("state", &self.state)
            .field("players", &self.slots_assigned.len())
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Scan {
    pads: Option<Vec<Pad>>,
}

impl Scan {
    fn pads(&mut self) -> &[Pad] {
        self.pads.get_or_insert_with(discover)
    }
}

fn discover() -> Vec<Pad> {
    match pad::discover(pad::Filter::default()) {
        Ok(pads) => pads,
        Err(error) => {
            warn!("enumerating input devices: {error}");
            Vec::new()
        }
    }
}

fn to_input_assignment(slot: &Slot) -> assignments::Assignment {
    assignments::Assignment {
        player: slot.player,
        path: slot.pad.path.clone(),
        name: slot.pad.name.clone(),
        phys: slot.pad.phys.clone(),
        vid: slot.pad.vid,
        pid: slot.pad.pid,
    }
}

impl Server {
    pub fn new() -> Result<Server, StartError> {
        let base = runtime::dir();
        Server::at(
            runtime::socket_path(),
            runtime::assignments_path(),
            base.join("launch.cfg"),
        )
    }

    pub fn at(
        socket_path: PathBuf,
        state_path: PathBuf,
        launch_config_path: PathBuf,
    ) -> Result<Server, StartError> {
        let launch_args_path = launch_config_path.with_extension("args");
        let prompted_path = launch_config_path
            .parent()
            .map(|dir| dir.join("prompted"))
            .unwrap_or_else(|| PathBuf::from("prompted"));
        let reactor = Reactor::new(crate::TICK).map_err(StartError::Reactor)?;
        let prompted = runtime::read_prompted(&prompted_path);
        let prompted_stamp = stamp_of(&prompted_path);
        Ok(Server {
            socket_path,
            state_path,
            launch_config_path,
            launch_args_path,
            prompted_path,
            reactor,
            listener: None,
            clients: BTreeMap::new(),
            state: STATE_IDLE,
            session: None,
            slots_assigned: Vec::new(),
            away: Vec::new(),
            republisher: None,
            motion: None,
            confirm: ConfirmHold::default(),
            last_progress: 0.0,
            last_confirm: 0.0,
            slots: 4,
            icon_overrides: runtime::load_icon_overrides(),
            seating: Seating::default(),
            calibration: None,
            mapping: None,
            choice: None,
            solo: None,
            last_finish: 0.0,
            last_input: None,
            late_attempts: BTreeMap::new(),
            held: siblings::Held::default(),
            held_for: None,
            scratch: Vec::with_capacity(64),
            pending_scope: String::new(),
            sdl_lines: Vec::new(),
            mode: IdentityMode::from_env(),
            prompted,
            prompted_stamp,
            last_scan_signature: None,
            last_pad_scan: 0.0,
            attached: Attached::default(),
            last_attach_nodes: None,
            last_attach_scan: 0.0,
            last_triton_scan: 0.0,
            triton_live: BTreeSet::new(),
            triton_present: false,
            triton_checked_for: None,
            last_stale_check: 0.0,
            tick_failures: 0,
            keyboard_seat: None,
            follow: None,
            last_follow_check: 0.0,
        })
    }

    pub fn start(&mut self) -> Result<(), StartError> {
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| StartError::Directory(self.socket_path.clone(), error))?;
        }
        if self.socket_path.exists() {
            // A socket left by a crashed daemon would fail bind() even though nothing listens.
            if daemon_alive(&self.socket_path) {
                return Err(StartError::Occupied(self.socket_path.clone()));
            }
            std::fs::remove_file(&self.socket_path)
                .map_err(|error| StartError::InTheWay(self.socket_path.clone(), error))?;
        }
        let listener = UnixListener::bind(&self.socket_path)
            .map_err(|error| StartError::Listen(self.socket_path.clone(), error))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| StartError::Listen(self.socket_path.clone(), error))?;
        if let Err(error) = self.reactor.watch(listener.as_fd(), Watched::Listener) {
            return Err(StartError::Listen(self.socket_path.clone(), error.into()));
        }
        self.listener = Some(listener);
        info!("listening on {}", self.socket_path.display());
        self.start_motion();
        Ok(())
    }

    fn start_motion(&mut self) {
        let port = match std::env::var("PADMAP_DSU_PORT") {
            Ok(value) => match value.trim().parse::<u16>() {
                Ok(port) => port,
                Err(_) => {
                    warn!("PADMAP_DSU_PORT={value:?} is not a port number; using the default");
                    padmap_core::dsu::PORT
                }
            },
            Err(_) => padmap_core::dsu::PORT,
        };
        if port == 0 {
            debug!("motion server disabled by PADMAP_DSU_PORT=0");
            return;
        }
        match DsuMotion::bind(port) {
            Ok(motion) => {
                if let Err(error) = self.reactor.watch(motion.as_fd(), Watched::Dsu) {
                    warn!("could not watch the motion socket: {error}");
                    return;
                }
                self.motion = Some(motion);
            }
            Err(error) => warn!(
                "no motion server on port {port} ({error}); controllers still work, \
                 their gyros do not"
            ),
        }
    }

    pub fn restore(&mut self) {
        let saved = match assignments::load(&self.state_path) {
            Ok(saved) => saved,
            Err(error) => {
                warn!("ignoring unreadable {}: {error}", self.state_path.display());
                return;
            }
        };
        if saved.is_empty() {
            return;
        }
        self.keyboard_seat = assignments::keyboard_seat(&saved);
        let pads = discover();
        let (found, missing) = assignments::resolve(&saved, &pads);
        let restored: Vec<Slot> = found
            .into_iter()
            .map(|(player, pad)| Slot {
                player,
                pad: pad.clone(),
            })
            .collect();
        self.away = missing.into_iter().cloned().collect();
        for entry in &self.away {
            warn!(
                "player {}: {} ({}) is not here; keeping the seat",
                entry.player,
                clean(&entry.name),
                entry.path.display()
            );
        }
        if restored.is_empty() && self.away.is_empty() && self.keyboard_seat.is_none() {
            return;
        }
        self.slots = restored
            .iter()
            .map(|slot| slot.player)
            .chain(self.away.iter().map(|entry| entry.player))
            .chain(self.keyboard_seat)
            .max()
            .unwrap_or(4)
            .max(4);
        self.slots_assigned = restored;
        match self.start_republisher() {
            Ok(()) => {
                self.state = STATE_READY;
                info!(
                    "restored {} assignment(s) from {}",
                    self.slots_assigned.len(),
                    self.state_path.display()
                );
            }
            Err(error) => {
                warn!("could not republish restored assignments: {error}");
                self.state = STATE_IDLE;
            }
        }
    }

    /// End with `pid`: once it is gone the daemon exits and releases every pad.
    /// Polled, because PR_SET_PDEATHSIG does not survive the reparenting
    /// `ensure-daemon` does.
    pub fn follow(&mut self, pid: u32) {
        self.follow = Some(pid);
        info!("following pid {pid}; ending when it does");
    }

    /// Whether the followed pid has gone away, checked at most every quarter second.
    fn followed_is_gone(&mut self) -> bool {
        let Some(pid) = self.follow else {
            return false;
        };
        let clock = now();
        if clock - self.last_follow_check < FOLLOW_POLL_SECONDS {
            return false;
        }
        self.last_follow_check = clock;
        !process_exists(pid)
    }

    pub fn run(&mut self, stop: &Arc<AtomicBool>) {
        while !stop.load(Ordering::Relaxed) {
            if self.followed_is_gone() {
                info!(
                    "pid {} is gone; ending with it",
                    self.follow.unwrap_or_default()
                );
                return;
            }
            let ready = match self.reactor.wait() {
                Ok(ready) => ready,
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => {
                    warn!("waiting for input: {error}");
                    continue;
                }
            };
            let items: Vec<Watched> = ready.iter().collect();
            for what in items {
                match what {
                    Watched::Listener => self.on_accept(),
                    Watched::Client(fd) => self.on_client_read(fd),
                    Watched::Session(index) => self.on_session_read(index),
                    Watched::Solo => self.on_solo_read(),
                    Watched::Seating(index) => self.on_seating_read(index),
                    Watched::Source(index) => self.on_source_read(index),
                    Watched::Motion(index) => self.on_motion_read(index),
                    Watched::Dsu => self.on_dsu_read(),
                    Watched::Clone(index) => {
                        if let Some(republisher) = self.republisher.as_mut() {
                            republisher.feedback(index);
                        }
                    }
                    Watched::Tick => {
                        self.reactor.take_tick();
                        self.guarded_tick();
                    }
                }
            }
        }
    }

    pub fn close(&mut self) {
        self.held.release_all();
        self.close_seating();
        self.end_session();
        self.release_solo();
        self.stop_republisher();
        let fds: Vec<i32> = self.clients.keys().copied().collect();
        for fd in fds {
            self.drop_client(fd);
        }
        if let Some(listener) = self.listener.take() {
            let _ = self.reactor.unwatch(listener.as_fd());
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }

    fn on_accept(&mut self) {
        loop {
            let accepted = match self.listener.as_ref() {
                Some(listener) => listener.accept(),
                None => return,
            };
            let stream = match accepted {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(error) => {
                    warn!("accept: {error}");
                    return;
                }
            };
            if stream.set_nonblocking(true).is_err() {
                continue;
            }
            let fd = stream.as_raw_fd();
            if let Err(error) = self.reactor.watch(stream.as_fd(), Watched::Client(fd)) {
                warn!("could not watch a client: {error}");
                continue;
            }
            self.clients.insert(
                fd,
                Client {
                    stream,
                    reader: LineReader::new(),
                    connected_at: now(),
                    pending: Vec::new(),
                },
            );
            info!("client connected ({} total)", self.clients.len());
            let greeting = self.state_event();
            self.send(fd, &greeting);
        }
    }

    fn on_client_read(&mut self, fd: i32) {
        let mut buffer = vec![0u8; 65536];
        let read = match self.clients.get_mut(&fd) {
            Some(client) => client.stream.read(&mut buffer),
            None => return,
        };
        let count = match read {
            Ok(0) => {
                self.drop_client(fd);
                return;
            }
            Ok(count) => count,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::Interrupted =>
            {
                return
            }
            Err(_) => {
                self.drop_client(fd);
                return;
            }
        };
        let messages = match self.clients.get_mut(&fd) {
            Some(client) => client.reader.feed(&buffer[..count]),
            None => return,
        };
        for message in messages {
            self.handle_command(fd, Value::Object(message));
        }
    }

    fn drop_client(&mut self, fd: i32) {
        log::debug!(
            "dropping client {fd} (pending {})",
            self.clients.get(&fd).map(|c| c.pending.len()).unwrap_or(0)
        );
        if let Some(client) = self.clients.remove(&fd) {
            let _ = self.reactor.unwatch(client.stream.as_fd());
        }
        // A session holds every pad and a modal flow holds one; neither may outlive its client.
        if self.clients.is_empty() && (self.session.is_some() || self.modal_open()) {
            info!("last client disconnected mid-session; releasing pads");
            self.cancel();
        }
    }

    fn send(&mut self, fd: i32, message: &Value) {
        match self.clients.get_mut(&fd) {
            Some(client) => client.pending.extend_from_slice(&wire::encode(message)),
            None => return,
        }
        self.flush(fd);
    }

    /// Write pending bytes.
    fn flush(&mut self, fd: i32) {
        let gone = {
            let Some(client) = self.clients.get_mut(&fd) else {
                return;
            };
            let mut written = 0;
            let mut failed = false;
            while written < client.pending.len() {
                match client.stream.write(&client.pending[written..]) {
                    Ok(0) => {
                        failed = true;
                        break;
                    }
                    Ok(count) => written += count,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
            client.pending.drain(..written);
            failed || client.pending.len() > MAX_PENDING
        };
        if gone {
            self.drop_client(fd);
        }
    }

    fn flush_all(&mut self) {
        for fd in self.clients.keys().copied().collect::<Vec<_>>() {
            if self.clients.get(&fd).is_some_and(|c| !c.pending.is_empty()) {
                self.flush(fd);
            }
        }
    }

    fn broadcast(&mut self, message: &Value) {
        let fds: Vec<i32> = self.clients.keys().copied().collect();
        for fd in fds {
            self.send(fd, message);
        }
    }

    fn handle_command(&mut self, fd: i32, message: Value) {
        let name = message
            .get("cmd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let parsed = Command::parse(&message);
        let result = catch_unwind(AssertUnwindSafe(|| match parsed {
            Ok(command) => {
                self.dispatch(fd, command);
                self.sync_republish_pause();
                None
            }
            Err(Refused::Unknown(_)) => Some(format!("unknown command {name:?}")),
            Err(refused) => Some(format!("{name:?} failed: {refused}")),
        }));
        let failure = match result {
            Ok(None) => return,
            Ok(Some(message)) => message,
            Err(panic) => {
                let what = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_else(|| "panic".to_owned());
                warn!("command {name:?} failed: {what}");
                format!("{name:?} failed: {what}")
            }
        };
        self.send(fd, &events::error(failure));
    }

    fn dispatch(&mut self, fd: i32, command: Command) {
        match command {
            Command::Begin { players } => self.begin(players.max(1) as u32),
            Command::Reset => self.reset(),
            Command::Accept => self.accept(),
            Command::Cancel => self.cancel(),
            Command::Map {
                player,
                layout,
                scope,
            } => self.begin_mapping(as_player(player), &layout, &scope),
            Command::ChooseLayout { player } => self.begin_layout_choice(as_player(player)),
            Command::ChooseScope { player } => self.begin_scope_choice(as_player(player)),
            Command::MapForGame {
                player,
                console,
                key,
                title,
            } => self.begin_game_scope_choice(as_player(player), &console, &key, &title),
            Command::ForgetPad { player } => self.forget_pad(as_player(player)),
            Command::SkipControl => self.skip_control(),
            Command::Calibrate { player } => self.begin_calibration(as_player(player)),
            Command::ConfigureEnd => {
                if self.calibration.is_some() {
                    self.finish_calibration();
                }
                if self.choice.is_some() {
                    self.end_layout_choice();
                }
                if self.mapping.is_some() {
                    self.finish_mapping(false);
                }
            }
            Command::SetIcon { player, icon } => self.set_icon(as_player(player), &icon),
            Command::Tune {
                player,
                signature,
                request,
            } => self.tune(fd, as_player(player), &signature, &request),
            Command::Seating { open, players } => {
                if open {
                    self.seating
                        .open(players.clamp(1, i64::from(u16::MAX)) as u32);
                    info!("seating open: {} seat(s)", self.seating.seats());
                    self.refresh_seating(&mut Scan::default());
                } else {
                    self.close_seating();
                    info!("seating closed");
                }
                let state = self.state_event();
                self.broadcast(&state);
            }
            Command::Unseat { player } => self.unseat(as_player(player)),
            Command::SeatKeyboard => self.seat_keyboard(),
            Command::Status => {
                let state = self.state_event();
                self.send(fd, &state);
                let lines = events::sdl_mapping(&self.sdl_lines);
                self.send(fd, &lines);
            }
        }
    }

    fn begin(&mut self, players: u32) {
        self.release_solo();
        self.stop_republisher();
        self.end_session();

        let pads = discover();
        if pads.is_empty() {
            self.broadcast(&events::error("no joypads found"));
            return;
        }
        self.icon_overrides = runtime::load_icon_overrides();

        // Seating must release pads before session grabs them.
        for source in self.seating.sources() {
            let _ = self.reactor.unwatch(source.as_fd());
        }
        self.seating.refresh(Vec::new());
        let session = match Session::open(pads) {
            Ok(session) => session,
            Err(error) => {
                self.broadcast(&events::error(error.to_string()));
                self.resume_republishing();
                let state = self.state_event();
                self.broadcast(&state);
                return;
            }
        };
        for (index, source) in session.sources().iter().enumerate() {
            if let Err(error) = self.reactor.watch(source.as_fd(), Watched::Session(index)) {
                warn!("could not watch session pad {index}: {error}");
            }
        }
        let count = session.len();
        let grab_failures = session.grab_failures.clone();
        self.session = Some(session);
        self.calibration = None;
        self.choice = None;
        self.mapping = None;
        self.slots = players;
        self.confirm.clear();
        self.last_progress = 0.0;
        self.last_confirm = 0.0;
        self.state = STATE_ASSIGNING;
        info!(
            "session open: {count} pad(s), {} slot(s), {} already assigned",
            self.slots,
            self.slots_assigned.len()
        );
        if !grab_failures.is_empty() {
            warn!(
                "session: {} pad(s) not grabbed exclusively ({}) -- presses also reach the front-end",
                grab_failures.len(),
                grab_failures.join(", ")
            );
        }
        self.broadcast(&events::pads(count));
        let state = self.state_event();
        self.broadcast(&state);
    }

    fn resume_republishing(&mut self) {
        if self.republisher.is_none() && !self.slots_assigned.is_empty() {
            if let Err(error) = self.start_republisher() {
                warn!("could not resume republishing: {error}");
            }
        }
        self.state = if self.republisher.is_some() {
            STATE_READY
        } else {
            STATE_IDLE
        };
    }

    fn reset(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.reset();
        self.confirm.clear();
        let state = self.state_event();
        self.broadcast(&state);
    }

    fn cancel(&mut self) {
        let had_session = self.session.is_some();
        if let Some(session) = &self.session {
            info!(
                "session cancelled after {} claim(s)",
                session.claims().len()
            );
        }
        if self.choice.is_some() {
            self.end_layout_choice();
        }
        if self.mapping.is_some() {
            self.finish_mapping(false);
        }
        self.end_session();
        self.calibration = None;
        if had_session {
            self.resume_republishing();
        }
        let state = self.state_event();
        self.broadcast(&state);
    }

    /// Drop one seat, or every seat for player 0. The clone stops, the pad is
    /// released, consumers are rewritten, and the seat is free to be taken
    /// again by a hold -- seating is left exactly as it was, because "hold a
    /// button to take a seat" is the next thing that happens.
    fn unseat(&mut self, player: u32) {
        if self.session.is_some() {
            self.broadcast(&events::error("a session is open; cancel it first"));
            return;
        }
        if self.modal_open() {
            self.broadcast(&events::error(
                "a controller is being set up; finish that first",
            ));
            return;
        }
        let targets: Vec<u32> = self
            .taken_seats()
            .into_iter()
            .filter(|seat| player == 0 || *seat == player)
            .collect();
        if targets.is_empty() {
            let why = if player == 0 {
                "nobody is seated".to_owned()
            } else {
                format!("no controller assigned to player {player}")
            };
            self.broadcast(&events::error(why));
            return;
        }
        let removed: Vec<Slot> = self
            .slots_assigned
            .iter()
            .filter(|slot| targets.contains(&slot.player))
            .cloned()
            .collect();
        self.slots_assigned
            .retain(|slot| !targets.contains(&slot.player));
        self.away.retain(|entry| !targets.contains(&entry.player));
        if self
            .keyboard_seat
            .is_some_and(|seat| targets.contains(&seat))
        {
            info!(
                "player {}: the keyboard unseated",
                self.keyboard_seat.unwrap_or_default()
            );
            self.keyboard_seat = None;
        }
        for slot in &removed {
            self.attached
                .live
                .remove(&profiles::signature_of(&slot.pad));
            info!("player {}: unseated {}", slot.player, clean(&slot.pad.name));
        }
        for seat in targets
            .iter()
            .filter(|seat| !removed.iter().any(|slot| slot.player == **seat))
        {
            info!("player {seat}: unseated (was away)");
        }
        if self.slots_assigned.is_empty() {
            self.stop_republisher();
            self.rewrite_consumers(&BTreeMap::new());
            self.state = STATE_IDLE;
        } else {
            if let Err(error) = self.start_republisher() {
                warn!("could not republish after unseating: {error}");
            }
            self.state = if self.republisher.is_some() {
                STATE_READY
            } else {
                STATE_IDLE
            };
        }
        self.save_assignments();
        for slot in &removed {
            let event =
                self.controller_event(announce::ACTION_REMOVED, slot.player, &slot.pad, "unseated");
            self.broadcast(&event);
        }
        self.refresh_seating(&mut Scan::default());
        let state = self.state_event();
        self.broadcast(&state);
    }

    fn accept(&mut self) {
        let claims: Vec<Slot> = match &self.session {
            Some(session) => session
                .claimed_pads()
                .into_iter()
                .map(|(player, pad)| Slot {
                    player,
                    pad: pad.clone(),
                })
                .collect(),
            None => Vec::new(),
        };
        if claims.is_empty() {
            self.broadcast(&events::error("nothing assigned yet"));
            return;
        }
        self.slots_assigned = claims;
        self.end_session();
        self.save_assignments();
        if let Err(error) = self.start_republisher() {
            warn!("could not start republishing: {error}");
            self.broadcast(&events::error(format!(
                "a controller went away before it could be published: {error}"
            )));
        }
        self.state = if self.republisher.is_some() {
            STATE_READY
        } else {
            STATE_IDLE
        };
        let accepted = events::accepted(
            self.players_payload(),
            &self.launch_config_path.display().to_string(),
        );
        self.broadcast(&accepted);
        let state = self.state_event();
        self.broadcast(&state);
    }

    fn end_session(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        for source in session.sources() {
            let _ = self.reactor.unwatch(source.as_fd());
        }
        drop(session);
        self.confirm.clear();
    }

    fn pad_for_player(&self, player: u32) -> Option<Pad> {
        if let Some(session) = &self.session {
            if let Some(pad) = session.claimed_pads().get(&player) {
                return Some((*pad).clone());
            }
        }
        self.slots_assigned
            .iter()
            .find(|slot| slot.player == player)
            .map(|slot| slot.pad.clone())
    }

    fn session_pad(&self, player: u32, what: &str) -> Result<(Pad, usize), Value> {
        let Some(session) = &self.session else {
            return Err(events::error(format!("{what} needs an open session")));
        };
        let Some(pad) = self.pad_for_player(player) else {
            return Err(events::error(format!(
                "no controller assigned to player {player}"
            )));
        };
        let Some(index) = session.index_of(&pad.path) else {
            return Err(events::error("controller is no longer open"));
        };
        Ok((pad, index))
    }

    /// A player's pad, open for a modal flow: in the session if one is open,
    /// otherwise through its clone's source, otherwise grabbed on its own.
    fn modal_pad(&mut self, player: u32, what: &str) -> Result<(Pad, Opened), Value> {
        if self.session.is_some() {
            return self
                .session_pad(player, what)
                .map(|(pad, index)| (pad, Opened::Session(index)));
        }
        let Some(pad) = self.pad_for_player(player) else {
            return Err(events::error(format!(
                "no controller assigned to player {player}"
            )));
        };
        let published = self.republisher.as_ref().and_then(|republisher| {
            republisher
                .pads
                .iter()
                .position(|vpad| vpad.pad.path == pad.path && !vpad.gone)
        });
        if let Some(index) = published {
            return Ok((pad, Opened::Published(index)));
        }
        if self
            .solo
            .as_ref()
            .is_some_and(|solo| solo.pad.path == pad.path)
        {
            return Ok((pad, Opened::Solo));
        }
        self.release_solo();
        let source = clone::open_source(&pad, true).map_err(|error| {
            events::error(format!(
                "{what}: could not open {}: {error}",
                clean(&pad.name)
            ))
        })?;
        if let Err(error) = self.reactor.watch(source.as_fd(), Watched::Solo) {
            return Err(events::error(format!(
                "{what}: could not watch {}: {error}",
                clean(&pad.name)
            )));
        }
        info!(
            "player {player}: {} grabbed on its own for {what}",
            clean(&pad.name)
        );
        self.solo = Some(Solo {
            pad: pad.clone(),
            source,
            buffer: Vec::with_capacity(64),
        });
        Ok((pad, Opened::Solo))
    }

    fn opened_source(&mut self, opened: Opened) -> Option<&mut Source> {
        match opened {
            Opened::Session(index) => self
                .session
                .as_mut()
                .and_then(|session| session.source_mut(index)),
            Opened::Published(index) => self
                .republisher
                .as_mut()
                .and_then(|republisher| republisher.pads.get_mut(index))
                .map(|vpad| &mut vpad.source),
            Opened::Solo => self.solo.as_mut().map(|solo| &mut solo.source),
        }
    }

    fn modal_open(&self) -> bool {
        self.mapping.is_some() || self.choice.is_some() || self.calibration.is_some()
    }

    /// The pads modal flows are reading right now.
    fn modal_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::with_capacity(3);
        if let Some(modal) = &self.mapping {
            paths.push(modal.pad_path.clone());
        }
        if let Some(modal) = &self.choice {
            paths.push(modal.pad_path.clone());
        }
        if let Some(run) = &self.calibration {
            paths.push(PathBuf::from(&run.pad_path));
        }
        paths
    }

    /// Let go of a pad grabbed on its own; dropping the descriptor ungrabs it.
    fn release_solo(&mut self) {
        let Some(solo) = self.solo.take() else {
            return;
        };
        let _ = self.reactor.unwatch(solo.source.as_fd());
        info!("{} released", clean(&solo.pad.name));
        drop(solo);
    }

    fn held_keys(&mut self, opened: Opened) -> BTreeSet<u16> {
        self.opened_source(opened)
            .map(|source| source.held_keys().into_iter().collect())
            .unwrap_or_default()
    }

    /// Get axis ranges and resting positions.
    fn absolute_ranges(&mut self, opened: Opened) -> BTreeMap<u16, padmap_core::sdl::AxisSpan> {
        self.opened_source(opened)
            .map(|source| source.axis_spans())
            .unwrap_or_default()
            .into_iter()
            .map(|(code, span)| {
                let rest = if span.minimum <= span.rest && span.rest <= span.maximum {
                    span.rest
                } else {
                    (span.minimum + span.maximum).div_euclid(2)
                };
                (
                    code,
                    padmap_core::sdl::AxisSpan::new(span.minimum, span.maximum, rest),
                )
            })
            .collect()
    }

    fn begin_calibration(&mut self, player: u32) {
        let (pad, opened) = match self.modal_pad(player, "calibration") {
            Ok(found) => found,
            Err(event) => {
                self.broadcast(&event);
                return;
            }
        };
        let declared = self
            .opened_source(opened)
            .map(|source| source.declared_axes())
            .unwrap_or_default();
        let axes: BTreeMap<u16, padmap_core::calibration::Declared> = declared
            .into_iter()
            .filter(|(code, axis)| axis.calibratable(*code))
            .collect();
        let name = clean(&pad.name);
        if axes.is_empty() {
            publish::store_calibration(&pad, BTreeMap::new());
            let mut run = CalibrationRun::new(
                player,
                pad.path.display().to_string(),
                BTreeMap::new(),
                now(),
            );
            run.begin_phase(Phase::Icon, now());
            self.calibration = Some(run);
            self.confirm.clear();
            self.broadcast(&events::calibration(
                Phase::Icon.as_str(),
                1.0,
                player,
                Some(&name),
                Some(0),
            ));
            return;
        }
        let run = CalibrationRun::new(player, pad.path.display().to_string(), axes, now());
        self.calibration = Some(run);
        self.confirm.clear();
        self.emit_calibration();
    }

    fn emit_calibration(&mut self) {
        let Some(run) = &self.calibration else {
            return;
        };
        let name = self
            .pad_for_player(run.player)
            .map(|pad| clean(&pad.name))
            .unwrap_or_default();
        let event = events::calibration(
            run.phase.as_str(),
            run.fraction(now()),
            run.player,
            Some(&name),
            None,
        );
        self.broadcast(&event);
    }

    fn tick_calibration(&mut self) {
        let Some(run) = self.calibration.as_mut() else {
            return;
        };
        match run.tick(now()) {
            Step::Quiet => {}
            Step::Progress | Step::Advanced => self.emit_calibration(),
            Step::Measured(axes) => {
                let player = run.player;
                let count = axes.len();
                if let Some(pad) = self.pad_for_player(player) {
                    publish::store_calibration(&pad, axes);
                }
                self.broadcast(&events::calibration(
                    Phase::Icon.as_str(),
                    1.0,
                    player,
                    None,
                    Some(count),
                ));
            }
        }
    }

    fn finish_calibration(&mut self) {
        let run = self.calibration.take();
        self.confirm.clear();
        self.last_confirm = 0.0;
        let player = run.map(|r| r.player).unwrap_or(0);
        self.broadcast(&events::calibration("done", 1.0, player, None, None));
        let state = self.state_event();
        self.broadcast(&state);
    }

    fn begin_layout_choice(&mut self, player: u32) {
        let (pad, opened) = match self.modal_pad(player, "choosing a layout") {
            Ok(found) => found,
            Err(event) => {
                self.broadcast(&event);
                return;
            }
        };
        let axes = self.absolute_ranges(opened);
        let held = self.held_keys(opened);
        let stored = publish::stored_layout(&pad);
        let guess = if stored.is_empty() {
            publish::icon_for(&pad, &self.icon_overrides).to_owned()
        } else {
            stored
        };
        let options = capture::layout_options(&publish::mapped_layouts(&pad));
        let mut chooser = Chooser::new(
            player,
            options,
            capture::ChoiceKind::Layout,
            "Which controller is this?".to_owned(),
            axes,
            held,
        );
        chooser.move_by(padmap_core::layout::index_of(&guess) as i32);
        self.confirm.clear();
        self.broadcast(&events::layout_choice(&chooser));
        self.choice = Some(Modal {
            run: chooser,
            pad_path: pad.path,
        });
    }

    fn begin_scope_choice(&mut self, player: u32) {
        let (pad, opened) = match self.modal_pad(player, "choosing a scope") {
            Ok(found) => found,
            Err(event) => {
                self.broadcast(&event);
                return;
            }
        };
        let scopes: BTreeSet<String> = profiles::load(&pad, None)
            .map(|profile| profile.mappings.keys().cloned().collect())
            .unwrap_or_default();
        let stored = publish::stored_layout(&pad);
        let guess = if stored.is_empty() {
            publish::icon_for(&pad, &self.icon_overrides).to_owned()
        } else {
            stored
        };
        let recent: Vec<capture::RecentGame> = runtime::read_recent_games()
            .into_iter()
            .map(|game| (game.console, game.key, game.title))
            .collect();
        let options = capture::scope_options(&scopes, &guess, &recent);
        let axes = self.absolute_ranges(opened);
        let held = self.held_keys(opened);
        self.pending_scope.clear();
        let chooser = Chooser::new(
            player,
            options,
            capture::ChoiceKind::Scope,
            "What is this mapping for?".to_owned(),
            axes,
            held,
        );
        self.confirm.clear();
        self.broadcast(&events::layout_choice(&chooser));
        self.choice = Some(Modal {
            run: chooser,
            pad_path: pad.path,
        });
    }

    fn begin_game_scope_choice(&mut self, player: u32, console: &str, key: &str, title: &str) {
        let (pad, opened) = match self.modal_pad(player, "mapping") {
            Ok(found) => found,
            Err(event) => {
                self.broadcast(&event);
                return;
            }
        };
        let scopes: BTreeSet<String> = profiles::load(&pad, None)
            .map(|profile| profile.mappings.keys().cloned().collect())
            .unwrap_or_default();
        let options = capture::game_scope_options(console, key, title, &scopes);
        if options.is_empty() {
            self.broadcast(&events::error("no console known for this game"));
            return;
        }
        let axes = self.absolute_ranges(opened);
        let held = self.held_keys(opened);
        self.pending_scope.clear();
        let chooser = Chooser::new(
            player,
            options,
            capture::ChoiceKind::Scope,
            if title.is_empty() {
                "What is this mapping for?".to_owned()
            } else {
                format!("Map for {title}?")
            },
            axes,
            held,
        );
        self.confirm.clear();
        self.broadcast(&events::layout_choice(&chooser));
        self.choice = Some(Modal {
            run: chooser,
            pad_path: pad.path,
        });
    }

    fn choice_confirmed(&mut self) {
        let Some(modal) = self.choice.take() else {
            return;
        };
        let player = modal.run.player;
        let chosen = modal.run.chosen().to_owned();
        let layout = modal.run.chosen_layout().to_owned();
        let kind = modal.run.kind;
        self.choice = Some(modal);
        self.end_layout_choice();

        if kind == capture::ChoiceKind::Layout {
            let scope = std::mem::take(&mut self.pending_scope);
            self.begin_mapping(player, &chosen, &scope);
            return;
        }
        self.pending_scope = chosen.clone();
        if chosen == scope::UNIVERSAL {
            self.begin_layout_choice(player);
        } else {
            self.begin_mapping(player, &layout, &chosen);
        }
    }

    fn end_layout_choice(&mut self) {
        let modal = self.choice.take();
        self.confirm.clear();
        self.last_confirm = 0.0;
        self.broadcast(&events::layout_choice_ended(modal.as_ref().map(|m| &m.run)));
        let state = self.state_event();
        self.broadcast(&state);
    }

    fn forget_pad(&mut self, player: u32) {
        let Some(pad) = self.pad_for_player(player) else {
            self.broadcast(&events::error(format!(
                "no controller assigned to player {player}"
            )));
            return;
        };
        let removed = profiles::forget(&pad, None);
        let signature = profiles::signature_of(&pad);
        if self.prompted.remove(&signature) {
            self.save_prompted();
        }
        info!(
            "player {player}: forgot {} ({})",
            clean(&pad.name),
            if removed {
                "profile removed"
            } else {
                "nothing stored"
            }
        );
        self.begin_layout_choice(player);
    }

    fn begin_mapping(&mut self, player: u32, layout_id: &str, scope: &str) {
        let (pad, opened) = match self.modal_pad(player, "mapping") {
            Ok(found) => found,
            Err(event) => {
                self.broadcast(&event);
                return;
            }
        };
        let chosen = if layout_id.is_empty() {
            publish::icon_for(&pad, &self.icon_overrides).to_owned()
        } else {
            layout_id.to_owned()
        };
        let layout = padmap_core::layout::for_icon(&chosen);
        let mut keys: Vec<u16> = self
            .opened_source(opened)
            .map(|source| source.capabilities().0)
            .unwrap_or_default();
        keys.sort_unstable();
        let axes = self.absolute_ranges(opened);
        let held = self.held_keys(opened);
        let stored = publish::stored_mapping(&pad, scope, &layout.id);
        let run =
            MappingRun::new(player, layout, keys, scope.to_owned(), axes, held).seeded(&stored);
        self.pending_scope.clear();
        self.confirm.clear();
        self.last_finish = 0.0;
        info!(
            "mapping {} as {} for scope {scope:?}{}",
            clean(&pad.name),
            layout.id,
            if stored.is_empty() {
                String::new()
            } else {
                format!(", starting from {} stored control(s)", stored.len())
            }
        );
        self.broadcast(&events::mapping(&run));
        self.mapping = Some(Modal {
            run,
            pad_path: pad.path,
        });
    }

    fn skip_control(&mut self) {
        let Some(modal) = self.mapping.as_mut() else {
            return;
        };
        modal.run.skip(now());
        let event = events::mapping(&modal.run);
        let finished = modal.run.finished();
        self.broadcast(&event);
        if finished {
            self.finish_mapping(true);
        }
    }

    fn finish_mapping(&mut self, store: bool) {
        let modal = self.mapping.take();
        self.confirm.clear();
        self.last_confirm = 0.0;
        self.pending_scope.clear();
        if self.last_finish != 0.0 {
            self.last_finish = 0.0;
            if let Some(modal) = &modal {
                self.broadcast(&events::finish(modal.run.player, 0.0));
            }
        }
        if let Some(modal) = &modal {
            if store && !modal.run.bindings().is_empty() {
                if let Some(pad) = self.pad_for_player(modal.run.player) {
                    let missing = publish::store_mapping(
                        &pad,
                        &modal.run.layout.id,
                        modal.run.bindings(),
                        &modal.run.scope,
                    );
                    info!(
                        "mapped {} for scope {:?}: {} control(s){}",
                        clean(&pad.name),
                        modal.run.scope,
                        modal.run.bindings().len(),
                        if missing.is_empty() {
                            String::new()
                        } else {
                            format!("; {} unmapped: {}", missing.len(), missing.join(", "))
                        }
                    );
                }
            }
        }
        self.broadcast(&events::mapping_done(modal.as_ref().map(|m| &m.run), store));
        let state = self.state_event();
        self.broadcast(&state);
    }

    fn set_icon(&mut self, player: u32, icon: &str) {
        if !padmap_core::icons::known(icon) {
            self.broadcast(&events::error(format!("unknown icon {icon:?}")));
            return;
        }
        let Some(pad) = self.pad_for_player(player) else {
            return;
        };
        publish::store_icon(&pad, icon);
        if self
            .calibration
            .as_ref()
            .is_some_and(|run| run.player == player)
        {
            self.finish_calibration();
        } else {
            let state = self.state_event();
            self.broadcast(&state);
        }
    }

    fn tune(&mut self, fd: i32, player: u32, signature: &str, request: &Request) {
        let pad = match self.pad_for_player(player).or_else(|| {
            (!signature.is_empty())
                .then(|| pad::discover(pad::Filter::default()).unwrap_or_default())
                .and_then(|pads| {
                    pads.into_iter()
                        .find(|pad| profiles::signature_of(pad) == signature)
                })
        }) {
            Some(pad) => pad,
            None => {
                self.send(
                    fd,
                    &events::error(if player > 0 {
                        format!("player {player} has no controller")
                    } else {
                        format!("no controller with signature {signature:?} is plugged in")
                    }),
                );
                return;
            }
        };
        let declared: Vec<u16> = self
            .republisher
            .as_ref()
            .and_then(|republisher| {
                republisher
                    .pads
                    .iter()
                    .find(|vpad| vpad.pad.path == pad.path)
                    .map(|vpad| vpad.declared.keys().copied().collect())
            })
            .or_else(|| {
                clone::open_source(&pad, false)
                    .ok()
                    .map(|source| source.declared_axes().keys().copied().collect())
            })
            .unwrap_or_default();
        if request.deadzone_all.is_some() && declared.is_empty() {
            self.send(
                fd,
                &events::error(format!(
                    "{}: could not read its axes to set a deadzone on them",
                    clean(&pad.name)
                )),
            );
            return;
        }
        let tuning = request.apply(&publish::tuning_for(&pad), &declared);
        if let Err(error) = publish::store_tuning(&pad, tuning.clone()) {
            self.send(
                fd,
                &events::error(format!(
                    "could not save tuning for {}: {error}",
                    clean(&pad.name)
                )),
            );
            return;
        }
        info!("tuned {}: {tuning:?}", clean(&pad.name));
        // Swap into running clone; rebuilding would disconnect games.
        if let Some(republisher) = self.republisher.as_mut() {
            if let Some(index) = republisher
                .pads
                .iter()
                .position(|vpad| vpad.pad.path == pad.path)
            {
                republisher.retune(index, tuning.clone());
            }
        }
        let seat = self
            .slots_assigned
            .iter()
            .find(|slot| slot.pad.path == pad.path)
            .map(|slot| slot.player)
            .unwrap_or(0);
        self.broadcast(&events::tuned(seat, &profiles::signature_of(&pad), &tuning));
    }

    fn on_session_read(&mut self, index: usize) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let raw = session.read(index);
        if session.is_gone(index) {
            // A dead node reports readable forever; stop asking.
            if let Some(source) = session.sources().get(index) {
                let _ = self.reactor.unwatch(source.as_fd());
            }
            let count = session.present();
            self.broadcast(&events::pads(count));
            return;
        }
        if raw.is_empty() {
            return;
        }
        let pad_path = session.pads.get(index).map(|pad| pad.path.clone());
        let claimed = session.is_claimed(index);
        let clock = now();
        for event in &raw {
            if let Some(path) = &pad_path {
                self.feed_modal(path, *event, clock);
            }
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if claimed {
            if let Some(path) = &pad_path {
                let path = path.display().to_string();
                for event in raw.iter().filter(|e| e.is_press_or_release()) {
                    self.confirm.feed(&path, event.code, event.value, clock);
                }
            }
            return;
        }
        for event in &raw {
            session
                .assigner
                .feed(index, event.kind, event.code, event.value, clock);
        }
    }

    fn feed_modal(&mut self, pad_path: &Path, event: Raw, clock: f64) {
        if let Some(run) = self.calibration.as_mut() {
            if run.pad_path == pad_path.display().to_string() {
                run.feed(event.kind, event.code, event.value);
            }
        }
        if !event.is_input() {
            return;
        }
        let capture_event = capture::Event {
            kind: event.kind,
            code: event.code,
            value: event.value,
        };
        if let Some(modal) = self.choice.as_mut() {
            if modal.pad_path == pad_path && modal.run.feed(capture_event, clock) {
                if modal.run.confirmed() {
                    self.choice_confirmed();
                } else {
                    let update = events::layout_choice(&modal.run);
                    self.broadcast(&update);
                }
            }
        }
        let pressed = self
            .mapping
            .as_ref()
            .filter(|modal| modal.pad_path == pad_path)
            .map(|modal| (modal.run.player, modal.run.describe(capture_event)));
        if let Some((player, Some(pressed))) = pressed {
            let event = events::input(player, pressed);
            if self.last_input.as_ref() != Some(&event) {
                self.last_input = Some(event.clone());
                self.broadcast(&event);
            }
        }
        if let Some(modal) = self.mapping.as_mut() {
            if modal.pad_path == pad_path {
                let before = modal.run.conflict();
                let outcome = modal.run.feed(capture_event, clock);
                if outcome == Outcome::Finished {
                    self.finish_by_hold();
                } else if outcome.advanced() {
                    let update = events::mapping(&modal.run);
                    let finished = modal.run.finished();
                    self.broadcast(&update);
                    if finished {
                        self.finish_mapping(true);
                    }
                } else if modal.run.conflict().is_some() && modal.run.conflict() != before {
                    let update = events::mapping(&modal.run);
                    self.broadcast(&update);
                }
            }
        }
    }

    /// The finish hold reached its tier: the ring is full, keep what is bound.
    fn finish_by_hold(&mut self) {
        let Some(player) = self.mapping.as_ref().map(|modal| modal.run.player) else {
            return;
        };
        info!("player {player}: finished the wizard from the pad");
        self.last_finish = 1.0;
        self.broadcast(&events::finish(player, 1.0));
        self.last_finish = 0.0;
        self.finish_mapping(true);
    }

    /// Time passing under the wizard: the finish hold fills, and ends the run when full.
    fn tick_mapping(&mut self, clock: f64) {
        let Some(modal) = self.mapping.as_mut() else {
            return;
        };
        if modal.run.tick(clock) == Outcome::Finished {
            self.finish_by_hold();
            return;
        }
        let player = modal.run.player;
        let fraction = modal.run.finish_hold(clock).unwrap_or(0.0);
        if fraction != self.last_finish {
            self.last_finish = fraction;
            self.broadcast(&events::finish(player, fraction));
        }
    }

    fn on_solo_read(&mut self) {
        let clock = now();
        let (path, raw) = {
            let Some(solo) = self.solo.as_mut() else {
                return;
            };
            solo.buffer.clear();
            match solo.source.fetch_events(&mut solo.buffer) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(error) => {
                    warn!(
                        "{} went away under a modal flow ({error}); ending it",
                        clean(&solo.pad.name)
                    );
                    self.release_solo();
                    self.end_modals();
                    return;
                }
            }
            (solo.pad.path.clone(), raw_events(&solo.buffer))
        };
        for event in raw {
            self.feed_modal(&path, event, clock);
        }
    }

    /// A published pad under a modal flow: its presses go to the flow, not to its clone.
    fn read_published_for_modal(&mut self, index: usize) {
        let clock = now();
        let (path, raw) = {
            let Some(republisher) = self.republisher.as_mut() else {
                return;
            };
            let Some(vpad) = republisher.pads.get_mut(index) else {
                return;
            };
            self.scratch.clear();
            match vpad.source.fetch_events(&mut self.scratch) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(error) => {
                    warn!(
                        "player {}: source disappeared under a modal flow ({error})",
                        vpad.player
                    );
                    vpad.gone = true;
                    let _ = self.reactor.unwatch(vpad.source.as_fd());
                    self.end_modals();
                    return;
                }
            }
            (vpad.pad.path.clone(), raw_events(&self.scratch))
        };
        for event in raw {
            self.feed_modal(&path, event, clock);
        }
    }

    /// End every modal flow without keeping anything, as `cancel` does.
    fn end_modals(&mut self) {
        if self.choice.is_some() {
            self.end_layout_choice();
        }
        if self.mapping.is_some() {
            self.finish_mapping(false);
        }
        if self.calibration.is_some() {
            self.finish_calibration();
        }
    }

    fn on_seating_read(&mut self, index: usize) {
        let clock = now();
        self.seating.read(index, clock);
    }

    fn close_seating(&mut self) {
        for source in self.seating.sources() {
            let _ = self.reactor.unwatch(source.as_fd());
        }
        self.seating.close();
    }

    fn refresh_seating(&mut self, scan: &mut Scan) {
        if !self.seating.is_open() {
            return;
        }
        if self.state == STATE_ASSIGNING {
            if !self.seating.pads().is_empty() {
                for source in self.seating.sources() {
                    let _ = self.reactor.unwatch(source.as_fd());
                }
                self.seating.refresh(Vec::new());
            }
            return;
        }
        let seated: Vec<PathBuf> = self
            .slots_assigned
            .iter()
            .map(|slot| slot.pad.path.clone())
            .collect();
        let wanted = self.seating.wanted(scan.pads(), &seated);
        // Only touch epoll when the set actually changes; the unwatch/rewatch
        // churn otherwise ran every tick for no reason.
        if !self.seating.would_change(&wanted) {
            return;
        }
        for source in self.seating.sources() {
            let _ = self.reactor.unwatch(source.as_fd());
        }
        self.seating.refresh(wanted);
        for (index, source) in self.seating.sources().iter().enumerate() {
            if let Err(error) = self.reactor.watch(source.as_fd(), Watched::Seating(index)) {
                warn!("seating: could not watch pad {index}: {error}");
            }
        }
        debug!(
            "seating: listening on {:?}",
            self.seating
                .pads()
                .iter()
                .map(|pad| pad.event())
                .collect::<Vec<_>>()
        );
    }

    fn tick_seating(&mut self) {
        if !self.seating.is_open() || self.state == STATE_ASSIGNING {
            return;
        }
        let claimed = self.seating.tick(now());
        for (index, fraction) in &claimed.progress {
            if let Some(pad) = self.seating.pads().get(*index) {
                let _ = pad;
                self.broadcast(&events::progress(*fraction));
            }
        }
        for index in claimed.pads {
            let Some(pad) = self.seating.pads().get(index).cloned() else {
                continue;
            };
            let player = padmap_core::announce::next_player(&self.taken_seats());
            if player > self.seating.seats() {
                info!("{} held a button but every seat is taken", clean(&pad.name));
                self.seating.reset();
                continue;
            }
            info!(
                "seating: player {player} <- {} ({})",
                clean(&pad.name),
                pad.event()
            );
            self.slots_assigned.push(Slot {
                player,
                pad: pad.clone(),
            });
            self.attached
                .live
                .insert(profiles::signature_of(&pad), player);
            let event = events::claim(
                player,
                &clean(&pad.name),
                pad.event(),
                publish::icon_for(&pad, &self.icon_overrides),
                profiles::is_known(&pad, None),
            );
            self.broadcast(&event);
            if let Err(error) = self.start_republisher() {
                warn!(
                    "seating: could not republish after {} joined: {error}",
                    clean(&pad.name)
                );
            }
            if self.republisher.is_some() {
                self.state = STATE_READY;
            }
            self.save_assignments();
            let announced =
                self.controller_event(padmap_core::announce::ACTION_ADDED, player, &pad, "");
            self.broadcast(&announced);
            let state = self.state_event();
            self.broadcast(&state);
            self.seating.reset();
            self.refresh_seating(&mut Scan::default());
        }
    }

    fn on_source_read(&mut self, index: usize) {
        if self
            .republisher
            .as_ref()
            .is_some_and(|republisher| republisher.held_back(index))
        {
            self.read_published_for_modal(index);
            return;
        }
        let Some(republisher) = self.republisher.as_mut() else {
            return;
        };
        let pumped = republisher.forward(index);
        let gone = pumped.gone;
        if gone {
            if let Some(vpad) = republisher.pads.get(index) {
                let _ = self.reactor.unwatch(vpad.source.as_fd());
            }
        }
        if !gone {
            self.publish_motion();
        }
    }

    fn on_motion_read(&mut self, index: usize) {
        let Some(republisher) = self.republisher.as_mut() else {
            return;
        };
        if republisher.read_motion(index).gone {
            if let Some(sensor) = republisher
                .pads
                .get(index)
                .and_then(|vpad| vpad.sensor.as_ref())
            {
                let _ = self.reactor.unwatch(sensor.as_fd());
            }
            republisher.drop_sensor(index);
            return;
        }
        self.publish_motion();
    }

    fn on_dsu_read(&mut self) {
        let ports = self.motion_ports();
        if let Some(motion) = self.motion.as_mut() {
            motion.serve(&ports);
        }
        self.publish_motion();
    }

    /// What padmap would tell a consumer about each slot right now.
    fn motion_ports(&self) -> Vec<padmap_core::dsu::Port> {
        let Some(republisher) = self.republisher.as_ref() else {
            return Vec::new();
        };
        republisher
            .pads
            .iter()
            .filter(|vpad| !vpad.gone)
            .map(|vpad| {
                let has_motion = vpad.sensor.is_some() || vpad.source.motion().is_some();
                crate::dsu::port_for(vpad.player, has_motion)
            })
            .collect()
    }

    fn publish_motion(&mut self) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if !motion.has_clients() {
            return;
        }
        let Some(republisher) = self.republisher.as_ref() else {
            return;
        };
        let mut ports = Vec::new();
        let mut pads = Vec::new();
        for vpad in republisher.pads.iter().filter(|vpad| !vpad.gone) {
            let has_motion = vpad.sensor.is_some() || vpad.source.motion().is_some();
            ports.push(crate::dsu::port_for(vpad.player, has_motion));
            pads.push(*vpad.tracker.pad());
        }
        motion.publish(&ports, &pads);
    }

    fn guarded_tick(&mut self) {
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| self.tick())) {
            self.tick_failures += 1;
            if self.tick_failures == 1
                || self.tick_failures == 100
                || self.tick_failures.is_multiple_of(1000)
            {
                let what = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_else(|| "panic".to_owned());
                warn!("tick failed ({} so far): {what}", self.tick_failures);
            }
        }
    }

    fn tick(&mut self) {
        self.flush_all();
        for fd in self.clients.keys().copied().collect::<Vec<_>>() {
            self.on_client_read(fd);
        }
        let mut scan = Scan::default();
        self.poll_controller_changes(&mut scan);
        self.poll_new_controllers(&mut scan);
        self.sync_republish_pause();
        self.reap_dead_pads();
        if let Some(republisher) = self.republisher.as_mut() {
            republisher.flush_debounce();
        }
        self.publish_motion();
        self.republish_if_stale();
        self.refresh_seating(&mut scan);
        self.tick_seating();
        self.hold_siblings();

        let clock = now();
        self.tick_mapping(clock);
        if self.choice.is_none() && self.mapping.is_none() && self.calibration.is_some() {
            self.tick_calibration();
        }
        if self.solo.is_some() && !self.modal_open() {
            self.release_solo();
        }
        if self.session.is_none() || self.modal_open() {
            return;
        }

        let (before, ticked) = {
            let Some(session) = self.session.as_mut() else {
                return;
            };
            let before = session.claims().len();
            (before, session.assigner.tick(clock))
        };
        let progress = ticked
            .progress
            .iter()
            .map(|(_, fraction)| fraction.min(1.0))
            .fold(0.0_f64, f64::max);
        for claim in &ticked.claimed {
            let Some(pad) = self
                .session
                .as_ref()
                .and_then(|session| session.pads.get(claim.pad).cloned())
            else {
                continue;
            };
            info!(
                "claim: player {} <- {} ({})",
                claim.player,
                clean(&pad.name),
                pad.event()
            );
            let event = events::claim(
                claim.player,
                &clean(&pad.name),
                pad.event(),
                publish::icon_for(&pad, &self.icon_overrides),
                profiles::is_known(&pad, None),
            );
            self.broadcast(&event);
        }
        if progress != self.last_progress {
            self.last_progress = progress;
            self.broadcast(&events::progress(progress));
        }
        let after = self.session.as_ref().map(|s| s.claims().len()).unwrap_or(0);
        if after != before {
            let state = self.state_event();
            self.broadcast(&state);
        }
        self.tick_confirm(clock);
    }

    fn tick_confirm(&mut self, clock: f64) {
        let Some(fraction) = self.confirm.fraction(clock) else {
            if self.last_confirm != 0.0 {
                self.last_confirm = 0.0;
                self.broadcast(&events::confirm(0.0));
            }
            return;
        };
        if fraction != self.last_confirm {
            self.last_confirm = fraction;
            self.broadcast(&events::confirm(fraction));
        }
        if fraction >= 1.0 {
            self.accept();
        }
    }

    fn start_republisher(&mut self) -> Result<(), clone::CloneError> {
        self.stop_republisher();
        let mut vpads = Vec::with_capacity(self.slots_assigned.len());
        let mut first_failure = None;
        for slot in &self.slots_assigned {
            let axes = profiles::load(&slot.pad, None)
                .map(|profile| profile.axes)
                .unwrap_or_default();
            let tuning = publish::tuning_for(&slot.pad);
            match clone::create(&slot.pad, slot.player, self.mode, &axes, tuning, true) {
                Ok(vpad) => vpads.push(vpad),
                Err(error) => {
                    warn!(
                        "player {}: not republished ({error}); its seat is kept",
                        slot.player
                    );
                    first_failure.get_or_insert(error);
                }
            }
        }
        if vpads.is_empty() {
            if let Some(error) = first_failure {
                return Err(error);
            }
        }
        let mut virtual_paths: BTreeMap<u32, String> = BTreeMap::new();
        for vpad in &mut vpads {
            if let Some(node) = vpad.node() {
                virtual_paths.insert(vpad.player, node);
            }
        }
        let republisher = Republisher::new(vpads);
        for (index, vpad) in republisher.pads.iter().enumerate() {
            if let Err(error) = self
                .reactor
                .watch(vpad.source.as_fd(), Watched::Source(index))
            {
                warn!(
                    "player {}: could not watch its source: {error}",
                    vpad.player
                );
            }
            if let Err(error) = self
                .reactor
                .watch(vpad.clone.as_fd(), Watched::Clone(index))
            {
                warn!("player {}: could not watch its clone: {error}", vpad.player);
            }
            if let Some(sensor) = vpad.sensor.as_ref() {
                if let Err(error) = self.reactor.watch(sensor.as_fd(), Watched::Motion(index)) {
                    warn!(
                        "player {}: could not watch its motion sensor: {error}",
                        vpad.player
                    );
                }
            }
        }
        info!(
            "republisher watching {:?}",
            republisher
                .pads
                .iter()
                .map(|vpad| (vpad.player, vpad.source.path()))
                .collect::<Vec<_>>()
        );
        self.republisher = Some(republisher);
        self.sync_republish_pause();

        self.rewrite_consumers(&virtual_paths);
        info!(
            "republishing {} pad(s); launch config at {}",
            self.slots_assigned.len(),
            self.launch_config_path.display()
        );
        Ok(())
    }

    /// Write every consumer's config for the seats as they are now -- with no
    /// seats, that is a launch config naming nobody, not yesterday's roster.
    fn rewrite_consumers(&mut self, virtual_paths: &BTreeMap<u32, String>) {
        let last = runtime::read_recent_games().into_iter().next();
        let written = publish::write_all(
            &self.slots_assigned,
            virtual_paths,
            self.mode,
            &self.launch_config_path,
            &self.launch_args_path,
            last.as_ref(),
            self.keyboard_seat,
        );
        self.sdl_lines = written.sdl_lines;
        let lines = events::sdl_mapping(&self.sdl_lines);
        self.broadcast(&lines);
    }

    fn stop_republisher(&mut self) {
        let Some(mut republisher) = self.republisher.take() else {
            return;
        };
        for vpad in &republisher.pads {
            let _ = self.reactor.unwatch(vpad.source.as_fd());
            let _ = self.reactor.unwatch(vpad.clone.as_fd());
            if let Some(sensor) = vpad.sensor.as_ref() {
                let _ = self.reactor.unwatch(sensor.as_fd());
            }
        }
        republisher.close();
    }

    /// Only the pad a modal flow is reading is held back from its clone; everyone else plays on.
    fn sync_republish_pause(&mut self) {
        let paths = self.modal_paths();
        if let Some(republisher) = self.republisher.as_mut() {
            for index in 0..republisher.pads.len() {
                let held = paths.contains(&republisher.pads[index].pad.path);
                republisher.hold_back(index, held);
            }
        }
    }

    fn reap_dead_pads(&mut self) {
        let Some(republisher) = &self.republisher else {
            return;
        };
        for index in republisher.dead() {
            if let Some(vpad) = republisher.pads.get(index) {
                let _ = self.reactor.unwatch(vpad.source.as_fd());
            }
        }
    }

    fn republish_if_stale(&mut self) {
        let clock = now();
        if clock - self.last_stale_check < STALE_CHECK_SECONDS {
            return;
        }
        self.last_stale_check = clock;
        let stale: Vec<(u32, String)> = match &self.republisher {
            Some(republisher) => republisher
                .pads
                .iter()
                .filter(|vpad| vpad.gone || !vpad.pad.path.exists())
                .map(|vpad| (vpad.player, vpad.pad.path.display().to_string()))
                .collect(),
            None => return,
        };
        if stale.is_empty() {
            return;
        }
        for (player, path) in &stale {
            warn!("player {player}: {path} is gone; reopening");
        }
        let pads = discover();
        for slot in &mut self.slots_assigned {
            if !slot.pad.path.exists() {
                let signature = profiles::signature_of(&slot.pad);
                if let Some(found) = pads
                    .iter()
                    .find(|pad| profiles::signature_of(pad) == signature)
                {
                    slot.pad = found.clone();
                }
            }
        }
        if let Err(error) = self.start_republisher() {
            warn!("could not reopen after a reconnect ({error}); will retry");
        }
    }

    fn poll_controller_changes(&mut self, scan: &mut Scan) {
        let event_nodes = hotplug::event_nodes();
        let mut nodes = event_nodes.clone();
        nodes.extend(self.triton_live_slots(&event_nodes));
        if Some(&nodes) == self.last_attach_nodes.as_ref() {
            return;
        }
        let clock = now();
        if clock - self.last_attach_scan < hotplug::ATTACH_SCAN_SECONDS {
            return;
        }
        self.last_attach_scan = clock;
        self.last_attach_nodes = Some(nodes);

        if self.state == STATE_ASSIGNING {
            self.admit_late_pads(scan);
            return;
        }
        let present: BTreeMap<String, Pad> = scan
            .pads()
            .iter()
            .map(|pad| (profiles::signature_of(pad), pad.clone()))
            .collect();
        let changes = self.attached.diff(&present.keys().cloned().collect());
        for signature in changes.departed {
            self.announce_departure(&signature);
        }
        for signature in changes.arrived {
            if let Some(pad) = present.get(&signature) {
                self.announce_arrival(pad.clone());
            }
        }
    }

    fn triton_live_slots(&mut self, event_nodes: &BTreeSet<String>) -> BTreeSet<String> {
        if self.triton_checked_for.as_ref() != Some(event_nodes) {
            self.triton_checked_for = Some(event_nodes.clone());
            self.triton_present = !triton::slots(false).is_empty();
            if !self.triton_present {
                self.triton_live.clear();
            }
        }
        if !self.triton_present {
            return BTreeSet::new();
        }
        let clock = now();
        if clock - self.last_triton_scan < TRITON_SCAN_SECONDS {
            return self.triton_live.clone();
        }
        self.last_triton_scan = clock;
        self.triton_live = triton::live_signature()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect();
        self.triton_live.clone()
    }

    /// Hold the keyboard and mouse siblings of every seated pad and every pad
    /// seating is listening to, and only those. Recomputed when the nodes on
    /// the machine or the pads that matter change, not every tick.
    fn hold_siblings(&mut self) {
        let mut pads: Vec<&Pad> = self.slots_assigned.iter().map(|slot| &slot.pad).collect();
        if self.seating.is_open() {
            pads.extend(self.seating.pads().iter());
        }
        let mut paths: Vec<PathBuf> = pads.iter().map(|pad| pad.path.clone()).collect();
        paths.sort();
        paths.dedup();
        let nodes = self
            .last_attach_nodes
            .clone()
            .unwrap_or_else(hotplug::event_nodes);
        if self.held_for.as_ref() == Some(&(nodes.clone(), paths.clone())) {
            return;
        }
        // Nothing seated and nothing listened to: nothing to look for.
        let known = if pads.is_empty() {
            Vec::new()
        } else {
            siblings::scan()
        };
        let wanted: BTreeSet<PathBuf> = pads
            .iter()
            .flat_map(|pad| siblings::of(pad, &known))
            .collect();
        self.held.sync(&wanted);
        self.held_for = Some((nodes, paths));
    }

    /// A pad switched on during a session joins it: grabbed and watched like
    /// the others, claimable by the same hold. `pads` is sent again with the
    /// new count, so "no controllers found" can stop saying it.
    fn admit_late_pads(&mut self, scan: &mut Scan) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let late: Vec<Pad> = scan
            .pads()
            .iter()
            .filter(|pad| {
                session
                    .index_of(&pad.path)
                    .is_none_or(|index| session.is_gone(index))
            })
            .cloned()
            .collect();
        if late.is_empty() {
            return;
        }
        let mut admitted = 0;
        let mut retry = false;
        for pad in late {
            let name = clean(&pad.name);
            let path = pad.path.clone();
            match session.admit(pad) {
                Ok(index) => {
                    self.late_attempts.remove(&path);
                    let Some(source) = session.sources().get(index) else {
                        continue;
                    };
                    match self.reactor.watch(source.as_fd(), Watched::Session(index)) {
                        Ok(()) => {
                            info!("session: {name} joined late as pad {index}");
                            admitted += 1;
                        }
                        Err(error) => warn!("session: could not watch {name}: {error}"),
                    }
                }
                Err(error) => {
                    // udev grants access a beat after the node appears; try again
                    // on the next scan, as the hotplug path does, up to a limit.
                    let tries = self.late_attempts.entry(path).or_insert(0);
                    *tries += 1;
                    if *tries < hotplug::ATTACH_ATTEMPTS {
                        debug!("session: {name} not admitted yet ({error}); retrying");
                        retry = true;
                    } else {
                        warn!("session: {name} could not be admitted ({error}); giving up");
                    }
                }
            }
        }
        if admitted > 0 {
            let count = session.present();
            self.broadcast(&events::pads(count));
        }
        if retry {
            self.last_attach_nodes = None;
        }
    }

    fn announce_departure(&mut self, signature: &str) {
        let Some(player) = self.attached.live.remove(signature) else {
            return;
        };
        let Some(gone) = self
            .slots_assigned
            .iter()
            .find(|slot| slot.player == player)
            .map(|slot| slot.pad.clone())
        else {
            return;
        };
        info!("player {player}: {} unplugged", clean(&gone.name));
        let event = self.controller_event(announce::ACTION_REMOVED, player, &gone, "");
        self.broadcast(&event);
    }

    fn announce_arrival(&mut self, pad: Pad) {
        let signature = profiles::signature_of(&pad);
        if let Some(at) = self.away.iter().position(|entry| {
            entry.name == pad.name
                && entry.vid == pad.vid
                && entry.pid == pad.pid
                && (entry.phys == pad.phys || entry.path == pad.path)
        }) {
            let entry = self.away.remove(at);
            info!("player {}: {} is back", entry.player, clean(&pad.name));
            self.slots_assigned.push(Slot {
                player: entry.player,
                pad: pad.clone(),
            });
            self.attached.live.insert(signature.clone(), entry.player);
            self.rebuild_for_attach(entry.player, &pad);
            return;
        }
        if let Some(existing) = self
            .slots_assigned
            .iter()
            .find(|slot| profiles::signature_of(&slot.pad) == signature)
        {
            let player = existing.player;
            self.attached.live.insert(signature, player);
            self.rebuild_for_attach(player, &pad);
            return;
        }
        if !publish::has_mapping(&pad) {
            info!(
                "{} attached with no stored mapping; not binding it",
                clean(&pad.name)
            );
            let event = self.controller_event(
                announce::ACTION_UNCONFIGURED,
                0,
                &pad,
                announce::REASON_UNMAPPED,
            );
            self.broadcast(&event);
            return;
        }
        if std::env::var(ENV_NO_AUTOATTACH).as_deref() == Ok("1") {
            info!(
                "{} attached; not binding it ({ENV_NO_AUTOATTACH})",
                clean(&pad.name)
            );
            return;
        }
        let player = announce::next_player(&self.taken_seats());
        if player > padmap_core::retroarch::MAX_PLAYERS {
            warn!(
                "{} attached but every player slot is taken",
                clean(&pad.name)
            );
            return;
        }
        self.slots_assigned.push(Slot {
            player,
            pad: pad.clone(),
        });
        self.attached.live.insert(signature.clone(), player);
        info!("player {player}: {} attached mid-session", clean(&pad.name));
        if self.rebuild_for_attach(player, &pad) {
            self.attached.attempts.remove(&signature);
            return;
        }
        self.slots_assigned.retain(|slot| slot.player != player);
        self.attached.live.remove(&signature);
        if !self.attached.failed(&signature) {
            self.last_attach_nodes = None;
            return;
        }
        warn!(
            "{} could not be opened after {} attempts; giving up",
            clean(&pad.name),
            hotplug::ATTACH_ATTEMPTS
        );
        let event = self.controller_event(
            announce::ACTION_UNCONFIGURED,
            0,
            &pad,
            announce::REASON_UNREADABLE,
        );
        self.broadcast(&event);
    }

    fn rebuild_for_attach(&mut self, player: u32, pad: &Pad) -> bool {
        if let Err(error) = self.start_republisher() {
            warn!(
                "could not republish after {} attached: {error}",
                clean(&pad.name)
            );
            return false;
        }
        if self.republisher.is_some() {
            self.state = STATE_READY;
        }
        self.save_assignments();
        let event = self.controller_event(announce::ACTION_ADDED, player, pad, "");
        self.broadcast(&event);
        true
    }

    fn controller_event(&mut self, action: &str, player: u32, pad: &Pad, reason: &str) -> Value {
        let last = runtime::read_recent_games().into_iter().next();
        let console = last.as_ref().map(|g| g.console.clone()).unwrap_or_default();
        let game = last.as_ref().map(|g| g.key.clone()).unwrap_or_default();
        let title = last.as_ref().map(|g| g.title.clone()).unwrap_or_default();

        let mut live: BTreeMap<u32, (String, emit::Identity)> = BTreeMap::new();
        if let Some(republisher) = self.republisher.as_mut() {
            for vpad in &mut republisher.pads {
                let node = vpad.node().unwrap_or_default();
                let identity = emit::Identity {
                    bustype: vpad.identity.bustype,
                    vendor: vpad.identity.vendor,
                    product: vpad.identity.product,
                    version: vpad.identity.version,
                };
                live.insert(vpad.player, (node, identity));
            }
        }
        let virtual_paths: BTreeMap<u32, String> = live
            .iter()
            .map(|(p, (node, _))| (*p, node.clone()))
            .collect();
        let indices =
            padmap_core::retroarch::compute_pad_indices(&virtual_paths, &publish::visible_order());

        let attach = |player: u32, pad: &Pad| -> announce::Attached {
            let identity = live
                .get(&player)
                .map(|(_, identity)| *identity)
                .unwrap_or_else(|| publish::identity_of(pad, player, self.mode));
            let name = emit::virtual_name(player);
            let facts = publish::pad_facts(pad);
            let profile_text =
                publish::profile_text(pad, player, identity, &console, &game, &title);
            announce::Attached {
                player,
                controller: announce::Controller {
                    name: pad.name.clone(),
                    vid: pad.vid,
                    pid: pad.pid,
                    path: pad.path.display().to_string(),
                    phys: pad.phys.clone(),
                    uniq: pad.uniq.clone(),
                    signature: profiles::signature_of(pad),
                    retroarch_visible: pad.retroarch_visible,
                },
                virtual_pad: Some(announce::Virtual {
                    node: live
                        .get(&player)
                        .map(|(node, _)| node.clone())
                        .unwrap_or_default(),
                    phys: clone::virtual_phys(player),
                    vid: identity.vendor,
                    pid: identity.product,
                    bustype: identity.bustype,
                    guid: emit::virtual_guid(player, identity),
                    identity_mode: self.mode.as_str().to_owned(),
                    name: name.clone(),
                }),
                retroarch: Some(announce::Retroarch {
                    index: indices.get(&player).copied(),
                    profile: artefacts::autoconfig_dir()
                        .join(format!("{name}.cfg"))
                        .display()
                        .to_string(),
                    binds: padmap_core::userconfig::parse_profile_text(&profile_text),
                }),
                sdl_mapping: publish::stored_sdl_line(player, pad, identity, &facts),
            }
        };
        let subject = attach(player, pad);
        let roster: Vec<announce::Attached> = self
            .slots_assigned
            .iter()
            .filter(|slot| {
                self.attached
                    .live
                    .contains_key(&profiles::signature_of(&slot.pad))
            })
            .map(|slot| attach(slot.player, &slot.pad))
            .collect();
        announce::controller_event(
            action,
            &subject,
            &roster,
            &console,
            &game,
            &runtime::build_id_of_binary(),
            reason,
        )
    }

    /// Why a first-time controller must not open setup, or None.
    fn autosetup_blocked(&self) -> Option<&'static str> {
        if std::env::var(ENV_NO_AUTOSETUP).as_deref() == Ok("1") {
            return Some("disabled by PADMAP_NO_AUTOSETUP");
        }
        if self.state == STATE_ASSIGNING {
            return Some("a session is already open");
        }
        if self.modal_open() {
            return Some("a controller is being set up");
        }
        let clock = now();
        if !self
            .clients
            .values()
            .any(|client| clock - client.connected_at >= AUTOSETUP_CLIENT_SECONDS)
        {
            return Some("no front-end is connected");
        }
        if runtime::game_is_running() {
            return Some("a game is running");
        }
        None
    }

    fn poll_new_controllers(&mut self, scan: &mut Scan) {
        let clock = now();
        if clock - self.last_pad_scan < PAD_SCAN_SECONDS {
            return;
        }
        self.last_pad_scan = clock;
        let nodes = hotplug::event_nodes();
        let stamp = stamp_of(&self.prompted_path);
        if self.last_scan_signature.as_ref() == Some(&(nodes.clone(), stamp)) {
            return;
        }
        if self.autosetup_blocked().is_some() {
            return;
        }
        self.last_scan_signature = Some((nodes, stamp));
        self.reload_prompted_if_changed();

        let mut fresh: BTreeMap<String, Pad> = BTreeMap::new();
        for pad in scan.pads() {
            let signature = profiles::signature_of(pad);
            if self.prompted.contains(&signature) || publish::has_mapping(pad) {
                continue;
            }
            fresh.insert(signature, pad.clone());
        }
        if fresh.is_empty() {
            return;
        }
        let mut names: Vec<String> = fresh.values().map(|pad| clean(&pad.name)).collect();
        names.sort();
        self.prompted.extend(fresh.into_keys());
        self.save_prompted();
        info!(
            "first time seeing {} -- opening controller setup",
            names.join(", ")
        );
        self.broadcast(&events::newpad(&names));
        self.begin(self.slots);
    }

    fn reload_prompted_if_changed(&mut self) {
        let stamp = stamp_of(&self.prompted_path);
        if stamp != self.prompted_stamp {
            self.prompted_stamp = stamp;
            self.prompted = runtime::read_prompted(&self.prompted_path);
        }
    }

    fn save_prompted(&mut self) {
        if let Err(error) = runtime::write_prompted(&self.prompted_path, &self.prompted) {
            warn!("could not record prompted controllers: {error}");
        }
        self.prompted_stamp = stamp_of(&self.prompted_path);
    }

    fn taken_seats(&self) -> Vec<u32> {
        self.slots_assigned
            .iter()
            .map(|slot| slot.player)
            .chain(self.away.iter().map(|entry| entry.player))
            .chain(self.keyboard_seat)
            .collect()
    }

    fn published_players(&self) -> Vec<u32> {
        self.republisher
            .as_ref()
            .map(|republisher| republisher.pads.iter().map(|vpad| vpad.player).collect())
            .unwrap_or_default()
    }

    fn players_payload(&self) -> Vec<PlayerState> {
        let source: Vec<(u32, Pad)> = match &self.session {
            Some(session) => session
                .claimed_pads()
                .into_iter()
                .map(|(player, pad)| (player, pad.clone()))
                .collect(),
            None => self
                .slots_assigned
                .iter()
                .map(|slot| (slot.player, slot.pad.clone()))
                .collect(),
        };
        let live = self.published_players();
        let mut players: Vec<PlayerState> = source
            .into_iter()
            .map(|(player, pad)| PlayerState {
                player,
                name: clean(&pad.name),
                node: pad.event().to_owned(),
                icon: publish::icon_for(&pad, &self.icon_overrides).to_owned(),
                configured: publish::has_mapping(&pad),
                mappings: publish::mapping_scopes(&pad),
                published: live.contains(&player),
                keyboard: false,
            })
            .collect();
        if let Some(seat) = self.keyboard_seat.filter(|_| self.session.is_none()) {
            players.push(keyboard_state(seat));
        }
        players.sort_by_key(|state| state.player);
        players
    }

    /// Seat the keyboard as the next player. No device is read and nothing is
    /// grabbed -- the keyboard stays the compositor's -- but every emulator's
    /// config now names it as that player, and a pad seated after it takes
    /// the seat after. Refused while a session is open, when it already holds
    /// a seat, and when every seat is taken.
    fn seat_keyboard(&mut self) {
        if self.session.is_some() {
            self.broadcast(&events::error("a session is open; cancel it first"));
            return;
        }
        if let Some(seat) = self.keyboard_seat {
            self.broadcast(&events::error(format!(
                "the keyboard is already player {seat}"
            )));
            return;
        }
        let player = padmap_core::announce::next_player(&self.taken_seats());
        let seats = self.slots.max(self.seating.seats());
        if player > seats {
            self.broadcast(&events::error("every seat is taken"));
            return;
        }
        self.keyboard_seat = Some(player);
        info!("seating: player {player} <- the keyboard");
        let state = keyboard_state(player);
        self.broadcast(&events::claim(player, &state.name, "", &state.icon, true));
        if self.republisher.is_some() {
            // Consumers are rewritten for the seat change; the clones are untouched.
            let paths = self.virtual_paths();
            self.rewrite_consumers(&paths);
        } else {
            self.rewrite_consumers(&BTreeMap::new());
        }
        self.save_assignments();
        let state = self.state_event();
        self.broadcast(&state);
    }

    /// The clone node of every published player.
    fn virtual_paths(&mut self) -> BTreeMap<u32, String> {
        let mut paths = BTreeMap::new();
        if let Some(republisher) = self.republisher.as_mut() {
            for vpad in &mut republisher.pads {
                if let Some(node) = vpad.node() {
                    paths.insert(vpad.player, node);
                }
            }
        }
        paths
    }

    fn state_event(&self) -> Value {
        events::state(
            self.state,
            self.slots,
            self.players_payload(),
            runtime::build_id_of_binary(),
            self.mode.as_str(),
            self.follow,
        )
    }

    fn save_assignments(&self) {
        let mut entries: Vec<assignments::Assignment> = self
            .slots_assigned
            .iter()
            .map(to_input_assignment)
            .collect();
        entries.extend(self.away.iter().cloned());
        entries.extend(self.keyboard_seat.map(assignments::keyboard_assignment));
        entries.sort_by_key(|entry| entry.player);
        if let Err(error) = assignments::save(&self.state_path, &entries) {
            warn!("could not save assignments: {error}");
        }
    }
}

fn as_player(player: i64) -> u32 {
    u32::try_from(player).unwrap_or(0)
}

/// What `state.players[]` says of the keyboard's seat.
fn keyboard_state(player: u32) -> PlayerState {
    PlayerState {
        player,
        name: "Keyboard".to_owned(),
        node: String::new(),
        icon: "keyboard".to_owned(),
        configured: true,
        mappings: Vec::new(),
        published: false,
        keyboard: true,
    }
}

const FOLLOW_POLL_SECONDS: f64 = 0.25;

/// Whether `pid` is still around. A signal of 0 checks without sending; EPERM
/// means it exists but is not ours, which is still "exists".
fn process_exists(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    !matches!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    )
}

fn raw_events(events: &[evdev::InputEvent]) -> Vec<Raw> {
    events
        .iter()
        .map(|event| Raw {
            kind: event.event_type().0,
            code: event.code(),
            value: event.value(),
        })
        .collect()
}

fn stamp_of(path: &Path) -> i128 {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_nanos() as i128)
        .unwrap_or(0)
}

fn daemon_alive(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}
