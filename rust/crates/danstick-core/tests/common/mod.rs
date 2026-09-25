//! The corpus predates the rename from padmap and is kept byte for byte, so
//! its answers are read as they would be written now.

use danstick_core::sdl::crc16;

/// `text` with padmap's names as danstick's, and every virtual pad's SDL GUID
/// carrying the checksum of its new name.
pub fn renamed(text: &str) -> String {
    let mut out = text
        .replace("padmap", "danstick")
        .replace("Padmap", "Danstick")
        .replace("PADMAP", "DANSTICK");
    for player in 1..=16 {
        let old = checksum(&format!("padmap Player {player}"));
        let new = checksum(&format!("danstick Player {player}"));
        out = rewrite_guids(&out, &old, &new);
    }
    out
}

/// The name checksum as it sits in a GUID's second word.
fn checksum(name: &str) -> String {
    let crc = crc16(name.as_bytes());
    format!("{:02x}{:02x}", crc & 0xff, crc >> 8)
}

/// Swap `old` for `new` in the second word of every 32-digit GUID in `text`.
fn rewrite_guids(text: &str, old: &str, new: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < bytes.len() {
        let run = bytes[at..]
            .iter()
            .take_while(|b| b.is_ascii_hexdigit())
            .count();
        if run == 32 && &text[at + 4..at + 8] == old {
            out.push_str(&text[at..at + 4]);
            out.push_str(new);
            out.push_str(&text[at + 8..at + 32]);
            at += 32;
        } else if run > 0 {
            out.push_str(&text[at..at + run]);
            at += run;
        } else {
            let next = text[at..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&text[at..at + next]);
            at += next;
        }
    }
    out
}
