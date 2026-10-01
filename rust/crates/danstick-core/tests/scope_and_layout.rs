//! Integration tests for scope and layout data.

use std::collections::BTreeSet;

use danstick_core::control::Control;
use danstick_core::layout::{self, Layout, LayoutControl, Shape};
use danstick_core::scope::{self, Scope};

fn shipped_ids() -> Vec<&'static str> {
    layout::all().iter().map(|l| l.id.as_str()).collect()
}

#[test]
fn all_three_kinds_of_scope_survive_construction_and_extraction() {
    // Scopes must survive round-trip; else mappings can't be filed and found.
    assert_eq!(scope::console_of(&scope::console("gamecube")), "gamecube");
    assert_eq!(
        scope::game_of(&scope::game("n64/goldeneye-007")),
        "n64/goldeneye-007"
    );
    assert_eq!(scope::console_of(scope::UNIVERSAL), "");
    assert_eq!(scope::game_of(scope::UNIVERSAL), "");
}

#[test]
fn a_console_scope_is_never_read_as_a_game_scope() {
    for id in shipped_ids() {
        let built = scope::console(id);
        assert_eq!(
            scope::game_of(&built),
            "",
            "{built} leaked into the game level"
        );
    }
}

#[test]
fn a_game_scope_is_never_read_as_a_console_scope() {
    for id in shipped_ids() {
        let built = scope::game(&format!("{id}/some-rom"));
        assert_eq!(
            scope::console_of(&built),
            "",
            "{built} leaked into the console level"
        );
    }
}

#[test]
fn a_scope_string_that_is_neither_prefix_resolves_as_universal() {
    // Hand-edited profiles must still load; refusing loses every mapping in the file.
    for raw in [
        "nonsense",
        "console",
        "game",
        "Console:n64",
        " console:n64",
        "n64",
    ] {
        assert_eq!(
            Scope::parse(raw),
            Scope::Universal,
            "{raw:?} should fall back"
        );
        assert_eq!(scope::console_of(raw), "");
        assert_eq!(scope::game_of(raw), "");
    }
}

#[test]
fn an_empty_payload_still_makes_a_well_formed_scope_string() {
    // Empty payload must stay syntactically its type, not collapse to universal.
    assert_eq!(scope::console(""), "console:");
    assert_eq!(scope::game(""), "game:");
    assert_ne!(scope::console(""), scope::UNIVERSAL);
    assert_ne!(scope::game(""), scope::UNIVERSAL);
}

#[test]
fn precedence_is_game_then_console_then_universal() {
    assert_eq!(
        scope::order("n64", "n64/goldeneye-007"),
        ["game:n64/goldeneye-007", "console:n64", ""]
    );
}

#[test]
fn a_context_with_a_console_but_no_game_omits_the_game_level() {
    assert_eq!(scope::order("gamecube", ""), ["console:gamecube", ""]);
}

#[test]
fn a_context_with_a_game_but_no_console_omits_the_console_level() {
    assert_eq!(
        scope::order("", "unknown/sonic"),
        ["game:unknown/sonic", ""]
    );
}

#[test]
fn a_context_with_neither_is_just_the_universal_scope() {
    assert_eq!(scope::order("", ""), [""]);
}

#[test]
fn an_omitted_level_is_skipped_rather_than_filled_with_an_empty_key() {
    for (console_id, game_id) in [("", ""), ("n64", ""), ("", "n64/x"), ("n64", "n64/x")] {
        for candidate in scope::order(console_id, game_id) {
            assert_ne!(candidate, "console:", "an empty console level was emitted");
            assert_ne!(candidate, "game:", "an empty game level was emitted");
        }
    }
}

#[test]
fn the_universal_scope_is_last_and_present_exactly_once_in_every_combination() {
    // Universal is fallback; must exist exactly once per lookup.
    for (console_id, game_id) in [("", ""), ("n64", ""), ("", "a/b"), ("n64", "a/b")] {
        let scopes = scope::order(console_id, game_id);
        assert_eq!(scopes.last().map(String::as_str), Some(scope::UNIVERSAL));
        assert_eq!(scopes.iter().filter(|s| s.is_empty()).count(), 1);
    }
}

#[test]
fn every_candidate_scope_parses_back_to_the_level_it_came_from() {
    let scopes = scope::order("gamecube", "gamecube/melee");
    let parsed: Vec<Scope<'_>> = scopes.iter().map(|s| Scope::parse(s)).collect();
    assert_eq!(
        parsed,
        [
            Scope::Game("gamecube/melee"),
            Scope::Console("gamecube"),
            Scope::Universal
        ]
    );
}

#[test]
fn a_parsed_scope_prints_back_to_exactly_what_it_was_parsed_from() {
    for raw in [
        "",
        "console:n64",
        "game:n64/mario-64",
        "console:gamecube",
        "game:unknown/x",
    ] {
        assert_eq!(
            Scope::parse(raw).to_string(),
            raw,
            "{raw:?} did not round trip"
        );
    }
}

#[test]
fn a_scope_with_an_empty_payload_round_trips_and_stays_its_own_kind() {
    assert_eq!(Scope::parse("console:"), Scope::Console(""));
    assert_eq!(Scope::parse("game:"), Scope::Game(""));
    assert_eq!(Scope::parse("console:").to_string(), "console:");
    assert_eq!(Scope::parse("game:").to_string(), "game:");
}

#[test]
fn a_game_key_that_contains_the_console_prefix_stays_a_game_scope() {
    assert_eq!(Scope::parse("game:console:n64"), Scope::Game("console:n64"));
    assert_eq!(
        Scope::parse("game:console:n64").to_string(),
        "game:console:n64"
    );
    assert_eq!(scope::game_of("game:console:n64"), "console:n64");
    assert_eq!(scope::console_of("game:console:n64"), "");
}

#[test]
fn a_console_scope_round_trips_for_every_shipped_console() {
    for id in layout::consoles() {
        let raw = scope::console(id);
        assert_eq!(Scope::parse(&raw), Scope::Console(id));
        assert_eq!(Scope::parse(&raw).to_string(), raw);
    }
}

#[test]
fn only_the_last_suffix_is_stripped_from_a_rom_name() {
    // No-Intro: version numbers in stem (e.g., "Legend (v1.2).z64") must survive.
    assert_eq!(
        scope::game_key("n64", "Legend of Zelda, The (v1.2).z64"),
        "n64/legend-of-zelda-the-v1-2"
    );
    assert_eq!(scope::game_key("n64", "a.b.c.d.rom"), "n64/a-b-c-d");
    assert_eq!(scope::game_key("n64", "ROM.tar.gz"), "n64/rom-tar");
}

#[test]
fn a_name_with_no_dot_at_all_keeps_all_of_itself() {
    assert_eq!(
        scope::game_key("arcade", "/roms/mame/10yard"),
        "arcade/10yard"
    );
    assert_eq!(
        scope::game_key("ps2", "/roms/ps2/Final Fantasy X"),
        "ps2/final-fantasy-x"
    );
}

#[test]
fn a_trailing_slash_still_yields_the_directory_name() {
    // Trailing separator must be stripped; front-end may hand over "/path/to/folder/".
    assert_eq!(
        scope::game_key("ps2", "/roms/ps2/Final Fantasy X/"),
        "ps2/final-fantasy-x"
    );
    assert_eq!(scope::game_key("n64", "/trailing/slash/"), "n64/slash");
    assert_eq!(scope::game_key("n64", "/trailing/slash///"), "n64/slash");
    assert_eq!(scope::game_key("n64", "/"), "");
    assert_eq!(scope::game_key("n64", "///"), "");
}

#[test]
fn the_directory_a_rom_sits_in_is_ignored_entirely() {
    // Key-not-path: library move must not take per-game mappings with it.
    let keys = [
        scope::game_key("n64", "/mnt/old/roms/Mario 64.z64"),
        scope::game_key("n64", "/home/x/games/n64/Mario 64.z64"),
        scope::game_key("n64", "Mario 64.z64"),
        scope::game_key("n64", "./Mario 64.z64"),
        scope::game_key("n64", "/a//b///Mario 64.z64"),
    ];
    for key in &keys {
        assert_eq!(key, "n64/mario-64");
    }
}

#[test]
fn the_key_is_case_insensitive() {
    assert_eq!(
        scope::game_key("n64", "MARIO.z64"),
        scope::game_key("n64", "mario.z64")
    );
    assert_eq!(scope::game_key("n64", "MaRiO.Z64"), "n64/mario");
}

#[test]
fn runs_of_punctuation_collapse_to_a_single_dash() {
    assert_eq!(
        scope::game_key("n64", "Super   Smash___Bros"),
        "n64/super-smash-bros"
    );
    assert_eq!(scope::game_key("n64", "a!!!b"), "n64/a-b");
    assert_eq!(scope::game_key("n64", "tab\there"), "n64/tab-here");
    assert_eq!(scope::game_key("n64", "new\nline"), "n64/new-line");
}

#[test]
fn leading_and_trailing_punctuation_is_trimmed_rather_than_becoming_a_dash() {
    assert_eq!(
        scope::game_key("c", "  ...Hello___World!!!  .rom"),
        "c/hello-world"
    );
    assert_eq!(scope::game_key("c", "--x--.rom"), "c/x");
    assert_eq!(scope::game_key("c", "(U) [!]Sonic"), "c/u-sonic");
}

#[test]
fn a_stem_that_normalises_to_nothing_gets_no_key_rather_than_a_bare_console() {
    // "n64/" would share one scope; mapping one unnameable ROM would remap all.
    for rom in ["...z64", "!!!.rom", "", ".z64", "----.rom", "   .rom"] {
        assert_eq!(scope::game_key("n64", rom), "", "{rom:?} produced a key");
    }
}

#[test]
fn a_console_that_was_not_identified_still_gets_a_usable_key() {
    assert_eq!(scope::game_key("", "Sonic.bin"), "unknown/sonic");
    assert!(!scope::game_key("", "Sonic.bin").starts_with('/'));
}

#[test]
fn two_different_consoles_never_share_a_key_for_the_same_stem() {
    let mut seen = BTreeSet::new();
    for id in shipped_ids() {
        let key = scope::game_key(id, "Sonic.bin");
        assert!(seen.insert(key.clone()), "{id} reuses the key {key}");
    }
    assert!(seen.insert(scope::game_key("", "Sonic.bin")));
}

#[test]
fn a_non_ascii_name_keeps_only_its_ascii_and_may_vanish_entirely() {
    assert_eq!(scope::game_key("n64", "日本語.rom"), "");
    assert_eq!(scope::game_key("gb", "Pokémon Red.gb"), "gb/pok-mon-red");
    assert_eq!(scope::game_key("n64", "café.rom"), "n64/caf");
    assert_eq!(scope::game_key("n64", "ＡＢ.rom"), "");
}

#[test]
fn a_leading_dot_is_a_suffix_and_leaves_no_stem() {
    assert_eq!(scope::game_key("n64", ".hidden"), "");
    assert_eq!(scope::game_key("n64", "/roms/.hidden"), "");
    assert_eq!(scope::game_key("n64", ".a.b"), "n64/a");
}

#[test]
fn a_name_that_is_only_dots_has_no_key() {
    for rom in [".", "..", "...", "....", "/roms/..."] {
        assert_eq!(scope::game_key("n64", rom), "", "{rom:?} produced a key");
    }
}

#[test]
fn a_very_long_name_is_kept_whole_rather_than_truncated() {
    let stem = "a".repeat(300);
    assert_eq!(
        scope::game_key("n64", &format!("{stem}.z64")),
        format!("n64/{stem}")
    );
}

#[test]
fn a_dot_path_component_is_taken_as_the_name_rather_than_normalised_away() {
    assert_eq!(scope::game_key("n64", "roms/."), "");
    assert_eq!(scope::game_key("n64", "a/."), "");
    assert_eq!(scope::game_key("n64", "a/./Mario.z64"), "n64/mario");
}

#[test]
fn every_shipped_layout_has_both_a_body_and_something_to_press() {
    for layout in layout::all() {
        assert!(!layout.id.is_empty(), "a layout shipped with no id");
        assert!(!layout.label.is_empty(), "{} has no label", layout.id);
        assert!(
            !layout.shapes.is_empty(),
            "{} has no body to fall back to",
            layout.id
        );
        assert!(
            !layout.controls.is_empty(),
            "{} asks about nothing",
            layout.id
        );
    }
}

#[test]
fn every_control_in_every_layout_sits_on_the_canvas() {
    for layout in layout::all() {
        for control in &layout.controls {
            assert!(
                (0.0..=1.0).contains(&control.x),
                "{}'s {} has x = {}",
                layout.id,
                control.canonical,
                control.x
            );
            assert!(
                (0.0..=1.0).contains(&control.y),
                "{}'s {} has y = {}",
                layout.id,
                control.canonical,
                control.y
            );
        }
    }
}

#[test]
fn every_control_in_every_layout_is_drawn_at_a_visible_size() {
    for layout in layout::all() {
        for control in &layout.controls {
            assert!(
                control.radius > 0.0 && control.radius < 0.5,
                "{}'s {} has radius {}",
                layout.id,
                control.canonical,
                control.radius
            );
        }
    }
}

#[test]
fn every_control_in_every_layout_has_a_label_a_user_could_act_on() {
    for layout in layout::all() {
        for control in &layout.controls {
            assert!(
                !control.label.trim().is_empty(),
                "{}'s {} has no label",
                layout.id,
                control.canonical
            );
        }
    }
}

#[test]
fn every_control_kind_is_one_a_front_end_knows_how_to_draw() {
    for layout in layout::all() {
        for control in &layout.controls {
            assert!(
                matches!(
                    control.kind.as_str(),
                    "button" | "shoulder" | "dpad" | "stick"
                ),
                "{}'s {} has kind {:?}",
                layout.id,
                control.canonical,
                control.kind
            );
        }
    }
}

#[test]
fn no_two_controls_in_one_layout_are_drawn_at_the_same_point() {
    for layout in layout::all() {
        let mut seen = BTreeSet::new();
        for control in &layout.controls {
            let point = (control.x.to_bits(), control.y.to_bits());
            assert!(
                seen.insert(point),
                "{}'s {} shares a position with another control",
                layout.id,
                control.canonical
            );
        }
    }
}

#[test]
fn no_two_controls_in_one_layout_share_a_label() {
    for layout in layout::all() {
        let mut seen = BTreeSet::new();
        for control in &layout.controls {
            assert!(
                seen.insert(control.label.as_str()),
                "{} uses the label {:?} twice",
                layout.id,
                control.label
            );
        }
    }
}

#[test]
fn every_shape_carries_the_geometry_its_kind_needs() {
    for layout in layout::all() {
        for shape in &layout.shapes {
            match shape.kind.as_str() {
                "rect" => {
                    assert_eq!(shape.points.len(), 4, "{}: a rect is x,y,w,h", layout.id);
                }
                "circle" => {
                    assert_eq!(shape.points.len(), 2, "{}: a circle is x,y", layout.id);
                    assert!(shape.radius > 0.0, "{}: a circle with no radius", layout.id);
                }
                "polygon" => {
                    assert!(
                        shape.points.len() >= 6,
                        "{}: a polygon needs at least three points",
                        layout.id
                    );
                    assert_eq!(
                        shape.points.len() % 2,
                        0,
                        "{}: a polygon is pairs",
                        layout.id
                    );
                }
                other => panic!("{}: no front-end draws a {other:?}", layout.id),
            }
        }
    }
}

#[test]
fn every_shape_coordinate_is_a_finite_number_on_the_canvas() {
    for layout in layout::all() {
        for shape in &layout.shapes {
            for point in &shape.points {
                assert!(
                    point.is_finite(),
                    "{}: a {} has a non-finite point",
                    layout.id,
                    shape.kind
                );
                assert!(
                    (0.0..=1.0).contains(point),
                    "{}: a {} sits off the canvas at {point}",
                    layout.id,
                    shape.kind
                );
            }
            assert!(
                shape.radius.is_finite(),
                "{}: a {} has a bad radius",
                layout.id,
                shape.kind
            );
        }
    }
}

#[test]
fn no_layout_asks_about_the_same_canonical_control_twice() {
    for layout in layout::all() {
        let mut seen = BTreeSet::new();
        for control in layout.order() {
            assert!(
                seen.insert(control),
                "{} asks for {control} twice",
                layout.id
            );
        }
    }
}

#[test]
fn no_two_layouts_share_an_id() {
    let mut seen = BTreeSet::new();
    for id in shipped_ids() {
        assert!(seen.insert(id), "two layouts are called {id}");
    }
}

#[test]
fn every_canonical_name_in_every_layout_is_spellable_to_sdl() {
    for layout in layout::all() {
        for control in layout.order() {
            assert!(
                !control.sdl_field().is_empty(),
                "{} / {control} has no SDL field",
                layout.id
            );
        }
    }
}

#[test]
fn get_falls_back_to_the_default_for_an_id_that_names_nothing() {
    assert_eq!(layout::get("no-such-console").id, layout::default_id());
    assert_eq!(layout::get("dreamcast").id, layout::default_id());
}

#[test]
fn get_falls_back_for_an_empty_id_a_prefix_and_the_wrong_case() {
    // Matching is exact: no prefix matching, no case folding.
    for id in ["", "n6", "N64", "SNES", " n64", "n64 ", "generic/"] {
        assert_eq!(
            layout::get(id).id,
            layout::default_id(),
            "get({id:?}) guessed"
        );
    }
}

#[test]
fn get_returns_the_layout_asked_for_when_the_id_is_real() {
    for id in shipped_ids() {
        assert_eq!(layout::get(id).id, id);
    }
}

#[test]
fn exists_says_yes_to_exactly_the_shipped_layouts() {
    for id in shipped_ids() {
        assert!(layout::exists(id), "{id} is shipped but exists() denies it");
    }
    for id in ["", "n6", "N64", "dreamcast", "console:n64"] {
        assert!(
            !layout::exists(id),
            "{id:?} is not shipped but exists() allows it"
        );
    }
}

#[test]
fn index_of_is_zero_for_an_id_that_is_not_in_the_catalogue() {
    for id in ["", "no-such-console", "N64"] {
        assert_eq!(layout::index_of(id), 0, "index_of({id:?})");
    }
}

#[test]
fn index_of_agrees_with_the_catalogue_position_of_every_layout() {
    for (position, id) in shipped_ids().into_iter().enumerate() {
        assert_eq!(layout::index_of(id), position, "{id} is not at {position}");
    }
    assert_eq!(layout::index_of("generic"), 0);
    assert_eq!(layout::index_of("n64"), 2);
}

#[test]
fn the_generic_pad_is_a_layout_but_not_a_console() {
    assert!(layout::exists(layout::default_id()));
    assert!(!layout::consoles().contains(&layout::default_id()));
    assert_eq!(layout::consoles().len(), layout::all().len() - 1);
}

#[test]
fn consoles_preserves_catalogue_order() {
    assert_eq!(
        layout::consoles(),
        ["snes", "n64", "arcade", "gamecube", "ps2", "switch", "wiiu", "genesis"]
    );
}

#[test]
fn every_console_is_a_layout_that_can_be_fetched_back() {
    for id in layout::consoles() {
        assert!(layout::exists(id));
        assert_eq!(layout::get(id).id, id);
    }
}

#[test]
fn the_default_id_names_a_layout_that_exists_and_is_the_fallback() {
    assert_eq!(layout::default_id(), "generic");
    assert!(layout::exists(layout::default_id()));
    assert_eq!(layout::default_layout().id, layout::default_id());
    assert_eq!(layout::get("no-such-console"), layout::default_layout());
}

#[test]
fn for_icon_is_get_by_another_name() {
    for id in shipped_ids() {
        assert_eq!(layout::for_icon(id), layout::get(id));
    }
    assert_eq!(layout::for_icon("xbox").id, layout::default_id());
    assert_eq!(layout::for_icon("").id, layout::default_id());
}

#[test]
fn the_generic_control_set_is_every_control() {
    assert_eq!(
        layout::get("generic").order(),
        [
            Control::A,
            Control::B,
            Control::X,
            Control::Y,
            Control::DpadUp,
            Control::DpadDown,
            Control::DpadLeft,
            Control::DpadRight,
            Control::Back,
            Control::Start,
            Control::LeftShoulder,
            Control::RightShoulder,
            Control::LeftTrigger,
            Control::RightTrigger,
            Control::LeftStickUp,
            Control::LeftStickDown,
            Control::LeftStickLeft,
            Control::LeftStickRight,
            Control::RightStickUp,
            Control::RightStickDown,
            Control::RightStickLeft,
            Control::RightStickRight,
        ]
    );
}

#[test]
fn the_generic_pads_stick_halves_sit_on_its_stick_circles() {
    let generic = layout::get("generic");
    let circles: Vec<(f64, f64, f64)> = generic
        .shapes
        .iter()
        .filter(|shape| shape.kind == "circle")
        .map(|shape| (shape.points[0], shape.points[1], shape.radius))
        .collect();
    let sticks: Vec<_> = generic
        .controls
        .iter()
        .filter(|c| c.kind == "stick")
        .collect();
    assert_eq!(sticks.len(), 8);
    for (control, side) in sticks.iter().zip([
        "Left", "Left", "Left", "Left", "Right", "Right", "Right", "Right",
    ]) {
        assert!(
            control.label.starts_with(&format!("{side} stick ")),
            "{}",
            control.label
        );
        let (x, y, radius) = if side == "Left" {
            circles[0]
        } else {
            circles[1]
        };
        let off = ((control.x - x).powi(2) + (control.y - y).powi(2)).sqrt();
        assert!(
            off + control.radius <= radius,
            "{} is off its circle at ({}, {})",
            control.canonical,
            control.x,
            control.y
        );
    }
}

#[test]
fn a_snes_pad_has_no_analogue_trigger_and_no_right_stick() {
    // Asking for a shoulder the hardware does not have is a prompt the user.
    let order = layout::get("snes").order();
    assert_eq!(
        order,
        [
            Control::A,
            Control::B,
            Control::X,
            Control::Y,
            Control::DpadUp,
            Control::DpadDown,
            Control::DpadLeft,
            Control::DpadRight,
            Control::Back,
            Control::Start,
            Control::LeftShoulder,
            Control::RightShoulder,
        ]
    );
    assert!(!order.contains(&Control::LeftTrigger));
    assert!(!order.contains(&Control::RightTrigger));
    assert!(!order.contains(&Control::RightStickUp));
}

#[test]
fn an_n64_pad_has_no_x_or_y_and_four_c_buttons_as_right_stick_halves() {
    let order = layout::get("n64").order();
    assert_eq!(
        order,
        [
            Control::A,
            Control::B,
            Control::Start,
            Control::DpadUp,
            Control::DpadDown,
            Control::DpadLeft,
            Control::DpadRight,
            Control::LeftShoulder,
            Control::RightShoulder,
            Control::LeftTrigger,
            Control::RightStickUp,
            Control::RightStickDown,
            Control::RightStickLeft,
            Control::RightStickRight,
            Control::LeftStickUp,
            Control::LeftStickDown,
            Control::LeftStickLeft,
            Control::LeftStickRight,
        ]
    );
    assert!(!order.contains(&Control::X), "an N64 pad has no X");
    assert!(!order.contains(&Control::Y), "an N64 pad has no Y");
    assert!(!order.contains(&Control::Back), "an N64 pad has no Select");
    assert!(order.contains(&Control::LeftTrigger));
    assert!(!order.contains(&Control::RightTrigger));
}

#[test]
fn an_arcade_stick_is_six_buttons_a_stick_and_a_coin_slot() {
    assert_eq!(
        layout::get("arcade").order(),
        [
            Control::X,
            Control::Y,
            Control::LeftShoulder,
            Control::A,
            Control::B,
            Control::RightShoulder,
            Control::DpadUp,
            Control::DpadDown,
            Control::DpadLeft,
            Control::DpadRight,
            Control::Back,
            Control::Start,
        ]
    );
}

#[test]
fn a_gamecube_pad_has_a_c_stick_a_z_button_and_no_select() {
    // Z is filed as the right *trigger* and overridden to RetroPad R, because.
    let order = layout::get("gamecube").order();
    assert_eq!(
        order,
        [
            Control::A,
            Control::B,
            Control::X,
            Control::Y,
            Control::Start,
            Control::DpadUp,
            Control::DpadDown,
            Control::DpadLeft,
            Control::DpadRight,
            Control::LeftShoulder,
            Control::RightShoulder,
            Control::RightTrigger,
            Control::RightStickUp,
            Control::RightStickDown,
            Control::RightStickLeft,
            Control::RightStickRight,
            Control::LeftStickUp,
            Control::LeftStickDown,
            Control::LeftStickLeft,
            Control::LeftStickRight,
        ]
    );
    assert!(
        !order.contains(&Control::Back),
        "a GameCube pad has no Select"
    );
    assert_eq!(
        layout::get("gamecube")
            .controls
            .iter()
            .find(|c| c.canonical == Control::LeftStickUp)
            .map(|c| c.label.as_str()),
        Some("Control stick up"),
        "in the console's own words, not SDL's"
    );
    assert!(!order.contains(&Control::LeftTrigger));
}

#[test]
fn a_ps2_pad_is_the_plain_retropad_with_its_own_words() {
    let generic = layout::get("generic").order();
    let ps2 = layout::get("ps2").order();
    assert_eq!(
        ps2,
        generic[..ps2.len()],
        "the generic pad's buttons, before its sticks"
    );
    let labels: Vec<&str> = layout::get("ps2")
        .controls
        .iter()
        .map(|c| c.label.as_str())
        .collect();
    assert_eq!(labels[0], "Cross (bottom)");
    assert_eq!(labels[3], "Triangle (top)");
}

#[test]
fn a_switch_pro_pad_is_the_plain_retropad_with_its_own_words_and_two_sticks() {
    assert_eq!(
        layout::get("switch").order(),
        layout::get("generic").order(),
        "everything the generic pad asks for, sticks included, in the same order"
    );
    let labels: Vec<&str> = layout::get("switch")
        .controls
        .iter()
        .map(|c| c.label.as_str())
        .collect();
    assert_eq!(labels[0], "B (bottom)");
    assert_eq!(labels[1], "A (right)");
    assert_eq!(labels[8], "Minus");
    assert_eq!(labels[14], "Left stick up");
}

#[test]
fn a_genesis_pad_is_six_buttons_in_two_rows_with_mode_for_select() {
    assert_eq!(
        layout::get("genesis").order(),
        [
            Control::X,
            Control::A,
            Control::B,
            Control::LeftShoulder,
            Control::Y,
            Control::RightShoulder,
            Control::DpadUp,
            Control::DpadDown,
            Control::DpadLeft,
            Control::DpadRight,
            Control::Start,
            Control::Back,
        ]
    );
}

#[test]
fn the_right_stick_halves_are_a_c_cluster_on_two_pads_and_a_stick_on_three_more() {
    let with_right: Vec<&str> = shipped_ids()
        .into_iter()
        .filter(|id| layout::get(id).order().contains(&Control::RightStickUp))
        .collect();
    assert_eq!(with_right, ["generic", "n64", "gamecube", "switch", "wiiu"]);
    // Which of the two it is, is in the label and nowhere else.
    for (id, word) in [
        ("generic", "Right stick up"),
        ("n64", "C-up"),
        ("gamecube", "C-stick up"),
        ("switch", "Right stick up"),
        ("wiiu", "Right stick up"),
    ] {
        let label = layout::get(id)
            .controls
            .iter()
            .find(|c| c.canonical == Control::RightStickUp)
            .map(|c| c.label.as_str());
        assert_eq!(label, Some(word), "{id}");
    }
}

#[test]
fn every_pad_with_an_analog_stick_asks_about_all_four_of_its_halves() {
    let with_left: Vec<&str> = shipped_ids()
        .into_iter()
        .filter(|id| layout::get(id).order().contains(&Control::LeftStickUp))
        .collect();
    assert_eq!(with_left, ["generic", "n64", "gamecube", "switch", "wiiu"]);
    for id in &with_left {
        let order = layout::get(id).order();
        for half in [
            Control::LeftStickUp,
            Control::LeftStickDown,
            Control::LeftStickLeft,
            Control::LeftStickRight,
        ] {
            assert!(order.contains(&half), "{id} is missing {half}");
        }
        // An axis is not a button: the wizard has to know to ask for a push.
        for control in layout::get(id).controls.iter() {
            if control.canonical == Control::LeftStickUp {
                assert_eq!(control.kind, "stick", "{id}");
            }
        }
    }
    for id in ["snes", "genesis", "arcade"] {
        assert!(
            !layout::get(id).order().contains(&Control::LeftStickUp),
            "{id} has no analog stick"
        );
    }
}

#[test]
fn every_layout_is_a_strict_subset_of_the_canonical_vocabulary() {
    // A layout naming a control the enum does not have would not deserialise at.
    for layout in layout::all() {
        let order = layout.order();
        assert!(
            order.len() <= Control::ALL.len(),
            "{} asks for too much",
            layout.id
        );
        for control in order {
            assert!(
                Control::ALL.contains(&control),
                "{} asks for {control}",
                layout.id
            );
        }
    }
}

#[test]
fn every_shipped_layout_round_trips_through_json() {
    for layout in layout::all() {
        let json = serde_json::to_string(layout).expect("a shipped layout must serialise");
        let back: Layout = serde_json::from_str(&json).expect("and must parse back");
        assert_eq!(&back, layout, "{} did not survive a round trip", layout.id);
    }
}

#[test]
fn the_optional_layout_fields_default_to_what_keeps_the_data_files_short() {
    let json = r#"{
        "id": "minimal",
        "label": "Minimal",
        "controls": [{"canonical": "a", "label": "A", "x": 0.5, "y": 0.5}]
    }"#;
    let layout: Layout = serde_json::from_str(json).expect("a minimal layout must parse");
    assert_eq!(
        layout.console_label, "",
        "console_label falls back to label"
    );
    assert_eq!(layout.image, "", "no image means draw the shapes");
    assert!(layout.shapes.is_empty());
    let control = layout.controls.first().expect("one control");
    assert_eq!(
        control.kind, "button",
        "a control with no kind is a face button"
    );
    assert_eq!(control.radius, 0.045, "the default dot size");
}

#[test]
fn a_shape_without_a_radius_is_a_square_cornered_one() {
    let shape: Shape = serde_json::from_str(r#"{"kind": "rect", "points": [0.0, 0.0, 1.0, 1.0]}"#)
        .expect("a rect needs no radius");
    assert_eq!(shape.radius, 0.0);
    assert_eq!(shape.points.len(), 4);
}

#[test]
fn the_empty_optional_fields_are_left_out_of_the_serialised_form() {
    let json = serde_json::to_string(layout::get("generic")).expect("serialise");
    assert!(
        !json.contains("console_label"),
        "an empty console_label was written"
    );
    assert!(!json.contains("\"image\""), "an empty image was written");
    // The ones that are always present, because the front-end indexes them.
    assert!(json.contains("\"shapes\""));
    assert!(json.contains("\"controls\""));
}

#[test]
fn a_console_label_does_survive_serialisation() {
    // The skip is on emptiness, not on the field, so the layouts that do carry.
    let json = serde_json::to_string(layout::get("arcade")).expect("serialise");
    assert!(json.contains("\"console_label\":\"Arcade\""), "{json}");
}

#[test]
fn a_control_round_trips_through_json_on_its_own() {
    let control = LayoutControl {
        canonical: Control::RightStickUp,
        label: "C-up".to_owned(),
        x: 0.745,
        y: 0.29,
        kind: "stick".to_owned(),
        radius: 0.032,
    };
    let json = serde_json::to_string(&control).expect("serialise");
    let back: LayoutControl = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(back, control);
}

#[test]
fn a_layout_naming_a_control_the_vocabulary_does_not_have_is_rejected() {
    // The one thing a data file must not be allowed to do quietly: a typo'd.
    let json = r#"{
        "id": "typo",
        "label": "Typo",
        "controls": [{"canonical": "guide", "label": "Guide", "x": 0.5, "y": 0.5}]
    }"#;
    assert!(
        serde_json::from_str::<Layout>(json).is_err(),
        "\"guide\" was accepted"
    );
}
