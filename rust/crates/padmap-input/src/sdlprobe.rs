//! SDL's controller database lookup (last resort before guessing from capabilities).

use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};

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

/// Same answer from a subprocess (spawns `padmap-rs sdl-mapping <guid>`; never errors).
pub fn isolated(guid: &str) -> Option<String> {
    // Remembered per GUID: half a second in a subprocess, and the daemon
    // rewrites every consumer's config on every seat change.
    static KNOWN: OnceLock<Mutex<BTreeMap<String, Option<String>>>> = OnceLock::new();
    let known = KNOWN.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(cache) = known.lock() {
        if let Some(found) = cache.get(guid) {
            return found.clone();
        }
    }
    // Only an answer is remembered: caching a probe that could not run would deny
    // that pad its mapping for the rest of the daemon's life.
    let Ok(line) = ask(guid) else {
        return None;
    };
    if let Ok(mut cache) = known.lock() {
        cache.insert(guid.to_owned(), line.clone());
    }
    line
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
