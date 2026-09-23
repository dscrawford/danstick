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
    let line = ask(guid);
    if let Ok(mut cache) = known.lock() {
        cache.insert(guid.to_owned(), line.clone());
    }
    line
}

fn ask(guid: &str) -> Option<String> {
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

/// What SDL calls this GUID's pad, cached per GUID since the lookup shells
/// out to SDL; None if SDL has never heard of it.
pub fn name_for(guid: &str) -> Option<String> {
    isolated(guid).and_then(|line| parsed_name(&line))
}

/// The second field of an SDL database line: the name SDL reports.
fn parsed_name(line: &str) -> Option<String> {
    line.split(',')
        .nth(1)
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_lines_second_field_is_the_name() {
        assert_eq!(
            parsed_name("030000004c050000c405000011810000,PS4 Controller,a:b0,platform:Linux,"),
            Some("PS4 Controller".to_owned())
        );
        assert_eq!(
            parsed_name("guid, spaced ,a:b0,"),
            Some("spaced".to_owned())
        );
    }

    #[test]
    fn a_line_that_names_nothing_names_nothing() {
        assert_eq!(parsed_name("justaguidwithnocommas"), None);
        assert_eq!(
            parsed_name("guid,,a:b0,"),
            None,
            "an empty field is unknown"
        );
        assert_eq!(parsed_name("guid,   ,a:b0,"), None);
        assert_eq!(parsed_name(""), None);
    }

    #[test]
    fn a_comma_in_a_name_truncates_it_and_that_is_known() {
        // SDL writes its own database and has never put a comma in a name;
        // splitting on commas is what SDL's own parser does too.
        assert_eq!(
            parsed_name("guid,Generic Joystick, rev 1.1,a:b0,"),
            Some("Generic Joystick".to_owned())
        );
    }

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
}
