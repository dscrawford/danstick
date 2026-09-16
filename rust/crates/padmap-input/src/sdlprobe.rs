//! What SDL's own database says about a GUID.
//!
//! SDL compiles its controller database into the library as a C array, so
//! there is no file to read: the library is the only place the answer exists.
//! This is the last-resort lookup for a pad nobody has mapped -- before
//! guessing from capabilities, ask whether SDL already knows the controller.
//!
//! **Run this in a throwaway process.** `SDL_Init` starts threads, opens a
//! udev monitor and enumerates every joystick on the machine, and none of
//! that belongs in a daemon that holds `EVIOCGRAB` on those same devices.
//! [`builtin_mapping`] is the in-process call; [`isolated`] re-executes this
//! binary as `padmap-rs sdl-mapping` so the daemon never initialises SDL
//! itself -- the same arrangement the Python had, with `pysdl2` in a
//! subprocess.
//!
//! Linked, not `dlopen`ed. On the machines this runs on SDL3 is always there,
//! and a link-time dependency is one the package system can see.

use std::ffi::{c_char, c_void, CStr, CString};

/// `SDL_GUID`: sixteen bytes, passed by value.
#[repr(C)]
#[derive(Clone, Copy)]
struct SdlGuid {
    data: [u8; 16],
}

/// `SDL_INIT_GAMEPAD`, which implies `SDL_INIT_JOYSTICK`.
const SDL_INIT_GAMEPAD: u32 = 0x0000_2000;

// The workspace warns on unsafe. Calling C is unsafe by definition; what makes
// these calls sound is that every signature below is copied from the SDL3
// headers the package is built against (SDL_init.h, SDL_hints.h, SDL_guid.h,
// SDL_gamepad.h, SDL_stdinc.h), and `SdlGuid` is `repr(C)` with the same one
// field. A mismatch would not be subtle: it would crash the probe process,
// which is exactly why the probe gets a process of its own.
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

/// Hints, set before `SDL_Init` so they take effect during it.
///
/// Every device ignored: enumerating is not wanted and is not free, and doing
/// it while the daemon holds the pads grabbed is worth avoiding on principle.
/// A vid/pid no device has leaves SDL with the database loaded and nothing
/// open -- verified as `joysticks 0` with the mapping still returned.
const IGNORE_ALL_DEVICES: (&str, &str) =
    ("SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT", "0xffff/0xffff");

/// Environment SDL reads that must *not* reach the probe.
///
/// Both name mappings padmap itself wrote. The file scan already covered
/// them, with the check that skips padmap's own lines -- which SDL cannot
/// make. Leaving them set fed a line padmap wrote for a virtual pad straight
/// back as though the controller had come with it.
const UNSET: [&str; 2] = ["SDL_GAMECONTROLLERCONFIG", "SDL_GAMECONTROLLERCONFIG_FILE"];

/// Why the probe could not answer. Only ever a reason to log.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("SDL could not initialise its gamepad subsystem")]
    Init,
    #[error("the GUID has a NUL in it")]
    Nul,
}

/// SDL's mapping line for this GUID, or `None` if it has never heard of it.
///
/// In-process: see the module note about why the caller should not be the
/// daemon. Initialises and quits SDL around the one call, so the process is
/// left as it was found.
#[allow(unsafe_code)]
pub fn builtin_mapping(guid: &str) -> Result<Option<String>, ProbeError> {
    for name in UNSET {
        std::env::remove_var(name);
    }
    let (hint, value) = IGNORE_ALL_DEVICES;
    let hint = CString::new(hint).map_err(|_| ProbeError::Nul)?;
    let value = CString::new(value).map_err(|_| ProbeError::Nul)?;
    let text = CString::new(guid).map_err(|_| ProbeError::Nul)?;

    // SAFETY: both strings are NUL-terminated CStrings that outlive the call;
    // SDL copies the hint.
    unsafe { SDL_SetHint(hint.as_ptr(), value.as_ptr()) };
    // SAFETY: no arguments but a flag word.
    if !unsafe { SDL_Init(SDL_INIT_GAMEPAD) } {
        return Err(ProbeError::Init);
    }
    // SAFETY: `text` is NUL-terminated and outlives the call. The returned
    // pointer is either null or a string SDL allocated for us to free.
    let found = unsafe {
        let parsed = SDL_StringToGUID(text.as_ptr());
        SDL_GetGamepadMappingForGUID(parsed)
    };
    let line = if found.is_null() {
        None
    } else {
        // SAFETY: non-null, NUL-terminated, and ours until SDL_free.
        let owned = unsafe { CStr::from_ptr(found) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: allocated by SDL, freed exactly once.
        unsafe { SDL_free(found.cast()) };
        Some(owned)
    };
    // SAFETY: paired with the SDL_Init above.
    unsafe { SDL_Quit() };
    Ok(line)
}

/// The same answer, from a process that is not this one.
///
/// Spawns this binary as `padmap-rs sdl-mapping <guid>` and reads its stdout.
/// Never errors: SDL missing, the probe crashing, or a controller SDL has not
/// heard of are all the ordinary "no entry", and the caller's next step -- a
/// guess from capabilities -- is the same in every case.
pub fn isolated(guid: &str) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let output = std::process::Command::new(exe)
        .args(["sdl-mapping", guid])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!line.is_empty()).then_some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, sequential: `SDL_Init`/`SDL_Quit` are process-global and the
    /// harness runs tests on threads.
    #[test]
    fn the_built_in_database_is_reachable() {
        // DualShock 4, bus 3, 054c:05c4. Chosen over the Xbox 360 pad on
        // purpose: SDL3 has no explicit line for xpad devices and recognises
        // them by heuristic when one is plugged in, so that GUID answers
        // nothing here and proves nothing about the database.
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

        // Nobody's pad. The ids are padmap's own, which SDL has no entry
        // for -- the exact case the fallback guess exists for.
        let nobody = builtin_mapping("0600c9a7091200000100000001000000").expect("init");
        assert_eq!(nobody, None);

        // A GUID with a NUL cannot even be asked about.
        assert!(matches!(builtin_mapping("abc\0def"), Err(ProbeError::Nul)));
    }
}
