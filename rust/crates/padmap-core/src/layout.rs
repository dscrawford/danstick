//! Controller layouts: what to draw, where to point, and what it means.
//!
//! The mapping wizard shows a picture of the controller with an arrow at the
//! control being mapped. Rather than shipping artwork per controller and a
//! separate table of where each button sits, the positions *are* the artwork:
//! the front-end draws the body from `shapes` and the controls from their own
//! coordinates, so a new layout is a data entry rather than an asset hunt.
//!
//! Three things a layout carries, and the third is what makes this more than a
//! drawing: where each control is, what to call it in the hardware's own words,
//! and which canonical control it *is* -- because that is what RetroArch and
//! SDL are told. An N64 pad has no X or Y and four C-buttons that behave as a
//! right stick; a SNES pad has no analogue anything. Layouts differ in which
//! controls exist, not merely where they sit.
//!
//! The layouts themselves live in `data/layouts/*.json`, embedded at compile
//! time. In the Python they were Rust-equivalent literals in the middle of the
//! module, which made adding a console a code change reviewed as code. Here it
//! is a file, `data/layouts.json` names the order, and
//! [`tests::every_shipped_layout_is_well_formed`] is what stops a malformed one
//! reaching a user.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::control::Control;

/// Part of the controller body, in normalised coordinates.
///
/// Coordinates are 0..1 on a 2:1 canvas, so the front-end can size the picture
/// however it likes. Radii are fractions of the canvas *height*.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub kind: String, // "rect" | "circle" | "polygon"
    pub points: Vec<f64>,
    #[serde(default)]
    pub radius: f64,
}

fn default_radius() -> f64 {
    0.045
}

fn default_kind() -> String {
    "button".to_owned()
}

/// One thing the user will be asked to press.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutControl {
    /// What SDL and RetroArch are told. Several controls can share one -- a
    /// SNES `Y` and an Xbox `X` are the same canonical button -- which is the
    /// entire reason this is separate from `label`.
    pub canonical: Control,
    /// What the user sees, in the hardware's own words.
    pub label: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_kind")]
    pub kind: String, // "button" | "shoulder" | "dpad" | "stick"
    #[serde(default = "default_radius")]
    pub radius: f64,
    /// RetroArch autoconfig key, when this console does not use the one the
    /// canonical name implies.
    ///
    /// Cores map the abstract RetroPad onto real console buttons themselves,
    /// and not identically. mupen64plus-next reads N64 B from RetroPad **Y**,
    /// so binding the physical B to the key the canonical name suggests
    /// produces a button that does nothing at all. The console's own wiring
    /// belongs with the console's layout.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub retroarch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub id: String,
    pub label: String,
    /// How to name this console inside a sentence, when `label` does not read
    /// as one. "Arcade stick games" describes the controller rather than the
    /// games; "Arcade games" is what the scope actually covers. Data rather
    /// than a rule, because the exceptions are per-console.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub console_label: String,
    /// Optional artwork, as a filename the front-end resolves against its own
    /// directory. Drawn behind the control dots in place of `shapes`.
    ///
    /// `shapes` is still required when an image is given, and is what gets
    /// drawn if the file is missing: an image and a set of coordinates are two
    /// things that can drift apart, and an arrow pointing at the wrong part of
    /// a photograph looks exactly like one pointing at the right part.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    pub controls: Vec<LayoutControl>,
}

impl Layout {
    /// Canonical control -> RetroArch key, for the controls that override it.
    pub fn retroarch_keys(&self) -> BTreeMap<Control, String> {
        self.controls
            .iter()
            .filter(|control| !control.retroarch.is_empty())
            .map(|control| (control.canonical, control.retroarch.clone()))
            .collect()
    }

    /// The controls, in the order the wizard will ask about them.
    pub fn order(&self) -> Vec<Control> {
        self.controls
            .iter()
            .map(|control| control.canonical)
            .collect()
    }
}

/// What `data/layouts.json` says about the set as a whole.
#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    /// Catalogue order. Also decides which layouts exist at all.
    order: Vec<String>,
    default: String,
    /// libretro core name -> the layout whose key table that core reads.
    cores: BTreeMap<String, String>,
}

const MANIFEST_JSON: &str = include_str!("../data/layouts.json");

/// Every shipped layout file, paired with the id the manifest expects.
///
/// `include_str!` needs a literal path, so this list is the one place a new
/// console has to be named in code. It is checked against the manifest by
/// [`tests::the_manifest_and_the_embedded_files_agree`], so the two cannot
/// drift without a test saying so.
const LAYOUT_FILES: &[(&str, &str)] = &[
    ("generic", include_str!("../data/layouts/generic.json")),
    ("snes", include_str!("../data/layouts/snes.json")),
    ("n64", include_str!("../data/layouts/n64.json")),
    ("arcade", include_str!("../data/layouts/arcade.json")),
    ("gamecube", include_str!("../data/layouts/gamecube.json")),
    ("ps2", include_str!("../data/layouts/ps2.json")),
    ("switch", include_str!("../data/layouts/switch.json")),
    ("genesis", include_str!("../data/layouts/genesis.json")),
];

struct Catalogue {
    manifest: Manifest,
    /// In manifest order, which is catalogue order.
    layouts: Vec<Layout>,
}

fn catalogue() -> &'static Catalogue {
    static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        let manifest: Manifest =
            serde_json::from_str(MANIFEST_JSON).expect("data/layouts.json is malformed");
        let layouts = manifest
            .order
            .iter()
            .map(|id| {
                let (_, json) = LAYOUT_FILES
                    .iter()
                    .find(|(name, _)| name == id)
                    .unwrap_or_else(|| panic!("no embedded layout file for {id:?}"));
                serde_json::from_str(json)
                    .unwrap_or_else(|error| panic!("data/layouts/{id}.json: {error}"))
            })
            .collect();
        Catalogue { manifest, layouts }
    })
}

/// Every layout a user may pick from, in a stable order.
///
/// Sent to the front-end rather than duplicated there. A hardcoded list in the
/// theme would be a second copy of this table with nothing to notice when it
/// fell behind -- a console added here would simply never appear, which looks
/// exactly like the picker being broken.
pub fn all() -> &'static [Layout] {
    &catalogue().layouts
}

/// The id of the layout used when nothing more specific is known.
pub fn default_id() -> &'static str {
    &catalogue().manifest.default
}

/// The layout used when nothing more specific is known.
pub fn default_layout() -> &'static Layout {
    get(default_id())
}

/// The layouts that name an actual console, in catalogue order.
///
/// `generic` is a layout but not a console: "my pad, when playing generic
/// games" is not a thing anyone can mean, and offering it as a mapping scope
/// would produce a scope that never resolves because no core ever reports it.
pub fn consoles() -> Vec<&'static str> {
    all()
        .iter()
        .map(|layout| layout.id.as_str())
        .filter(|id| *id != default_id())
        .collect()
}

/// A layout by id, falling back to the generic pad.
///
/// Never fails: an unknown id comes from stored state or a front-end, and
/// refusing to show a wizard at all is a worse answer than showing the ordinary
/// one.
pub fn get(layout_id: &str) -> &'static Layout {
    let catalogue = catalogue();
    catalogue
        .layouts
        .iter()
        .find(|layout| layout.id == layout_id)
        .or_else(|| {
            catalogue
                .layouts
                .iter()
                .find(|layout| layout.id == catalogue.manifest.default)
        })
        .expect("the default layout must exist")
}

/// Whether a layout id names something actually shipped.
pub fn exists(layout_id: &str) -> bool {
    all().iter().any(|layout| layout.id == layout_id)
}

/// Where a layout sits in the catalogue, or 0 for one that is not in it.
pub fn index_of(layout_id: &str) -> usize {
    all()
        .iter()
        .position(|layout| layout.id == layout_id)
        .unwrap_or(0)
}

/// The layout matching a controller icon the user already chose.
///
/// The icon set and the layout set overlap by design -- someone who has said
/// "this is an N64 controller" should not be asked again in different words.
pub fn for_icon(icon: &str) -> &'static Layout {
    get(icon)
}

/// The console a libretro core plays, as a layout id, or `""`.
///
/// Empty rather than `generic` for a core nothing is known about. The two are
/// not the same answer: `generic` is a layout somebody could deliberately map
/// to, while `""` means "no console context", which resolution has to treat as
/// "skip the console scope" rather than "look for a mapping filed under the
/// generic pad".
pub fn for_core(core: &str) -> &'static str {
    if core.is_empty() {
        return "";
    }
    let mut name = core.rsplit('/').next().unwrap_or(core);
    // Strip the platform's library suffix, however it is spelled, then the
    // libretro marker: `mupen64plus_next_libretro.so` -> `mupen64plus_next`.
    for suffix in [".so", ".dll", ".dylib"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped;
            break;
        }
    }
    for marker in ["_libretro", "-libretro"] {
        if let Some(stripped) = name.strip_suffix(marker) {
            name = stripped;
            break;
        }
    }
    catalogue()
        .manifest
        .cores
        .get(&name.to_lowercase())
        .map(String::as_str)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_layout_parses() {
        // The whole point of the data files: a malformed one must fail here and
        // not in front of somebody holding a controller.
        assert_eq!(all().len(), LAYOUT_FILES.len());
    }

    #[test]
    fn the_manifest_and_the_embedded_files_agree() {
        let embedded: Vec<&str> = LAYOUT_FILES.iter().map(|(id, _)| *id).collect();
        assert_eq!(catalogue().manifest.order, embedded);
    }

    #[test]
    fn every_shipped_layout_is_well_formed() {
        for layout in all() {
            assert!(!layout.id.is_empty(), "a layout with no id");
            assert!(!layout.label.is_empty(), "{} has no label", layout.id);
            assert!(!layout.controls.is_empty(), "{} has no controls", layout.id);
            assert!(
                !layout.shapes.is_empty(),
                "{} has no shapes, so there is nothing to draw if its image is \
                 missing",
                layout.id
            );
            for control in &layout.controls {
                assert!(
                    (0.0..=1.0).contains(&control.x) && (0.0..=1.0).contains(&control.y),
                    "{}'s {} sits off the canvas at ({}, {})",
                    layout.id,
                    control.canonical,
                    control.x,
                    control.y
                );
                assert!(
                    control.radius > 0.0,
                    "{}'s {} is invisible",
                    layout.id,
                    control.canonical
                );
                assert!(
                    !control.label.is_empty(),
                    "{}'s {} has no label",
                    layout.id,
                    control.canonical
                );
                assert!(
                    matches!(
                        control.kind.as_str(),
                        "button" | "shoulder" | "dpad" | "stick"
                    ),
                    "{}'s {} has kind {:?}, which no front-end draws",
                    layout.id,
                    control.canonical,
                    control.kind
                );
            }
            for shape in &layout.shapes {
                match shape.kind.as_str() {
                    "rect" => assert_eq!(shape.points.len(), 4, "{}: a rect is x,y,w,h", layout.id),
                    "circle" => {
                        assert_eq!(shape.points.len(), 2, "{}: a circle is x,y", layout.id);
                        assert!(shape.radius > 0.0, "{}: a circle needs a radius", layout.id);
                    }
                    "polygon" => assert!(
                        shape.points.len() >= 6 && shape.points.len() % 2 == 0,
                        "{}: a polygon is pairs, at least three",
                        layout.id
                    ),
                    other => panic!("{}: no front-end draws a {other:?}", layout.id),
                }
            }
        }
    }

    #[test]
    fn no_layout_asks_about_the_same_control_twice() {
        // Two prompts for one canonical control means the second silently
        // overwrites the first, and one of the two presses is thrown away.
        for layout in all() {
            let mut seen = layout.order();
            let before = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), before, "{} repeats a control", layout.id);
        }
    }

    #[test]
    fn layout_ids_are_unique() {
        let mut ids: Vec<&str> = all().iter().map(|layout| layout.id.as_str()).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before);
    }

    #[test]
    fn an_unknown_id_falls_back_to_the_generic_pad_rather_than_failing() {
        assert_eq!(get("no-such-console").id, default_id());
        assert_eq!(get("").id, default_id());
        assert!(!exists("no-such-console"));
        assert_eq!(index_of("no-such-console"), 0);
    }

    #[test]
    fn the_generic_pad_is_a_layout_but_not_a_console() {
        assert!(exists(default_id()));
        assert!(!consoles().contains(&default_id()));
        assert_eq!(consoles().len(), all().len() - 1);
    }

    #[test]
    fn a_core_resolves_to_its_console_through_every_spelling() {
        for core in [
            "mupen64plus_next",
            "mupen64plus_next_libretro",
            "mupen64plus_next_libretro.so",
            "mupen64plus_next-libretro.dll",
            "/nix/store/abc-cores/mupen64plus_next_libretro.so",
            "Mupen64plus_Next_libretro.so",
        ] {
            assert_eq!(for_core(core), "n64", "{core} did not resolve");
        }
    }

    #[test]
    fn the_library_suffix_is_matched_case_sensitively() {
        // Pinning what the Python did rather than improving on it: the suffix
        // is stripped before the name is lowercased, so an uppercase ".SO"
        // survives into the lookup and misses. Linux cores are lowercase, so
        // this has never bitten; changing it here without changing the Python
        // would make the two disagree, which is worse than the quirk.
        assert_eq!(for_core("MUPEN64PLUS_NEXT_LIBRETRO.SO"), "");
        assert_eq!(for_core("mupen64plus_next_libretro.SO"), "");
    }

    #[test]
    fn an_unknown_core_gives_no_console_rather_than_the_generic_one() {
        // "" means "skip the console scope". `generic` would mean "look for a
        // mapping filed under the generic pad", which is a different answer.
        assert_eq!(for_core(""), "");
        assert_eq!(for_core("some_core_libretro.so"), "");
        assert_ne!(for_core("some_core_libretro.so"), default_id());
    }

    #[test]
    fn every_core_in_the_manifest_names_a_layout_that_exists() {
        // A typo here resolves a mapping to a console nothing can render.
        for (core, layout_id) in &catalogue().manifest.cores {
            assert!(
                exists(layout_id),
                "core {core} names missing layout {layout_id}"
            );
        }
    }

    #[test]
    fn the_n64_layout_carries_the_override_its_core_needs() {
        // mupen64plus-next reads N64 B from RetroPad Y. Without this the
        // physical B is bound to a key that does nothing at all.
        let keys = get("n64").retroarch_keys();
        assert_eq!(
            keys.get(&Control::B).map(String::as_str),
            Some("input_y_btn")
        );
    }

    #[test]
    fn a_console_whose_core_needs_no_overrides_carries_none() {
        // SNES was read against snes9x's own source and needs none. An override
        // appearing here later is a claim that wants the same reading.
        assert!(get("snes").retroarch_keys().is_empty());
        assert!(get("ps2").retroarch_keys().is_empty());
    }

    #[test]
    fn the_n64_c_cluster_is_right_stick_halves_not_invented_face_buttons() {
        let order = get("n64").order();
        for control in [
            Control::RightStickUp,
            Control::RightStickDown,
            Control::RightStickLeft,
            Control::RightStickRight,
        ] {
            assert!(order.contains(&control), "n64 lost {control}");
        }
        assert!(!order.contains(&Control::X), "an N64 pad has no X");
        assert!(!order.contains(&Control::Y), "an N64 pad has no Y");
    }

    #[test]
    fn every_layout_can_produce_a_mapping_for_both_consumers() {
        // Every canonical name in every layout has to be spellable by both, or
        // the wizard asks for a press it can then do nothing with. The Control
        // enum makes this true by construction; this pins that it stays so.
        for layout in all() {
            for control in layout.order() {
                assert!(!control.sdl_field().is_empty());
                assert!(!control.retroarch_key().is_empty());
            }
        }
    }

    #[test]
    fn a_layout_round_trips_through_json() {
        let layout = get("gamecube");
        let json = serde_json::to_string(layout).expect("serialize");
        let back: Layout = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, layout);
    }

    #[test]
    fn catalogue_order_is_the_manifest_order() {
        let ids: Vec<&str> = all().iter().map(|layout| layout.id.as_str()).collect();
        assert_eq!(ids, catalogue().manifest.order);
        assert_eq!(index_of("n64"), 2);
    }
}
