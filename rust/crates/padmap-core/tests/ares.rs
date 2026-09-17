//! ares bindings, checked against a settings.bml ares itself wrote.

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

/// Measured on SDL 3 as `axes=6 hats=1 buttons=11`.
fn standard() -> Indices {
    Indices::of(
        &[
            0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13A, 0x13B, 0x13C, 0x13D, 0x13E,
        ],
        &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x10, 0x11],
    )
}

fn hat(vertical: bool, half: Half) -> Source {
    Source::Hat { vertical, half }
}

#[test]
fn every_control_name_is_one_ares_uses() {
    // ares keeps an unbound entry beside a name it does not know, so only its own file can say.
    let theirs = reference_controls();
    assert!(!theirs.is_empty(), "the reference file has no VirtualPad1");
    let ours: Vec<&str> = ares::CONTROLS.iter().map(|(name, _)| *name).collect();
    for name in &ours {
        assert!(
            theirs.iter().any(|known| known == name),
            "{name:?} is not a control ares writes"
        );
    }
    // Rumble is the one ares has that padmap does not bind.
    assert!(theirs.contains(&"Rumble".to_owned()));
    assert_eq!(ours.len() + 1, theirs.len(), "ours {ours:?} vs {theirs:?}");
}

#[test]
fn an_index_is_the_ordinal_in_ascending_evdev_order() {
    let indices = standard();
    for (name, source, want) in [
        ("BTN_SOUTH is button 0", Source::Button(0x130), "G/0/3/0"),
        ("BTN_EAST is next", Source::Button(0x131), "G/0/3/1"),
        (
            "ABS_X is axis 0",
            Source::Axis(0x00, Some(Half::Lo)),
            "G/0/0/0/Lo",
        ),
        (
            "ABS_RZ is axis 5; hats not counted",
            Source::Axis(0x05, Some(Half::Hi)),
            "G/0/0/5/Hi",
        ),
        ("left is hat X negative", hat(false, Half::Lo), "G/0/1/0/Lo"),
        ("up is hat Y negative", hat(true, Half::Lo), "G/0/1/1/Lo"),
    ] {
        assert_eq!(
            ares::assignment("G", source, &indices).as_deref(),
            Some(want),
            "{name}"
        );
    }
}

#[test]
fn a_control_the_pad_does_not_have_is_left_unbound() {
    // Index zero would bind a real button to a control the user never pressed.
    let no_hat = Indices::of(&[0x130], &[0x00, 0x01]);
    for source in [
        hat(true, Half::Lo),
        Source::Button(0x137),
        Source::Axis(0x05, Some(Half::Hi)),
    ] {
        assert_eq!(ares::assignment("G", source, &no_hat), None, "{source:?}");
    }
    let block = ares::virtual_pad(1, "G", &no_hat);
    assert!(block.contains("Pad.Up: ;;"), "{block}");
    assert!(block.contains("A..South: G/0/3/0;;"), "{block}");
}

#[test]
fn the_block_is_shaped_the_way_ares_writes_one() {
    let block = ares::virtual_pad(3, "abc", &standard());
    assert!(block.starts_with("VirtualPad3\n"));
    for line in block.lines().skip(1) {
        assert!(
            line.starts_with("  ") && !line.starts_with("   "),
            "{line:?}"
        );
        assert!(line.ends_with(";;"), "{line:?}");
    }
    assert!(block.contains("  A..South: abc/0/3/0;;"), "{block}");
    assert!(block.ends_with("  Rumble: ;;\n"));
}

#[test]
fn only_five_ports_exist() {
    assert_eq!(ares::MAX_PLAYERS, 5);
    assert!(ares_own().contains("VirtualPad5"));
    assert!(!ares_own().contains("VirtualPad6"));
}
