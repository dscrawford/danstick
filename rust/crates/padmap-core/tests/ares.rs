//! ares bindings, checked against a settings.bml ares itself wrote.
//!
//! ares reads raw SDL joystick state, so every number in a binding is a
//! position on the device rather than a standard id -- which means a wrong one
//! does not fail, it binds a different button. The reference file is one ares
//! produced on the development machine, and the control names are taken from
//! it rather than from the documentation: ares keeps its own unbound entry
//! beside any name it does not recognise, so a misspelling leaves the control
//! dead with nothing to say why.

use std::path::Path;

use padmap_core::ares::{self, Half, Indices, Source};

fn ares_own() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/ares_settings.bml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The control names ares wrote under its own `VirtualPad1`.
fn reference_controls() -> Vec<String> {
    let text = ares_own();
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        if line == "VirtualPad1" {
            inside = true;
            continue;
        }
        if inside {
            if !line.starts_with("  ") || line.starts_with("   ") {
                break;
            }
            let Some((name, _)) = line.trim().split_once(':') else {
                break;
            };
            out.push(name.to_owned());
        }
    }
    out
}

/// A standard pad: the eleven keys and six axes plus a hat that SDL reported
/// as `axes=6 hats=1 buttons=11` when measured.
fn standard() -> Indices {
    Indices::of(
        &[
            0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13A, 0x13B, 0x13C, 0x13D, 0x13E,
        ],
        &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x10, 0x11],
    )
}

#[test]
fn every_control_name_is_one_ares_uses() {
    // ares does not reject a name it does not know; it keeps its own entry
    // beside it and the control stays unbound. So the only way to be sure is
    // to compare against a file ares wrote.
    let theirs = reference_controls();
    assert!(!theirs.is_empty(), "the reference file has no VirtualPad1");
    let ours: Vec<&str> = ares::CONTROLS.iter().map(|(name, _)| *name).collect();
    for name in &ours {
        assert!(
            theirs.iter().any(|known| known == name),
            "{name:?} is not a control ares writes; it has {theirs:?}"
        );
    }
    // Rumble is the one ares has that padmap does not bind.
    assert!(theirs.contains(&"Rumble".to_owned()));
    assert_eq!(ours.len() + 1, theirs.len(), "ours {ours:?} vs {theirs:?}");
}

#[test]
fn an_index_is_the_ordinal_in_ascending_evdev_order() {
    // Measured against SDL 3: a device declaring these eleven keys and six
    // non-hat axes was reported as axes=6 hats=1 buttons=11.
    let indices = standard();
    // BTN_SOUTH is the lowest key code, so button 0.
    assert_eq!(
        ares::assignment("G", Source::Button(0x130), &indices).as_deref(),
        Some("G/0/3/0")
    );
    // BTN_EAST is next.
    assert_eq!(
        ares::assignment("G", Source::Button(0x131), &indices).as_deref(),
        Some("G/0/3/1")
    );
    // ABS_X is axis 0, ABS_RZ is axis 5 -- the hat codes are not counted.
    assert_eq!(
        ares::assignment("G", Source::Axis(0x00, Some(Half::Lo)), &indices).as_deref(),
        Some("G/0/0/0/Lo")
    );
    assert_eq!(
        ares::assignment("G", Source::Axis(0x05, Some(Half::Hi)), &indices).as_deref(),
        Some("G/0/0/5/Hi")
    );
}

#[test]
fn the_hat_is_two_inputs_x_then_y() {
    let indices = standard();
    assert_eq!(
        ares::assignment(
            "G",
            Source::Hat {
                vertical: false,
                half: Half::Lo
            },
            &indices
        )
        .as_deref(),
        Some("G/0/1/0/Lo"),
        "left is hat X negative"
    );
    assert_eq!(
        ares::assignment(
            "G",
            Source::Hat {
                vertical: true,
                half: Half::Lo
            },
            &indices
        )
        .as_deref(),
        Some("G/0/1/1/Lo"),
        "up is hat Y negative"
    );
}

#[test]
fn a_control_the_pad_does_not_have_is_left_unbound() {
    // Pointing it at index zero would bind a real button to a control the
    // user never pressed, which is worse than a dead entry they can see.
    let no_hat = Indices::of(&[0x130], &[0x00, 0x01]);
    assert_eq!(
        ares::assignment(
            "G",
            Source::Hat {
                vertical: true,
                half: Half::Lo
            },
            &no_hat
        ),
        None
    );
    assert_eq!(ares::assignment("G", Source::Button(0x137), &no_hat), None);
    assert_eq!(
        ares::assignment("G", Source::Axis(0x05, Some(Half::Hi)), &no_hat),
        None
    );

    let block = ares::virtual_pad(1, "G", &no_hat);
    assert!(block.contains("Pad.Up: ;;"), "{block}");
    assert!(block.contains("A..South: G/0/3/0;;"), "{block}");
}

#[test]
fn the_block_is_shaped_the_way_ares_writes_one() {
    let block = ares::virtual_pad(3, "abc", &standard());
    assert!(block.starts_with("VirtualPad3\n"));
    // Two-space indent, `Name: value`, three alternatives joined by ';'.
    for line in block.lines().skip(1) {
        assert!(line.starts_with("  "), "{line:?}");
        assert!(!line.starts_with("   "), "{line:?}");
        assert!(line.ends_with(";;"), "{line:?}");
    }
    assert!(block.contains("  A..South: abc/0/3/0;;"), "{block}");
    assert!(block.ends_with("  Rumble: ;;\n"));
}

#[test]
fn only_five_ports_exist() {
    // ares has five; padmap's own limit is higher, and a sixth player simply
    // has no port rather than overwriting the first.
    assert_eq!(ares::MAX_PLAYERS, 5);
    assert!(ares_own().contains("VirtualPad5"));
    assert!(!ares_own().contains("VirtualPad6"));
}
