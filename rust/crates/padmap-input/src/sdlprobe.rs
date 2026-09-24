//! SDL's controller database lookup (last resort before guessing from capabilities).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};

/// Sixteen bytes, passed by value.
#[repr(C)]
#[derive(Clone, Copy)]
struct SdlGuid {
    data: [u8; 16],
}

const SDL_INIT_GAMEPAD: u32 = 0x0000_2000;

#[allow(unsafe_code)]
#[link(name = "SDL3")]
extern "C" {
    fn SDL_Init(flags: u32) -> bool;
    fn SDL_Quit();
    fn SDL_SetHint(name: *const c_char, value: *const c_char) -> bool;
    fn SDL_StringToGUID(text: *const c_char) -> SdlGuid;
    fn SDL_GetGamepadMappingForGUID(guid: SdlGuid) -> *mut c_char;
    fn SDL_free(mem: *mut c_void);
}

// Ignore all devices: enumeration not wanted, especially while daemon holds pads grabbed.
const IGNORE_ALL_DEVICES: (&str, &str) =
    ("SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT", "0xffff/0xffff");

const UNSET: [&str; 2] = ["SDL_GAMECONTROLLERCONFIG", "SDL_GAMECONTROLLERCONFIG_FILE"];

/// Why the probe could not answer.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("SDL could not initialise its gamepad subsystem")]
    Init,
    #[error("the GUID has a NUL in it")]
    Nul,
}

/// SDL's mapping line for this GUID, or None if unknown.
#[allow(unsafe_code)]
pub fn builtin_mapping(guid: &str) -> Result<Option<String>, ProbeError> {
    for name in UNSET {
        std::env::remove_var(name);
    }
    let (hint, value) = IGNORE_ALL_DEVICES;
    let hint = CString::new(hint).map_err(|_| ProbeError::Nul)?;
    let value = CString::new(value).map_err(|_| ProbeError::Nul)?;
    let text = CString::new(guid).map_err(|_| ProbeError::Nul)?;

    unsafe { SDL_SetHint(hint.as_ptr(), value.as_ptr()) };
    if !unsafe { SDL_Init(SDL_INIT_GAMEPAD) } {
        return Err(ProbeError::Init);
    }
    let found = unsafe {
        let parsed = SDL_StringToGUID(text.as_ptr());
        SDL_GetGamepadMappingForGUID(parsed)
    };
    let line = if found.is_null() {
        None
    } else {
        let owned = unsafe { CStr::from_ptr(found) }
            .to_string_lossy()
            .into_owned();
        unsafe { SDL_free(found.cast()) };
        Some(owned)
    };
    unsafe { SDL_Quit() };
    Ok(line)
}

/// What SDL's database says about a GUID, as far as anybody has asked yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// Asked and answered: SDL's own line, or that it has none.
    Known(Option<String>),
    /// Being asked on the prober's thread; use something else for now.
    Pending,
}

type Asker = Box<dyn Fn(&str) -> Result<Option<String>, ()> + Send>;

#[derive(Debug, Default)]
struct Shared {
    known: Mutex<BTreeMap<String, Option<String>>>,
    asking: Mutex<BTreeSet<String>>,
    /// Bumped by every answer, so a caller can tell there is something new.
    generation: AtomicU64,
}

/// Asks SDL on a thread of its own, one GUID at a time.
///
/// A probe is a subprocess that initialises SDL, about half a second, and it
/// used to run on the daemon's event loop -- which forwards every seated
/// player's presses -- once for every new pad that claimed without a mapping.
#[derive(Debug)]
pub struct Prober {
    shared: Arc<Shared>,
    queue: mpsc::Sender<String>,
}

impl Prober {
    pub fn new(ask: impl Fn(&str) -> Result<Option<String>, ()> + Send + 'static) -> Prober {
        let shared = Arc::new(Shared::default());
        let (queue, asked) = mpsc::channel::<String>();
        let worker = shared.clone();
        let ask: Asker = Box::new(ask);
        let spawned = std::thread::Builder::new()
            .name("sdl-probe".to_owned())
            .spawn(move || {
                for guid in asked {
                    // A probe that panics is a probe that failed, not the end of
                    // every answer after it.
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ask(&guid)))
                            .unwrap_or(Err(()));
                    // Remembered before it stops being asked, so a lookup in
                    // between never sees neither and queues it again.
                    if let Ok(line) = result {
                        if let Ok(mut known) = worker.known.lock() {
                            known.insert(guid.clone(), line);
                        }
                        worker.generation.fetch_add(1, Ordering::SeqCst);
                    }
                    if let Ok(mut asking) = worker.asking.lock() {
                        asking.remove(&guid);
                    }
                }
            });
        if let Err(error) = spawned {
            log::warn!("SDL's database cannot be asked without blocking: {error}");
        }
        Prober { shared, queue }
    }

    /// SDL's answer if it has one, else `Pending` -- never a wait.
    pub fn lookup(&self, guid: &str) -> Lookup {
        if let Ok(known) = self.shared.known.lock() {
            if let Some(found) = known.get(guid) {
                return Lookup::Known(found.clone());
            }
        }
        self.prefetch(guid);
        Lookup::Pending
    }

    /// Start asking about `guid` now, so the answer is there by the time
    /// somebody needs it. Asking twice at once is asking once.
    pub fn prefetch(&self, guid: &str) {
        if guid.is_empty() {
            return;
        }
        if self
            .shared
            .known
            .lock()
            .is_ok_and(|known| known.contains_key(guid))
        {
            return;
        }
        let fresh = self
            .shared
            .asking
            .lock()
            .is_ok_and(|mut asking| asking.insert(guid.to_owned()));
        if fresh && self.queue.send(guid.to_owned()).is_err() {
            if let Ok(mut asking) = self.shared.asking.lock() {
                asking.remove(guid);
            }
        }
    }

    /// Changes whenever SDL has answered something new.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::SeqCst)
    }
}

static PROBER: OnceLock<Prober> = OnceLock::new();

/// The daemon's prober, asking through a `padmap-rs sdl-mapping` subprocess.
pub fn shared() -> &'static Prober {
    PROBER.get_or_init(|| Prober::new(ask))
}

/// How many answers the daemon's prober has had; 0 if nothing ever asked.
pub fn answered() -> u64 {
    PROBER.get().map(Prober::generation).unwrap_or(0)
}

/// `Err` when the probe could not be run or failed; `Ok(None)` when it ran and
/// SDL had nothing for this GUID.
fn ask(guid: &str) -> Result<Option<String>, ()> {
    let exe = std::env::current_exe().map_err(|_| ())?;
    let output = std::process::Command::new(exe)
        .args(["sdl-mapping", guid])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| ())?;
    answer(
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
    )
}

/// What the probe's exit and output mean, which is what decides whether the
/// answer is worth remembering.
fn answer(ran: bool, stdout: &str) -> Result<Option<String>, ()> {
    if !ran {
        return Err(());
    }
    let line = stdout.trim();
    Ok((!line.is_empty()).then(|| line.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, sequential: `SDL_Init`/`SDL_Quit` are process-global and the harness runs tests on threads.
    #[test]
    fn the_built_in_database_is_reachable() {
        let ds4 = builtin_mapping("030000004c050000c405000011810000");
        let ds4 = match ds4 {
            Ok(found) => found,
            Err(ProbeError::Init) => {
                eprintln!("skipped: SDL could not initialise here (no udev?)");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        let line = ds4.expect("SDL knows the DualShock 4");
        assert!(
            line.contains("PS4 Controller") && line.contains("a:b0"),
            "{line}"
        );

        let nobody = builtin_mapping("0600c9a7091200000100000001000000").expect("init");
        assert_eq!(nobody, None);

        assert!(matches!(builtin_mapping("abc\0def"), Err(ProbeError::Nul)));
    }

    /// What a test hands the gated asker: the answer it should give next.
    type Release = std::sync::mpsc::Sender<Result<Option<String>, ()>>;
    type Calls = std::sync::Arc<std::sync::atomic::AtomicUsize>;

    /// An asker that answers only when the test lets it, and counts its calls.
    fn gated() -> (Prober, Release, Calls) {
        let (release, gate) = std::sync::mpsc::channel::<Result<Option<String>, ()>>();
        let gate = std::sync::Mutex::new(gate);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        let prober = Prober::new(move |_guid: &str| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            gate.lock().expect("gate").recv().unwrap_or(Err(()))
        });
        (prober, release, calls)
    }

    fn settle(prober: &Prober, from: u64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while prober.generation() == from && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn a_lookup_never_waits_for_sdl_to_answer() {
        let (prober, release, _) = gated();
        let started = std::time::Instant::now();
        assert_eq!(prober.lookup("g1"), Lookup::Pending);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "the lookup waited for an asker that has not answered"
        );
        let before = prober.generation();
        release
            .send(Ok(Some("g1,Pad,a:b0,".to_owned())))
            .expect("release");
        settle(&prober, before);
        assert_eq!(
            prober.lookup("g1"),
            Lookup::Known(Some("g1,Pad,a:b0,".to_owned()))
        );
        assert!(prober.generation() > before, "an answer said nothing");
    }

    #[test]
    fn sdl_knowing_nothing_is_an_answer_and_a_failed_probe_is_not() {
        let (prober, release, calls) = gated();
        prober.lookup("nobody");
        let before = prober.generation();
        release.send(Ok(None)).expect("release");
        settle(&prober, before);
        assert_eq!(prober.lookup("nobody"), Lookup::Known(None));

        prober.lookup("flaky");
        release.send(Err(())).expect("release");
        // A failure bumps nothing, so wait for the asker to have run instead.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while calls.load(std::sync::atomic::Ordering::SeqCst) < 2
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(
            prober.lookup("flaky"),
            Lookup::Pending,
            "one bad moment denied the pad its mapping for good"
        );
        release.send(Ok(None)).expect("release the retry");
    }

    #[test]
    fn a_guid_being_asked_is_not_asked_again() {
        let (prober, release, calls) = gated();
        prober.prefetch("g");
        prober.prefetch("g");
        assert_eq!(prober.lookup("g"), Lookup::Pending);
        let before = prober.generation();
        release.send(Ok(None)).expect("release");
        settle(&prober, before);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn an_asker_that_panics_does_not_stop_the_next_answer() {
        let prober = Prober::new(|guid: &str| {
            if guid == "boom" {
                panic!("the probe fell over");
            }
            Ok(Some(format!("{guid},Pad,a:b0,")))
        });
        prober.lookup("boom");
        let before = prober.generation();
        prober.lookup("fine");
        settle(&prober, before);
        assert_eq!(
            prober.lookup("fine"),
            Lookup::Known(Some("fine,Pad,a:b0,".to_owned()))
        );
    }

    #[test]
    fn a_probe_that_could_not_run_is_not_sdl_knowing_nothing() {
        // Only `Ok` is remembered, so a probe that failed has to say so rather
        // than answer None: otherwise one bad moment denies a pad its mapping
        // for the rest of the daemon's life.
        assert_eq!(
            answer(false, ""),
            Err(()),
            "a failed probe is not an answer"
        );
        assert_eq!(
            answer(false, "guid,Pad,a:b0,"),
            Err(()),
            "output without a zero exit"
        );
        assert_eq!(answer(true, "  "), Ok(None), "it ran; SDL knows nothing");
        assert_eq!(
            answer(true, " guid,Pad,a:b0, \n"),
            Ok(Some("guid,Pad,a:b0,".to_owned()))
        );
    }
}
