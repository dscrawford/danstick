//! What a mapping is *for*, and what the console it is for actually has on it.
//!
//! Two modules that only look unrelated: a console scope names a *layout* id,
//! because the console is the thing that decides which controls exist and which
//! RetroArch key each is emitted under.
//!
//! The unit tests in `src/` cover the happy path; these cover the edges that
//! reach a user. Mostly the shipped layout data, which is now JSON rather than
//! code literals -- nothing type-checks it, so this file is what stands between
//! a malformed layout and somebody holding a controller.

use std::collections::{BTreeMap, BTreeSet};

use padmap_core::control::Control;
use padmap_core::layout::{self, Layout, LayoutControl, Shape};
use padmap_core::scope::{self, Scope};

/// Every id the crate ships, so a console added tomorrow joins the sweeping
/// tests by existing rather than by someone remembering to list it.
fn shipped_ids() -> Vec<&'static str> {
    layout::all().iter().map(|l| l.id.as_str()).collect()
}

/// `retroarch_keys()` flattened, so an expectation can be written as a literal.
fn overrides_of(layout_id: &str) -> Vec<(Control, String)> {
    layout::get(layout_id)
        .retroarch_keys()
        .into_iter()
        .collect()
}

/// The same shape as [`overrides_of`], built from a literal table.
fn expected_overrides(pairs: &[(Control, &str)]) -> Vec<(Control, String)> {
    pairs
        .iter()
        .map(|(c, key)| (*c, (*key).to_owned()))
        .collect()
}

/// The RetroArch key a control will actually be written under on one console:
/// the layout's override if it has one, otherwise the canonical default.
fn effective_key(overrides: &BTreeMap<Control, String>, control: Control) -> String {
    overrides
        .get(&control)
        .cloned()
        .unwrap_or_else(|| control.retroarch_key().to_owned())
}

// -- scope: construction and extraction ---------------------------------------

#[test]
fn all_three_kinds_of_scope_survive_construction_and_extraction() {
    // Storage and lookup both run on the flat string, so a scope that cannot be
    // taken apart again is a mapping that can be filed and never found.
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
    // If it were, the GameCube pad's console mapping would answer a lookup for
    // a per-game one and quietly win over the mapping the user actually made.
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
    // These strings come off disk. A profile someone hand-edited must still
    // resolve to *something* -- refusing to load it loses every mapping in the
    // file, not just the malformed line.
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
    // `console("")` is not a thing resolution should produce, but if it ever
    // does it must stay syntactically a console scope rather than collapsing
    // into the universal one, which would silently overwrite the default.
    assert_eq!(scope::console(""), "console:");
    assert_eq!(scope::game(""), "game:");
    assert_ne!(scope::console(""), scope::UNIVERSAL);
    assert_ne!(scope::game(""), scope::UNIVERSAL);
}

// -- scope: precedence --------------------------------------------------------

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
    // `"console:"` in the candidate list is a key a mapping could be filed
    // under by accident, and it would then apply to every console at once.
    for (console_id, game_id) in [("", ""), ("n64", ""), ("", "n64/x"), ("n64", "n64/x")] {
        for candidate in scope::order(console_id, game_id) {
            assert_ne!(candidate, "console:", "an empty console level was emitted");
            assert_ne!(candidate, "game:", "an empty game level was emitted");
        }
    }
}

#[test]
fn the_universal_scope_is_last_and_present_exactly_once_in_every_combination() {
    // It is the fallback every other consumer relies on existing. Twice would
    // be harmless; zero times means a pad with only a default mapping resolves
    // to nothing and reports as unconfigured.
    for (console_id, game_id) in [("", ""), ("n64", ""), ("", "a/b"), ("n64", "a/b")] {
        let scopes = scope::order(console_id, game_id);
        assert_eq!(scopes.last().map(String::as_str), Some(scope::UNIVERSAL));
        assert_eq!(scopes.iter().filter(|s| s.is_empty()).count(), 1);
    }
}

#[test]
fn every_candidate_scope_parses_back_to_the_level_it_came_from() {
    // The list is walked against a stored map, so a candidate that does not
    // mean what it looks like means the wrong mapping is applied silently.
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

// -- scope: parse and display -------------------------------------------------

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
    // `Scope::parse` keeps the distinction the bare extractors cannot: a
    // `console_of("console:")` and a `console_of("nonsense")` are both `""`,
    // but only one of them was a console scope.
    assert_eq!(Scope::parse("console:"), Scope::Console(""));
    assert_eq!(Scope::parse("game:"), Scope::Game(""));
    assert_eq!(Scope::parse("console:").to_string(), "console:");
    assert_eq!(Scope::parse("game:").to_string(), "game:");
}

#[test]
fn a_game_key_that_contains_the_console_prefix_stays_a_game_scope() {
    // The game prefix is checked first, so a key that happens to spell the
    // other prefix does not change which level the scope belongs to. A ROM can
    // legitimately be named anything.
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

// -- scope: game_key ----------------------------------------------------------

#[test]
fn only_the_last_suffix_is_stripped_from_a_rom_name() {
    // "Legend of Zelda, The (v1.2).z64" must not lose everything after the
    // first dot -- version numbers in the stem are how No-Intro names things.
    assert_eq!(
        scope::game_key("n64", "Legend of Zelda, The (v1.2).z64"),
        "n64/legend-of-zelda-the-v1-2"
    );
    assert_eq!(scope::game_key("n64", "a.b.c.d.rom"), "n64/a-b-c-d");
    assert_eq!(scope::game_key("n64", "ROM.tar.gz"), "n64/rom-tar");
}

#[test]
fn a_name_with_no_dot_at_all_keeps_all_of_itself() {
    // A directory-shaped "ROM" is normal: a MAME set, a PlayStation disc
    // folder. Treating the whole name as a suffix would key every one to "".
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
    // A real divergence found during the port: `Path(...).name` strips trailing
    // separators, a bare `rsplit('/')` does not, and a front-end that hands over
    // a folder with a slash on the end would have got no key and a per-game
    // mapping that silently never applied.
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
    // The whole reason the key is not the path: a library that moves must not
    // take every per-game mapping with it, because nothing would report that.
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
    // The same ROM re-dumped with different capitalisation is the same game to
    // the person holding the controller.
    assert_eq!(
        scope::game_key("n64", "MARIO.z64"),
        scope::game_key("n64", "mario.z64")
    );
    assert_eq!(scope::game_key("n64", "MaRiO.Z64"), "n64/mario");
}

#[test]
fn runs_of_punctuation_collapse_to_a_single_dash() {
    // One dash per run, not one per character: otherwise the key depends on how
    // many spaces the dumper left between the title and the region tag.
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
    // A key ending in a dash sorts and reads badly in the profile on disk, and
    // "-x" and "x" would be two scopes for one game.
    assert_eq!(
        scope::game_key("c", "  ...Hello___World!!!  .rom"),
        "c/hello-world"
    );
    assert_eq!(scope::game_key("c", "--x--.rom"), "c/x");
    assert_eq!(scope::game_key("c", "(U) [!]Sonic"), "c/u-sonic");
}

#[test]
fn a_stem_that_normalises_to_nothing_gets_no_key_rather_than_a_bare_console() {
    // "n64/" would be one scope shared by every unnameable ROM on the console,
    // so mapping one of them would silently remap all of them.
    for rom in ["...z64", "!!!.rom", "", ".z64", "----.rom", "   .rom"] {
        assert_eq!(scope::game_key("n64", rom), "", "{rom:?} produced a key");
    }
}

#[test]
fn a_console_that_was_not_identified_still_gets_a_usable_key() {
    // The console prefix is what stops `sonic` on an arcade board sharing a
    // mapping with `sonic` on a Genesis, so it must never be empty.
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
    // And the unidentified case is its own bucket too, not a collision with a
    // console that happens to be called "unknown".
    assert!(seen.insert(scope::game_key("", "Sonic.bin")));
}

#[test]
fn a_non_ascii_name_keeps_only_its_ascii_and_may_vanish_entirely() {
    // Pinned rather than improved: the Python slugs with `[^a-z0-9]+`, so
    // accented and CJK characters are punctuation as far as the key is
    // concerned. A transliterating port would give every European-titled ROM a
    // different key from the one already in users' profiles.
    assert_eq!(scope::game_key("n64", "日本語.rom"), "");
    assert_eq!(scope::game_key("gb", "Pokémon Red.gb"), "gb/pok-mon-red");
    assert_eq!(scope::game_key("n64", "café.rom"), "n64/caf");
    assert_eq!(scope::game_key("n64", "ＡＢ.rom"), "");
}

#[test]
fn a_leading_dot_is_a_suffix_and_leaves_no_stem() {
    // ".hidden" has its only dot at position zero, so the stem is empty. A
    // dotfile is not a ROM, and giving it a key would file a mapping under one.
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
    // Truncating would make two long titles that share a prefix into one scope,
    // and the key only ever goes into a JSON value, so length costs nothing.
    let stem = "a".repeat(300);
    assert_eq!(
        scope::game_key("n64", &format!("{stem}.z64")),
        format!("n64/{stem}")
    );
}

#[test]
fn a_dot_path_component_is_taken_as_the_name_rather_than_normalised_away() {
    // DIVERGENCE, pinned deliberately. Python's `Path("roms/.").name` is
    // "roms" because `PurePath` drops "." components on construction; this
    // splits on "/" and sees ".", whose stem is empty. Nothing hands padmap a
    // path ending in "/." -- a launcher passes the ROM it is about to run --
    // and matching CPython's path normalisation here would cost more than the
    // case is worth. Recorded so the choice is visible rather than accidental.
    assert_eq!(scope::game_key("n64", "roms/."), "");
    assert_eq!(scope::game_key("n64", "a/."), "");
    // The ordinary form of the same thing does agree, which is what matters.
    assert_eq!(scope::game_key("n64", "a/./Mario.z64"), "n64/mario");
}

// -- layout: the shipped data is drawable -------------------------------------

#[test]
fn every_shipped_layout_has_both_a_body_and_something_to_press() {
    // `shapes` is the fallback when an image is missing or fails to load; a
    // layout with none draws an empty box with dots floating in it.
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
    // Coordinates are normalised 0..1. One outside that range is an arrow
    // pointing off the edge of the picture, at nothing.
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
    // Radius is a fraction of canvas height. Zero is an invisible target the
    // user is nonetheless being asked to find; half the canvas is not a dot.
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
    // The label is the whole prompt: "press Z (underneath)". An empty one asks
    // the user to press nothing in particular and then waits.
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
    // `kind` picks the glyph. An unknown one is not a compile error anywhere --
    // the front-end just draws nothing where a button should be.
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
    // Two controls under one arrow makes the wizard's two prompts look
    // identical, and the user has no way to tell which one is being asked for.
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
    // The label is what the user is shown. Two prompts reading "L" in a row is
    // indistinguishable from the wizard having failed to advance.
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
    // The front-end reads these positionally. A rect with three numbers is an
    // index panic or a body drawn at a garbage size, depending on the renderer.
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
    // JSON permits any float. A NaN from a hand-edited data file propagates
    // into the renderer's transform and takes the whole picture with it.
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

// -- layout: the catalogue is coherent ----------------------------------------

#[test]
fn no_layout_asks_about_the_same_canonical_control_twice() {
    // The wizard's result is keyed by canonical control, so the second prompt
    // silently overwrites the first and one of the two presses is thrown away.
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
    // The id is what a console scope names and what `get` matches on. A
    // duplicate means one of the two layouts is unreachable forever.
    let mut seen = BTreeSet::new();
    for id in shipped_ids() {
        assert!(seen.insert(id), "two layouts are called {id}");
    }
}

#[test]
fn every_canonical_name_in_every_layout_is_spellable_by_both_consumers() {
    // A control the wizard asks for but neither SDL nor RetroArch can be told
    // about is a press collected and then dropped, with nothing reporting it.
    for layout in layout::all() {
        for control in layout.order() {
            assert!(
                !control.sdl_field().is_empty(),
                "{} / {control} has no SDL field",
                layout.id
            );
            assert!(
                !control.retroarch_key().is_empty(),
                "{} / {control} has no RetroArch key",
                layout.id
            );
        }
    }
}

#[test]
fn no_two_controls_on_one_console_end_up_under_the_same_retroarch_key() {
    // Overrides are per-console and hand-written, so a copy-paste slip here
    // puts two buttons on one key: the second wins in the autoconfig and the
    // pad still reports as fully configured.
    for layout in layout::all() {
        let overrides = layout.retroarch_keys();
        let mut seen = BTreeSet::new();
        for control in layout.order() {
            let key = effective_key(&overrides, control);
            assert!(
                seen.insert(key.clone()),
                "{}: {control} collides on {key}",
                layout.id
            );
        }
    }
}

#[test]
fn every_override_names_a_control_the_layout_actually_asks_about() {
    // An override on a control the layout does not carry is dead data that
    // reads as a deliberate decision when someone next edits the file.
    for layout in layout::all() {
        let asked: BTreeSet<Control> = layout.order().into_iter().collect();
        for control in layout.retroarch_keys().keys() {
            assert!(
                asked.contains(control),
                "{} overrides absent {control}",
                layout.id
            );
        }
    }
}

#[test]
fn every_override_is_spelled_like_a_retroarch_autoconfig_key() {
    // These go straight into the autoconfig file. A key RetroArch does not
    // recognise is ignored on load, which looks exactly like an unbound button.
    for layout in layout::all() {
        for (control, key) in layout.retroarch_keys() {
            assert!(
                key.starts_with("input_") && key.ends_with("_btn"),
                "{}: {control} overrides to {key:?}",
                layout.id
            );
        }
    }
}

// -- layout: lookup never fails -----------------------------------------------

#[test]
fn get_falls_back_to_the_default_for_an_id_that_names_nothing() {
    // An unknown id comes from stored state or a front-end. Refusing to show a
    // wizard at all is a worse answer than showing the ordinary pad.
    assert_eq!(layout::get("no-such-console").id, layout::default_id());
    assert_eq!(layout::get("dreamcast").id, layout::default_id());
}

#[test]
fn get_falls_back_for_an_empty_id_a_prefix_and_the_wrong_case() {
    // Matching is exact: no prefix matching, no case folding. "n6" must not
    // silently become the N64 pad, and "N64" must not either -- a lookup that
    // guesses is how a user ends up mapping a console they did not pick.
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
    // Zero, not an error: the index drives a picker's selected row, and there
    // is always a first row to fall back to.
    for id in ["", "no-such-console", "N64"] {
        assert_eq!(layout::index_of(id), 0, "index_of({id:?})");
    }
}

#[test]
fn index_of_agrees_with_the_catalogue_position_of_every_layout() {
    for (position, id) in shipped_ids().into_iter().enumerate() {
        assert_eq!(layout::index_of(id), position, "{id} is not at {position}");
    }
    // Pinned so a reorder of `data/layouts.json` has to be a deliberate edit:
    // the index is what a stored picker selection means.
    assert_eq!(layout::index_of("generic"), 0);
    assert_eq!(layout::index_of("n64"), 2);
}

#[test]
fn the_generic_pad_is_a_layout_but_not_a_console() {
    // "My pad, when playing generic games" is not a thing anyone can mean, and
    // offering it as a scope produces one no core will ever report.
    assert!(layout::exists(layout::default_id()));
    assert!(!layout::consoles().contains(&layout::default_id()));
    assert_eq!(layout::consoles().len(), layout::all().len() - 1);
}

#[test]
fn consoles_preserves_catalogue_order() {
    // The picker renders this list directly, and the order is the one a reader
    // of `data/layouts.json` chose.
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
    // The icon set and the layout set overlap by design: someone who has
    // already said "this is an N64 controller" must not be asked again.
    for id in shipped_ids() {
        assert_eq!(layout::for_icon(id), layout::get(id));
    }
    assert_eq!(layout::for_icon("xbox").id, layout::default_id());
    assert_eq!(layout::for_icon("").id, layout::default_id());
}

// -- layout: cores ------------------------------------------------------------

#[test]
fn a_core_resolves_to_its_console_through_every_spelling() {
    // padmap-play is handed `-L <core>` and that string is the only thing at
    // launch that says which console is about to run.
    for core in [
        "mupen64plus_next",
        "mupen64plus_next_libretro",
        "mupen64plus_next_libretro.so",
        "mupen64plus_next-libretro.so",
        "mupen64plus_next_libretro.dll",
        "mupen64plus_next_libretro.dylib",
        "/nix/store/abc-cores/mupen64plus_next_libretro.so",
        "/usr/lib/libretro/mupen64plus_next_libretro.so",
    ] {
        assert_eq!(layout::for_core(core), "n64", "{core} did not resolve");
    }
}

#[test]
fn the_core_name_is_matched_case_insensitively() {
    for core in [
        "Mupen64plus_Next_libretro.so",
        "SNES9X",
        "MAME2003_Plus",
        "PCSX2",
    ] {
        assert!(!layout::for_core(core).is_empty(), "{core} did not resolve");
    }
    assert_eq!(layout::for_core("SNES9X"), "snes");
    assert_eq!(layout::for_core("PCSX2"), "ps2");
}

#[test]
fn the_library_suffix_is_matched_case_sensitively_as_the_python_does() {
    // Pinning the quirk rather than improving on it: the suffix is stripped
    // before the name is lowercased, so an uppercase ".SO" survives into the
    // lookup and misses. Linux core filenames are lowercase, so this has never
    // bitten -- and fixing it only here would make the two implementations
    // disagree, which is worse than the quirk.
    assert_eq!(layout::for_core("MUPEN64PLUS_NEXT_LIBRETRO.SO"), "");
    assert_eq!(layout::for_core("mupen64plus_next_libretro.SO"), "");
    assert_eq!(layout::for_core("mupen64plus_next_libretro.Dll"), "");
}

#[test]
fn an_unknown_core_gives_no_console_rather_than_the_generic_one() {
    // "" means "skip the console scope". `generic` would mean "look for a
    // mapping filed under the generic pad". Two different answers, and only one
    // of them leaves the user's universal mapping in charge.
    for core in [
        "",
        "some_core_libretro.so",
        "vice_x64",
        "/opt/cores/nestopia_libretro.so",
    ] {
        assert_eq!(layout::for_core(core), "", "{core:?} resolved to something");
        assert_ne!(layout::for_core(core), layout::default_id());
    }
}

#[test]
fn a_core_that_is_nothing_but_a_suffix_resolves_to_nothing() {
    // Strip ".so" then "_libretro" from "_libretro.so" and the name is empty.
    // An empty name must miss the table rather than match some empty key.
    for core in [
        "_libretro.so",
        "-libretro.so",
        ".so",
        ".dll",
        ".dylib",
        "_libretro",
        "/",
    ] {
        assert_eq!(layout::for_core(core), "", "{core:?} resolved to something");
    }
}

#[test]
fn only_one_library_suffix_and_one_marker_are_stripped() {
    // A versioned filename keeps its ".1", so the name never matches. Pinned
    // because it is what the Python does and because guessing at repeated
    // suffixes would let ".so.so" resolve, which is not a real core either.
    assert_eq!(layout::for_core("mupen64plus_next_libretro.so.1"), "");
    assert_eq!(layout::for_core("mupen64plus_next_libretro_libretro"), "");
}

#[test]
fn a_core_path_with_a_trailing_slash_keeps_its_core_name() {
    // This test found the divergence in its first form: `scope::game_key` trims
    // a trailing separator and `for_core` did not, so the two read a path
    // differently and could disagree about which console a launch was.
    assert_eq!(layout::for_core("mame/"), "arcade");
    assert_eq!(layout::for_core("/usr/lib/libretro/mame/"), "arcade");
    assert_eq!(layout::for_core("/"), "");
    assert_eq!(layout::for_core("///"), "");
}

#[test]
fn every_core_in_the_manifest_resolves_to_a_layout_that_exists() {
    // A typo in `data/layouts.json` resolves a mapping to a console nothing can
    // render, and the only symptom is a wizard showing the wrong pad.
    for core in [
        "mupen64plus_next",
        "mupen64plus",
        "parallel_n64",
        "snes9x",
        "snes9x2010",
        "snes9x2005",
        "snes9x2002",
        "bsnes",
        "bsnes_mercury_accuracy",
        "bsnes_mercury_balanced",
        "bsnes_mercury_performance",
        "mesen_s",
        "mame2010",
        "mame2003",
        "mame2003_plus",
        "mame2000",
        "mame",
        "fbalpha",
        "fbalpha2012",
        "fbneo",
        "dolphin",
        "pcsx2",
        "play",
        "genesis_plus_gx",
        "picodrive",
        "blastem",
    ] {
        let resolved = layout::for_core(core);
        assert!(
            layout::exists(resolved),
            "core {core} resolved to missing layout {resolved:?}"
        );
        assert!(
            layout::consoles().contains(&resolved),
            "core {core} resolved to {resolved}, which is not a console"
        );
    }
}

#[test]
fn a_resolved_core_can_be_turned_straight_into_a_console_scope() {
    // This is the whole launch path: core name in, scope out, mapping looked up.
    let console = layout::for_core("dolphin_libretro.so");
    assert_eq!(console, "gamecube");
    assert_eq!(scope::order(console, ""), ["console:gamecube", ""]);
    assert_eq!(
        Scope::parse(&scope::console(console)),
        Scope::Console("gamecube")
    );
}

// -- layout: the per-console override tables ----------------------------------

#[test]
fn the_n64_override_map_is_exactly_b_to_the_retropad_y_key() {
    // mupen64plus-next reads N64 B from RetroPad **Y**, not RetroPad A. Without
    // this the physical B is bound to a key that does nothing at all, and the
    // pad still reports as configured.
    assert_eq!(
        overrides_of("n64"),
        expected_overrides(&[(Control::B, "input_y_btn")])
    );
}

#[test]
fn the_snes_layout_carries_no_overrides_at_all() {
    // Read against snes9x's own source (libretro.cpp, the MAP_BUTTON table).
    // An override appearing here later is a claim that wants the same reading.
    assert!(overrides_of("snes").is_empty());
}

#[test]
fn the_ps2_layout_carries_no_overrides_at_all() {
    // Read against pcsx2's PAD.cpp, the PAD_* -> RETRO_DEVICE_ID_JOYPAD_* table.
    assert!(overrides_of("ps2").is_empty());
}

#[test]
fn the_arcade_override_map_is_exactly_the_five_the_mame_reading_produced() {
    // src/osd/retro/retromain.c, the P1_state block: the arcade six-button
    // cluster does not line up with the RetroPad face buttons in any order that
    // the canonical names would produce on their own.
    assert_eq!(
        overrides_of("arcade"),
        expected_overrides(&[
            (Control::A, "input_y_btn"),
            (Control::B, "input_l_btn"),
            (Control::X, "input_a_btn"),
            (Control::Y, "input_b_btn"),
            (Control::LeftShoulder, "input_x_btn"),
        ])
    );
}

#[test]
fn the_gamecube_override_map_is_exactly_the_dolphin_reading() {
    // Source/Core/DolphinLibretro/Input.cpp: GameCube's face buttons map to the
    // identically-lettered RetroPad buttons, which is *not* what the canonical
    // names give -- the canonical table crosses a/b and x/y for Nintendo.
    assert_eq!(
        overrides_of("gamecube"),
        expected_overrides(&[
            (Control::A, "input_a_btn"),
            (Control::B, "input_b_btn"),
            (Control::X, "input_x_btn"),
            (Control::Y, "input_y_btn"),
            (Control::LeftShoulder, "input_l2_btn"),
            (Control::RightShoulder, "input_r2_btn"),
            (Control::RightTrigger, "input_r_btn"),
        ])
    );
}

#[test]
fn the_generic_switch_and_genesis_layouts_carry_no_overrides() {
    // Genesis was read against genesis_plus_gx's DEVICE_PAD6B case, which
    // showed the top row comes from RetroPad L/R -- which is what the canonical
    // names already say. The other two describe the RetroPad itself.
    for id in ["generic", "switch", "genesis"] {
        assert!(overrides_of(id).is_empty(), "{id} grew an override");
    }
}

#[test]
fn exactly_three_of_the_eight_layouts_override_anything() {
    // A regression anchor over the set as a whole: adding an override to a
    // fourth console should move this test as well as that console's own.
    let overriding: Vec<&str> = shipped_ids()
        .into_iter()
        .filter(|id| !layout::get(id).retroarch_keys().is_empty())
        .collect();
    assert_eq!(overriding, ["n64", "arcade", "gamecube"]);
}

// -- layout: the per-console control sets -------------------------------------

#[test]
fn the_generic_control_set_is_the_plain_retropad() {
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
        ]
    );
}

#[test]
fn a_snes_pad_has_no_analogue_trigger_and_no_right_stick() {
    // Asking for a shoulder the hardware does not have is a prompt the user
    // cannot satisfy, and the wizard has no way to know they are stuck.
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
    // SDL has no concept of "C-up", so a C-button is a half-axis push. Inventing
    // face buttons for them instead would bind four presses to controls the core
    // never reads.
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
        ]
    );
    assert!(!order.contains(&Control::X), "an N64 pad has no X");
    assert!(!order.contains(&Control::Y), "an N64 pad has no Y");
    assert!(!order.contains(&Control::Back), "an N64 pad has no Select");
    // Z is the only trigger, and it is the left one.
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
    // Z is filed as the right *trigger* and overridden to RetroPad R, because
    // the physical L and R are the analogue ones.
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
        ]
    );
    assert!(
        !order.contains(&Control::Back),
        "a GameCube pad has no Select"
    );
    assert!(!order.contains(&Control::LeftTrigger));
}

#[test]
fn a_ps2_pad_is_the_plain_retropad_with_its_own_words() {
    // Same controls as generic, different labels -- which is the point of
    // keeping `label` separate from `canonical`.
    assert_eq!(layout::get("ps2").order(), layout::get("generic").order());
    let labels: Vec<&str> = layout::get("ps2")
        .controls
        .iter()
        .map(|c| c.label.as_str())
        .collect();
    assert_eq!(labels[0], "Cross (bottom)");
    assert_eq!(labels[3], "Triangle (top)");
}

#[test]
fn a_switch_pro_pad_is_the_plain_retropad_with_its_own_words() {
    assert_eq!(
        layout::get("switch").order(),
        layout::get("generic").order()
    );
    let labels: Vec<&str> = layout::get("switch")
        .controls
        .iter()
        .map(|c| c.label.as_str())
        .collect();
    // Nintendo's A and B are swapped relative to the canonical positions, which
    // is exactly the confusion `label` exists to remove.
    assert_eq!(labels[0], "B (bottom)");
    assert_eq!(labels[1], "A (right)");
    assert_eq!(labels[8], "Minus");
}

#[test]
fn a_genesis_pad_is_six_buttons_in_two_rows_with_mode_for_select() {
    // The top row reaches the core through RetroPad L/R rather than through
    // face buttons, which is why X and Z are canonical shoulders here.
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
fn only_the_two_layouts_with_a_c_cluster_ask_about_the_right_stick() {
    // A regression anchor on the set: the stick halves exist for C-buttons, and
    // a console growing them is a claim about how its core reads the pad.
    let with_stick: Vec<&str> = shipped_ids()
        .into_iter()
        .filter(|id| layout::get(id).order().contains(&Control::RightStickUp))
        .collect();
    assert_eq!(with_stick, ["n64", "gamecube"]);
}

#[test]
fn every_layout_is_a_strict_subset_of_the_canonical_vocabulary() {
    // A layout naming a control the enum does not have would not deserialise at
    // all; this pins the other direction, that no layout is asking about
    // eighteen things when the pad has twelve.
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

// -- layout: serde ------------------------------------------------------------

#[test]
fn every_shipped_layout_round_trips_through_json() {
    // The catalogue is serialised to the front-end. A field lost in the round
    // trip is a control the picker draws differently from the wizard.
    for layout in layout::all() {
        let json = serde_json::to_string(layout).expect("a shipped layout must serialise");
        let back: Layout = serde_json::from_str(&json).expect("and must parse back");
        assert_eq!(&back, layout, "{} did not survive a round trip", layout.id);
    }
}

#[test]
fn the_optional_layout_fields_default_to_what_keeps_the_data_files_short() {
    // These defaults are the reason a layout file is readable. A wrong one is a
    // control drawn invisibly, at the wrong size, or with the wrong glyph -- and
    // none of those fail to parse.
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
    assert_eq!(
        control.retroarch, "",
        "no override means use the canonical key"
    );
}

#[test]
fn a_shape_without_a_radius_is_a_square_cornered_one() {
    // Radius on a rect is the corner rounding, and zero is a legitimate answer,
    // so this default is 0.0 rather than the control dot's 0.045.
    let shape: Shape = serde_json::from_str(r#"{"kind": "rect", "points": [0.0, 0.0, 1.0, 1.0]}"#)
        .expect("a rect needs no radius");
    assert_eq!(shape.radius, 0.0);
    assert_eq!(shape.points.len(), 4);
}

#[test]
fn the_empty_optional_fields_are_left_out_of_the_serialised_form() {
    // What keeps the payload sent to the front-end from being mostly `""`.
    let json = serde_json::to_string(layout::get("generic")).expect("serialise");
    assert!(
        !json.contains("console_label"),
        "an empty console_label was written"
    );
    assert!(!json.contains("\"image\""), "an empty image was written");
    assert!(!json.contains("retroarch"), "empty overrides were written");
    // The ones that are always present, because the front-end indexes them.
    assert!(json.contains("\"shapes\""));
    assert!(json.contains("\"controls\""));
}

#[test]
fn a_console_label_and_an_override_do_survive_serialisation() {
    // The skip is on emptiness, not on the field, so the layouts that do carry
    // these must still send them.
    let json = serde_json::to_string(layout::get("arcade")).expect("serialise");
    assert!(json.contains("\"console_label\":\"Arcade\""), "{json}");
    assert!(json.contains("\"retroarch\":\"input_a_btn\""), "{json}");
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
        retroarch: String::new(),
    };
    let json = serde_json::to_string(&control).expect("serialise");
    let back: LayoutControl = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(back, control);
}

#[test]
fn a_layout_naming_a_control_the_vocabulary_does_not_have_is_rejected() {
    // The one thing a data file must not be allowed to do quietly: a typo'd
    // canonical name has to fail at parse rather than produce a layout with a
    // control nothing downstream can spell.
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
