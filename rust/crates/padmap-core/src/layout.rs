//! Layouts for mapping wizard: positions, labels, canonical controls.
//! Controls differ by console (N64 has no X/Y; SNES no analogue).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::control::Control;

/// Normalized coordinates (0..1 on 2:1 canvas); radii as height fractions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub kind: String,
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

/// One control in the wizard; canonical name shared across consoles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutControl {
    /// SDL/RetroArch canonical button (shared across consoles).
    pub canonical: Control,
    /// Hardware label (user-facing).
    pub label: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default = "default_radius")]
    pub radius: f64,
    /// RetroArch key override (when canonical key is wrong for this console).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub retroarch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub id: String,
    pub label: String,
    /// Grammatically correct console name (e.g. "Arcade games" not "Arcade stick games").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub console_label: String,
    /// Optional artwork; shapes required as fallback if image missing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    pub controls: Vec<LayoutControl>,
}

impl Layout {
    /// RetroArch key overrides (canonical -> key).
    pub fn retroarch_keys(&self) -> BTreeMap<Control, String> {
        self.controls
            .iter()
            .filter(|control| !control.retroarch.is_empty())
            .map(|control| (control.canonical, control.retroarch.clone()))
            .collect()
    }

    /// Controls in wizard order.
    pub fn order(&self) -> Vec<Control> {
        self.controls
            .iter()
            .map(|control| control.canonical)
            .collect()
    }
}

/// Manifest: order, default, core -> layout mappings.
#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    order: Vec<String>,
    default: String,
    cores: BTreeMap<String, String>,
}

const MANIFEST_JSON: &str = include_str!("../data/layouts.json");

/// Embedded layout files; checked against manifest in tests.
const LAYOUT_FILES: &[(&str, &str)] = &[
    ("generic", include_str!("../data/layouts/generic.json")),
    ("snes", include_str!("../data/layouts/snes.json")),
    ("n64", include_str!("../data/layouts/n64.json")),
    ("arcade", include_str!("../data/layouts/arcade.json")),
    ("gamecube", include_str!("../data/layouts/gamecube.json")),
    ("ps2", include_str!("../data/layouts/ps2.json")),
    ("switch", include_str!("../data/layouts/switch.json")),
    ("wiiu", include_str!("../data/layouts/wiiu.json")),
    ("genesis", include_str!("../data/layouts/genesis.json")),
];

struct Catalogue {
    manifest: Manifest,
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

/// All layouts in stable order (sent to front-end, not hardcoded there).
pub fn all() -> &'static [Layout] {
    &catalogue().layouts
}

/// Default layout id.
pub fn default_id() -> &'static str {
    &catalogue().manifest.default
}

/// Default layout.
pub fn default_layout() -> &'static Layout {
    get(default_id())
}

/// Actual console layouts in catalogue order (excludes generic).
pub fn consoles() -> Vec<&'static str> {
    all()
        .iter()
        .map(|layout| layout.id.as_str())
        .filter(|id| *id != default_id())
        .collect()
}

/// Layout by id; falls back to default (never fails).
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

/// Whether layout id is shipped.
pub fn exists(layout_id: &str) -> bool {
    all().iter().any(|layout| layout.id == layout_id)
}

/// Layout index in catalogue, or 0 if not found.
pub fn index_of(layout_id: &str) -> usize {
    all()
        .iter()
        .position(|layout| layout.id == layout_id)
        .unwrap_or(0)
}

/// Layout matching chosen controller icon.
pub fn for_icon(icon: &str) -> &'static Layout {
    get(icon)
}

/// Layout for libretro core, or "" if unknown (not "generic").
pub fn for_core(core: &str) -> &'static str {
    if core.is_empty() {
        return "";
    }
    // Trailing separators first, as `pathlib.Path(...).name` does and as
    // scope::game_key already did. The two read a path the same way or they
    // disagree about which console a launch is, which is a mapping resolved
    // against the wrong control set.
    let mut name = core.trim_end_matches('/').rsplit('/').next().unwrap_or("");
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
    fn a_trailing_separator_does_not_lose_the_core_name() {
        assert_eq!(for_core("mame/"), "arcade");
        assert_eq!(for_core("/usr/lib/libretro/mame/"), "arcade");
        assert_eq!(for_core("/"), "");
    }

    #[test]
    fn the_library_suffix_is_matched_case_sensitively() {
        assert_eq!(for_core("MUPEN64PLUS_NEXT_LIBRETRO.SO"), "");
        assert_eq!(for_core("mupen64plus_next_libretro.SO"), "");
    }

    #[test]
    fn an_unknown_core_gives_no_console_rather_than_the_generic_one() {
        assert_eq!(for_core(""), "");
        assert_eq!(for_core("some_core_libretro.so"), "");
        assert_ne!(for_core("some_core_libretro.so"), default_id());
    }

    #[test]
    fn every_core_in_the_manifest_names_a_layout_that_exists() {
        for (core, layout_id) in &catalogue().manifest.cores {
            assert!(
                exists(layout_id),
                "core {core} names missing layout {layout_id}"
            );
        }
    }

    #[test]
    fn the_n64_layout_carries_the_override_its_core_needs() {
        let keys = get("n64").retroarch_keys();
        assert_eq!(
            keys.get(&Control::B).map(String::as_str),
            Some("input_y_btn")
        );
    }

    #[test]
    fn a_console_whose_core_needs_no_overrides_carries_none() {
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
