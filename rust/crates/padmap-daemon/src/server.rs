//! The daemon proper: one process, one loop, every long-lived thing.
//!
//! See the crate note for the invariant. Concretely, every path from the
//! outside world into this file is guarded: a command is dispatched under
//! `catch_unwind` and answered with an error event if it fails; the tick is
//! guarded the same way, rationed so a fault that repeats fifty times a second
//! cannot fill a disk with its own report; and nothing a client sends, no
//! device going away, and no file on disk may reach a `panic` that gets out.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use log::{info, warn};
use padmap_core::announce;
use padmap_core::capture::{self, Chooser, MappingRun};
use padmap_core::command::{Command, Refused};
use padmap_core::state::{PlayerState, STATE_ASSIGNING, STATE_IDLE, STATE_READY};
use padmap_core::wire::{self, LineReader};
use padmap_core::{emit, scope};
use padmap_input::clone::{self, IdentityMode};
use padmap_input::pad::{self, Pad};
use padmap_input::reactor::{Reactor, Watched};
use padmap_input::republish::Republisher;
use padmap_input::{artefacts, assignments, profiles, runtime, triton};
use serde_json::Value;

use crate::calibration::{CalibrationRun, Phase, Step};
use crate::confirm::ConfirmHold;
use crate::hotplug::{self, Attached};
use crate::publish::{self, Slot};
use crate::session::{Raw, Session};
use crate::{clean, events, now};

/// Set to "1" to keep the setup screen from opening by itself.
pub const ENV_NO_AUTOSETUP: &str = "PADMAP_NO_AUTOSETUP";
/// Announce arrivals but never claim a player slot for them.
pub const ENV_NO_AUTOATTACH: &str = "PADMAP_NO_AUTOATTACH";

/// How often to look for a controller model that has never been set up.
pub const PAD_SCAN_SECONDS: f64 = 1.0;
/// How often to ask a Steam Controller receiver which slots have a pad in
/// them: four real USB control transfers, 12ms measured, so not every tick.
pub const TRITON_SCAN_SECONDS: f64 = 1.0;
/// How long a client must have been connected before the daemon will grab
/// the pads on its behalf. Longer than a status query, far shorter than a
/// front-end's lifetime.
pub const AUTOSETUP_CLIENT_SECONDS: f64 = 3.0;
/// How often to check that a republished pad is still the device we opened.
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

/// A slow client must not be dropped: a front-end busy drawing a frame stops
/// reading for a moment, and the protocol is built for a client to miss an
/// event and re-sync from the next `state`. So an event that will not fit the
/// socket right now is buffered rather than written half-way -- a partial
/// write would corrupt the framing -- and drained on the next tick. Only a
/// client that has fallen this far behind is genuinely gone.
const MAX_PENDING: usize = 8 * 1024 * 1024;

struct Client {
    stream: UnixStream,
    reader: LineReader,
    connected_at: f64,
    /// Bytes written but not yet accepted by the socket, from the front.
    pending: Vec<u8>,
}

/// A wizard bound to the pad it reads.
struct Modal<T> {
    run: T,
    pad_path: PathBuf,
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
    republisher: Option<Republisher>,
    confirm: ConfirmHold,
    last_progress: f64,
    last_confirm: f64,
    slots: u32,
    icon_overrides: BTreeMap<String, String>,
    calibration: Option<CalibrationRun>,
    mapping: Option<Modal<MappingRun>>,
    choice: Option<Modal<Chooser>>,
    /// Which scope the wizard that a picker is about to start will file its
    /// capture under. "" is this controller's default.
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

/// A device scan, run at most once per tick and never reused across ticks.
///
/// Both pollers need the full pad list on the tick a controller arrives, and
/// discovery is the call this loop has spent the most effort avoiding.
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
    /// A server on the default paths under `XDG_RUNTIME_DIR`.
    pub fn new() -> Result<Server, StartError> {
        let base = runtime::dir();
        Server::at(
            runtime::socket_path(),
            runtime::assignments_path(),
            base.join("launch.cfg"),
        )
    }

    /// A server on explicit paths. Nothing is opened yet.
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
            republisher: None,
            confirm: ConfirmHold::default(),
            last_progress: 0.0,
            last_confirm: 0.0,
            slots: 4,
            icon_overrides: runtime::load_icon_overrides(),
            calibration: None,
            mapping: None,
            choice: None,
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
        })
    }

    // -- lifecycle --------------------------------------------------------

    /// Bind the socket. Refuses to steal it from a daemon that is alive.
    pub fn start(&mut self) -> Result<(), StartError> {
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| StartError::Directory(self.socket_path.clone(), error))?;
        }
        // A socket left by a crashed daemon would make bind() fail even
        // though nothing is listening. Probe before removing.
        if self.socket_path.exists() {
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
        Ok(())
    }

    /// Republish the saved assignments, making a restart invisible.
    ///
    /// Pads are matched by device path. A pad that really is gone is skipped
    /// rather than faked: a dead virtual pad in the enumeration shifts every
    /// index after it.
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
        let pads = discover();
        let mut restored = Vec::new();
        for entry in &saved {
            match pads.iter().find(|pad| pad.path == entry.path) {
                Some(pad) => restored.push(Slot {
                    player: entry.player,
                    pad: pad.clone(),
                }),
                None => warn!(
                    "player {}: {} ({}) is gone, not restored",
                    entry.player,
                    clean(&entry.name),
                    entry.path.display()
                ),
            }
        }
        if restored.is_empty() {
            return;
        }
        self.slots = restored.iter().map(|slot| slot.player).max().unwrap_or(4);
        self.slots_assigned = restored;
        // Coming up idle is a state the user can fix from the setup screen;
        // not coming up is not.
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

    /// The loop, until `stop` is set.
    pub fn run(&mut self, stop: &Arc<AtomicBool>) {
        while !stop.load(Ordering::Relaxed) {
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
                    Watched::Source(index) => self.on_source_read(index),
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

    /// Release everything: the session's grabs, the clones, the socket.
    pub fn close(&mut self) {
        self.end_session();
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

    // -- socket -----------------------------------------------------------

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
        // An assignment session holds every pad. If the front-end that
        // opened it goes away -- crashed, killed, quit without cancel --
        // nothing would ever release those grabs, and every controller on the
        // machine stays dead until the daemon is restarted.
        if self.clients.is_empty() && self.session.is_some() {
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

    /// Write as much of a client's pending bytes as the socket will take.
    ///
    /// Never a partial message left dangling: the socket is written from the
    /// front of the buffer and whatever it would not take stays there, so the
    /// stream a client reads is always a run of whole messages. A real write
    /// error, or a backlog past [`MAX_PENDING`], is a client that is gone.
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

    /// Drain every client's backlog. Called from the tick, so a client that
    /// went quiet for a moment catches up without waiting for the next event
    /// addressed to it.
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

    // -- commands ---------------------------------------------------------

    /// Dispatch one command, surviving anything the client sends.
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
                // A wizard opens on a command, and the press answering its
                // first prompt can arrive before the next tick.
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
                // Escape hatch: leave the modal flow without finishing it.
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
            Command::Status => {
                let state = self.state_event();
                self.send(fd, &state);
                // Every connect, not only after a capture: an SDL client reads
                // its database once at startup. Re-sending is idempotent.
                let lines = events::sdl_mapping(&self.sdl_lines);
                self.send(fd, &lines);
            }
        }
    }

    // -- the session ------------------------------------------------------

    fn begin(&mut self, players: u32) {
        // Republishing grabs the physical pads, so it has to stop before the
        // session can open them; otherwise every press would be invisible.
        self.stop_republisher();
        self.end_session();

        let pads = discover();
        if pads.is_empty() {
            self.broadcast(&events::error("no joypads found"));
            return;
        }
        // Re-read so an icon correction takes effect on the next setup.
        self.icon_overrides = runtime::load_icon_overrides();

        let session = match Session::open(pads) {
            Ok(session) => session,
            Err(error) => {
                // Put back whatever was on the air before: failing here must
                // not be a more expensive way to lose your controllers than
                // never having asked.
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
        // And the wizard. Without this a capture in flight survived into the
        // new session holding a pad from the closed one, and the tick returns
        // early while a mapping is open -- so for the whole of the next
        // session no hold could claim a slot and confirm never fired.
        self.mapping = None;
        // `players` is advisory: the session ends on the confirm gesture.
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

    /// Put the stored assignments back on the air, and settle the state.
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

    /// Abandon an assignment session. Only moves to idle if a session was
    /// actually open: cancelling while READY used to drop the state to idle
    /// while the virtual pads kept running.
    fn cancel(&mut self) {
        let had_session = self.session.is_some();
        if let Some(session) = &self.session {
            info!(
                "session cancelled after {} claim(s)",
                session.claims().len()
            );
        }
        // Close a modal flow properly rather than dropping it: an overlay left
        // believing it is active would sit waiting for events that cannot
        // arrive.
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
        // The SDL lines and every other file, then the clones. Guarded: a
        // controller unplugged between the last claim and the confirm hold
        // makes the clone fail, and that used to escape *after* the state
        // file was written -- the setup screen waited forever and the daemon
        // exited.
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
        // Dropping the session ungrabs and closes every pad.
        drop(session);
        self.confirm.clear();
    }

    /// The pad behind a player number, claims first, then what is stored.
    ///
    /// A session's claims win because during one they are the live answer.
    /// But a session starts with no claims, and the stored assignment is
    /// still true -- consulting only the claims made every per-player command
    /// fail for the first seconds of a session, about a controller that was
    /// plainly assigned.
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

    /// A player's pad and its session index, or the error event to send.
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

    fn held_keys(&mut self, index: usize) -> BTreeSet<u16> {
        self.session
            .as_mut()
            .and_then(|session| session.source_mut(index))
            .map(|source| source.held_keys().into_iter().collect())
            .unwrap_or_default()
    }

    /// Declared travel per axis plus where each one is sitting right now.
    ///
    /// Rest is read from the driver rather than assumed to be the middle:
    /// an analogue trigger rests at its minimum, and an uncalibrated stick
    /// can rest well off centre. Nonsense from the driver -- a rest outside
    /// the range -- falls back to the midpoint.
    fn absolute_ranges(&mut self, index: usize) -> BTreeMap<u16, padmap_core::sdl::AxisSpan> {
        self.session
            .as_mut()
            .and_then(|session| session.source_mut(index))
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

    // -- calibration ------------------------------------------------------

    fn begin_calibration(&mut self, player: u32) {
        let (pad, index) = match self.session_pad(player, "calibration") {
            Ok(found) => found,
            Err(event) => {
                self.broadcast(&event);
                return;
            }
        };
        let declared = self
            .session
            .as_mut()
            .and_then(|session| session.source_mut(index))
            .map(|source| source.declared_axes())
            .unwrap_or_default();
        let axes: BTreeMap<u16, padmap_core::calibration::Declared> = declared
            .into_iter()
            .filter(|(code, axis)| axis.calibratable(*code))
            .collect();
        let name = clean(&pad.name);
        if axes.is_empty() {
            // Nothing to centre -- a d-pad-only pad is already correct. Skip
            // straight to the icon step: the profile still needs writing so
            // this pad counts as configured.
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
        // The button that opened this is very likely still down; it must not
        // resume a confirm hold later.
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

    /// Leave the modal flow and hand input back to claim detection.
    fn finish_calibration(&mut self) {
        let run = self.calibration.take();
        // A button held while choosing an icon would otherwise be sitting in
        // the confirm tracker and fire the moment normal handling resumes.
        self.confirm.clear();
        self.last_confirm = 0.0;
        let player = run.map(|r| r.player).unwrap_or(0);
        self.broadcast(&events::calibration("done", 1.0, player, None, None));
        let state = self.state_event();
        self.broadcast(&state);
    }

    // -- the pickers ------------------------------------------------------

    /// Ask which console this controller is, before walking its buttons.
    fn begin_layout_choice(&mut self, player: u32) {
        let (pad, index) = match self.session_pad(player, "choosing a layout") {
            Ok(found) => found,
            Err(event) => {
                self.broadcast(&event);
                return;
            }
        };
        let axes = self.absolute_ranges(index);
        let held = self.held_keys(index);
        // Start on the best guess. A stored layout wins: it is what the user
        // chose last time.
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

    /// Ask what a mapping is *for* before asking where the buttons are.
    fn begin_scope_choice(&mut self, player: u32) {
        let (pad, index) = match self.session_pad(player, "choosing a scope") {
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
        let axes = self.absolute_ranges(index);
        let held = self.held_keys(index);
        // Asking the question again abandons whatever the last answer was.
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

    /// Ask console-or-this-game, for a game the front-end is sitting on.
    fn begin_game_scope_choice(&mut self, player: u32, console: &str, key: &str, title: &str) {
        let (pad, index) = match self.session_pad(player, "mapping") {
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
            // A mapping filed under a console padmap cannot name is one the
            // launcher will never look for.
            self.broadcast(&events::error("no console known for this game"));
            return;
        }
        let axes = self.absolute_ranges(index);
        let held = self.held_keys(index);
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

    /// Act on a picker the user just held a button to accept.
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
        // A scope answer. Every scope but the default names a console, and
        // the console *is* the control set, so the layout question is
        // skipped. The default scope is the exception: "any game" says
        // nothing about the shape of the controller, so the picker runs.
        self.pending_scope = chosen.clone();
        if chosen == scope::UNIVERSAL {
            self.begin_layout_choice(player);
        } else {
            self.begin_mapping(player, &layout, &chosen);
        }
    }

    /// Leave the picker without starting a wizard.
    fn end_layout_choice(&mut self) {
        let modal = self.choice.take();
        self.confirm.clear();
        self.last_confirm = 0.0;
        self.broadcast(&events::layout_choice_ended(modal.as_ref().map(|m| &m.run)));
        let state = self.state_event();
        self.broadcast(&state);
    }

    /// Throw away a controller's stored config and map it again, now.
    fn forget_pad(&mut self, player: u32) {
        let Some(pad) = self.pad_for_player(player) else {
            self.broadcast(&events::error(format!(
                "no controller assigned to player {player}"
            )));
            return;
        };
        // Check the wizard can actually open *before* throwing anything
        // away. Deleting first and finding no session to map in leaves the
        // controller with no configuration and no way to make one.
        let open = self
            .session
            .as_ref()
            .is_some_and(|session| session.index_of(&pad.path).is_some());
        if !open {
            self.broadcast(&events::error(
                "resetting a controller needs an open session",
            ));
            return;
        }
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

    // -- the wizard -------------------------------------------------------

    fn begin_mapping(&mut self, player: u32, layout_id: &str, scope: &str) {
        let (pad, index) = match self.session_pad(player, "mapping") {
            Ok(found) => found,
            Err(event) => {
                self.broadcast(&event);
                return;
            }
        };
        // An explicit request always wins; failing that, the same answer the
        // UI already draws, so the icon and the wizard cannot disagree.
        let chosen = if layout_id.is_empty() {
            publish::icon_for(&pad, &self.icon_overrides).to_owned()
        } else {
            layout_id.to_owned()
        };
        let layout = padmap_core::layout::for_icon(&chosen);
        let mut keys: Vec<u16> = self
            .session
            .as_mut()
            .and_then(|session| session.source_mut(index))
            .map(|source| source.capabilities().0)
            .unwrap_or_default();
        keys.sort_unstable();
        let axes = self.absolute_ranges(index);
        let held = self.held_keys(index);
        let run = MappingRun::new(player, layout, keys, scope.to_owned(), axes, held);
        // Consumed: the pending scope belongs to the run now.
        self.pending_scope.clear();
        self.confirm.clear();
        info!(
            "mapping {} as {} for scope {scope:?}",
            clean(&pad.name),
            layout.id
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

    /// Leave the modal flow, keeping what was captured if asked.
    ///
    /// Abandoning halfway keeps nothing: a partial mapping is worse than
    /// none, because the pad then counts as configured and is never offered
    /// again, leaving half its buttons dead with no indication why.
    fn finish_mapping(&mut self, store: bool) {
        let modal = self.mapping.take();
        self.confirm.clear();
        self.last_confirm = 0.0;
        self.pending_scope.clear();
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
        // Choosing an icon is the last step, so this ends the modal flow.
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

    // -- session input ----------------------------------------------------

    fn on_session_read(&mut self, index: usize) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let raw = session.read(index);
        if raw.is_empty() {
            return;
        }
        let pad_path = session.pads.get(index).map(|pad| pad.path.clone());
        let claimed = session.is_claimed(index);
        let clock = now();
        for event in &raw {
            // Every event, to whichever modal flow is reading this pad.
            if let Some(path) = &pad_path {
                self.feed_modal(path, *event, clock);
            }
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if claimed {
            // Claimed pads still have to be drained; forwarding their presses
            // is what the confirm gesture is built out of.
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
        if let Some(modal) = self.mapping.as_mut() {
            if modal.pad_path == pad_path {
                let before = modal.run.conflict();
                let outcome = modal.run.feed(capture_event, clock);
                if outcome.advanced() {
                    let update = events::mapping(&modal.run);
                    let finished = modal.run.finished();
                    self.broadcast(&update);
                    if finished {
                        self.finish_mapping(true);
                    }
                } else if modal.run.conflict().is_some() && modal.run.conflict() != before {
                    // A refused press records nothing, so the ordinary path
                    // sends nothing and the screen sits exactly as it did --
                    // which is what "it doesn't accept it" looks like from
                    // the sofa.
                    let update = events::mapping(&modal.run);
                    self.broadcast(&update);
                }
            }
        }
    }

    fn on_source_read(&mut self, index: usize) {
        let Some(republisher) = self.republisher.as_mut() else {
            return;
        };
        let pumped = republisher.forward(index);
        if pumped.gone {
            // A dead node reports readable forever; left registered, the loop
            // spins on it for as long as the daemon runs.
            if let Some(vpad) = republisher.pads.get(index) {
                let _ = self.reactor.unwatch(vpad.source.as_fd());
            }
        }
    }

    // -- the tick ---------------------------------------------------------

    fn guarded_tick(&mut self) {
        // No *device* may end the process either. The tick probes hidraw
        // nodes, opens devices and parses profiles, and a fault here costs
        // every player's clone at once, mid-game.
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| self.tick())) {
            self.tick_failures += 1;
            // Rationed: a tick that fails permanently fails 50 times a second.
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
        // Drain any client that fell behind, before anything new is queued.
        self.flush_all();
        // Read every client here too, not only when epoll reports it. The one
        // loop already wakes on a 20ms tick, so a command waits at most that
        // long, and a client is never left unread because a wakeup for its
        // descriptor was missed -- which is the difference between a front-end
        // whose next button works and one that has silently gone deaf.
        for fd in self.clients.keys().copied().collect::<Vec<_>>() {
            self.on_client_read(fd);
        }
        let mut scan = Scan::default();
        // Announcing is never modal and is allowed during a game, and a
        // consumer should hear about a controller the moment it arrives.
        self.poll_controller_changes(&mut scan);
        self.poll_new_controllers(&mut scan);
        // Before any early return: the modal flows below each end in one,
        // and a pause applied only on the paths that fall through would be
        // applied exactly never.
        self.sync_republish_pause();
        self.reap_dead_pads();
        self.republish_if_stale();

        if self.session.is_none() {
            return;
        }
        // Mapping and the pickers are modal: every prompt is answered with a
        // press, and those must not also claim slots or trip the confirm.
        if self.choice.is_some() || self.mapping.is_some() {
            return;
        }
        if self.calibration.is_some() {
            self.tick_calibration();
            return;
        }

        let clock = now();
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

    // -- republishing -----------------------------------------------------

    fn start_republisher(&mut self) -> Result<(), clone::CloneError> {
        self.stop_republisher();
        let mut vpads = Vec::with_capacity(self.slots_assigned.len());
        for slot in &self.slots_assigned {
            let axes = profiles::load(&slot.pad, None)
                .map(|profile| profile.axes)
                .unwrap_or_default();
            let vpad = clone::create(&slot.pad, slot.player, self.mode, &axes, true)?;
            vpads.push(vpad);
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
        // A fresh Republisher starts forwarding, and this runs *during* a
        // mapping; carrying the pause across the restart is the difference
        // between it holding for the whole wizard and until the first thing
        // that republishes.
        self.sync_republish_pause();

        // Regenerate the profiles the *launcher* would, not context-free
        // ones: the last game is the only context this side knows.
        let last = runtime::read_recent_games().into_iter().next();
        let written = publish::write_all(
            &self.slots_assigned,
            &virtual_paths,
            self.mode,
            &self.launch_config_path,
            &self.launch_args_path,
            last.as_ref(),
        );
        // Writing the file is not enough: a front-end loads its SDL database
        // once and never looks again, so the lines are handed over for
        // SDL_GameControllerAddMapping.
        self.sdl_lines = written.sdl_lines;
        let lines = events::sdl_mapping(&self.sdl_lines);
        self.broadcast(&lines);
        info!(
            "republishing {} pad(s); launch config at {}",
            self.slots_assigned.len(),
            self.launch_config_path.display()
        );
        Ok(())
    }

    fn stop_republisher(&mut self) {
        let Some(mut republisher) = self.republisher.take() else {
            return;
        };
        for vpad in &republisher.pads {
            let _ = self.reactor.unwatch(vpad.source.as_fd());
            let _ = self.reactor.unwatch(vpad.clone.as_fd());
        }
        republisher.close();
    }

    /// Silence the clones while a wizard is reading the physical pad.
    ///
    /// Driven from state rather than from the begin/finish methods, because
    /// a modal flow can also end by the pad being unplugged, the session
    /// being cancelled, or `configure_end` arriving.
    fn sync_republish_pause(&mut self) {
        let modal = self.mapping.is_some() || self.calibration.is_some() || self.choice.is_some();
        if let Some(republisher) = self.republisher.as_mut() {
            republisher.set_paused(modal);
        }
    }

    /// Stop watching sources that have gone, so the loop cannot spin.
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

    /// Re-open a pad whose device was replaced under us.
    ///
    /// A Bluetooth controller that drops and reconnects comes back as a new
    /// device; the descriptor we hold stops being readable and nothing
    /// errors. Restarting the republisher rebuilds from the stored
    /// assignments.
    fn republish_if_stale(&mut self) {
        let clock = now();
        if clock - self.last_stale_check < STALE_CHECK_SECONDS {
            return;
        }
        self.last_stale_check = clock;
        // The pad's device node, not `Source::path` -- that is the *phys*
        // string, which never exists on disk, so every pad looked stale and
        // the daemon rebuilt every clone every two seconds.
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
        // Whoever came back may have come back elsewhere: re-resolve each
        // assignment against what is plugged in now.
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
            // The pad may be mid-reconnect and not back yet. Next poll
            // retries; dying here would take every other player's controller.
            warn!("could not reopen after a reconnect ({error}); will retry");
        }
    }

    // -- hotplug ----------------------------------------------------------

    /// Announce controllers arriving and leaving, game running or not.
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

        // A session owns every pad while it is open; attaching underneath it
        // would claim a slot the user is in the middle of assigning.
        if self.state == STATE_ASSIGNING {
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

    /// Which Steam Controller slots have a pad in them, cheaply.
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
        // The slot is not freed: a Bluetooth pad that drops for four seconds
        // and comes back must come back as the same player.
        let event = self.controller_event(announce::ACTION_REMOVED, player, &gone, "");
        self.broadcast(&event);
    }

    /// Attach a controller that has a mapping, and say so either way.
    fn announce_arrival(&mut self, pad: Pad) {
        let signature = profiles::signature_of(&pad);
        if let Some(existing) = self
            .slots_assigned
            .iter()
            .find(|slot| profiles::signature_of(&slot.pad) == signature)
        {
            // A reconnect, not a new player.
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
        let taken: Vec<u32> = self.slots_assigned.iter().map(|slot| slot.player).collect();
        let player = announce::next_player(&taken);
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
        // Undo it: a slot claimed for a controller with no clone would have
        // the next arrival take player 3 while player 2 does not exist.
        self.slots_assigned.retain(|slot| slot.player != player);
        self.attached.live.remove(&signature);
        if !self.attached.failed(&signature) {
            // Come straight back: the usual cause is a udev ACL that has not
            // landed yet, and it lands in milliseconds.
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

    /// Republish, then announce the pad that is now live -- in that order,
    /// because the event names the clone's node.
    fn rebuild_for_attach(&mut self, player: u32, pad: &Pad) -> bool {
        if let Err(error) = self.start_republisher() {
            // Never fatal: losing every other player's clone because a fifth
            // controller could not be opened would take a working game down.
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

    /// Build the message, filling in whatever the republisher knows.
    fn controller_event(&mut self, action: &str, player: u32, pad: &Pad, reason: &str) -> Value {
        let last = runtime::read_recent_games().into_iter().next();
        let console = last.as_ref().map(|g| g.console.clone()).unwrap_or_default();
        let game = last.as_ref().map(|g| g.key.clone()).unwrap_or_default();
        let title = last.as_ref().map(|g| g.title.clone()).unwrap_or_default();

        // Indices come from the *clones*, because that is what RetroArch
        // enumerates -- the physical pads are hidden.
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

    // -- the setup screen -------------------------------------------------

    /// Why a first-time controller must not open setup now, or `None`.
    fn autosetup_blocked(&self) -> Option<&'static str> {
        if std::env::var(ENV_NO_AUTOSETUP).as_deref() == Ok("1") {
            return Some("disabled by PADMAP_NO_AUTOSETUP");
        }
        if self.state == STATE_ASSIGNING {
            return Some("a session is already open");
        }
        // A *settled* client, not merely a connected socket: `ensure-daemon`
        // connects for milliseconds to read status, and counting those meant
        // grabbing every pad to display a screen on a front-end that was not
        // running.
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

    /// Open setup when a controller model is seen for the first time.
    fn poll_new_controllers(&mut self, scan: &mut Scan) {
        let clock = now();
        if clock - self.last_pad_scan < PAD_SCAN_SECONDS {
            return;
        }
        self.last_pad_scan = clock;
        // Nothing plugged or unplugged since the last look? Then a full scan
        // could discover nothing, and a full scan is expensive on the thread
        // that forwards controller events.
        let nodes = hotplug::event_nodes();
        let stamp = stamp_of(&self.prompted_path);
        if self.last_scan_signature.as_ref() == Some(&(nodes.clone(), stamp)) {
            return;
        }
        // Ask first whether setup could open at all, because everything
        // below is expensive and this is not. Deliberately without recording
        // the signature: being blocked is temporary.
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
        // Mark before starting: if begin fails, the user still gets to reach
        // setup by hand rather than being re-prompted every second.
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
            // Not fatal: the worst case is offering setup again next launch.
            warn!("could not record prompted controllers: {error}");
        }
        // Our own write is not a change to react to.
        self.prompted_stamp = stamp_of(&self.prompted_path);
    }

    // -- state ------------------------------------------------------------

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
        source
            .into_iter()
            .map(|(player, pad)| PlayerState {
                player,
                name: clean(&pad.name),
                node: pad.event().to_owned(),
                icon: publish::icon_for(&pad, &self.icon_overrides).to_owned(),
                configured: publish::has_mapping(&pad),
                mappings: publish::mapping_scopes(&pad),
            })
            .collect()
    }

    fn state_event(&self) -> Value {
        events::state(
            self.state,
            self.slots,
            self.players_payload(),
            runtime::build_id_of_binary(),
            self.mode.as_str(),
        )
    }

    fn save_assignments(&self) {
        let entries: Vec<assignments::Assignment> = self
            .slots_assigned
            .iter()
            .map(to_input_assignment)
            .collect();
        if let Err(error) = assignments::save(&self.state_path, &entries) {
            warn!("could not save assignments: {error}");
        }
    }
}

fn as_player(player: i64) -> u32 {
    u32::try_from(player).unwrap_or(0)
}

fn stamp_of(path: &Path) -> i128 {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_nanos() as i128)
        .unwrap_or(0)
}

/// Is something listening on this socket?
fn daemon_alive(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}
