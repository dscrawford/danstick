//! Layouts for mapping wizard: positions, labels, canonical controls.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::control::Control;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutControl {
    pub canonical: Control,
    /// Hardware label (user-facing).
    pub label: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default = "default_radius")]
    pub radius: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub console_label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    pub controls: Vec<LayoutControl>,
}

impl Layout {
    pub fn order(&self) -> Vec<Control> {
        self.controls
            .iter()
            .map(|control| control.canonical)
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    order: Vec<String>,
    default: String,
}

const MANIFEST_JSON: &str = include_str!("../data/layouts.json");

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

pub fn all() -> &'static [Layout] {
    &catalogue().layouts
}

pub fn default_id() -> &'static str {
    &catalogue().manifest.default
}

pub fn default_layout() -> &'static Layout {
    get(default_id())
}

pub fn consoles() -> Vec<&'static str> {
    all()
        .iter()
        .map(|layout| layout.id.as_str())
        .filter(|id| *id != default_id())
        .collect()
}

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

pub fn exists(layout_id: &str) -> bool {
    all().iter().any(|layout| layout.id == layout_id)
}

pub fn index_of(layout_id: &str) -> usize {
    all()
        .iter()
        .position(|layout| layout.id == layout_id)
        .unwrap_or(0)
}

pub fn for_icon(icon: &str) -> &'static Layout {
    get(icon)
}

/// The face buttons, whose labels differ between consoles at the same positions.
const FACES: [Control; 4] = [Control::A, Control::B, Control::X, Control::Y];

/// The letter a face button is labelled with, when it is a single letter.
fn face_letter(label: &str) -> Option<&str> {
    label
        .split_whitespace()
        .next()
        .filter(|word| word.len() == 1 && word.chars().all(|c| c.is_ascii_uppercase()))
}

/// Where each of `layout_id`'s face buttons goes on a 360 pad when its label is kept.
pub fn label_faces(layout_id: &str) -> BTreeMap<Control, Control> {
    let letters = |layout: &Layout| -> Option<BTreeMap<Control, String>> {
        layout
            .controls
            .iter()
            .filter(|control| FACES.contains(&control.canonical))
            .map(|control| Some((control.canonical, face_letter(&control.label)?.to_owned())))
            .collect()
    };
    if !exists(layout_id) {
        return BTreeMap::new();
    }
    let (Some(theirs), Some(xbox)) = (letters(get(layout_id)), letters(get("generic"))) else {
        return BTreeMap::new();
    };
    let by_letter: BTreeMap<&str, Control> = xbox
        .iter()
        .map(|(control, letter)| (letter.as_str(), *control))
        .collect();
    let Some(moved) = theirs
        .iter()
        .map(|(control, letter)| Some((*control, *by_letter.get(letter.as_str())?)))
        .collect::<Option<BTreeMap<Control, Control>>>()
    else {
        return BTreeMap::new();
    };
    let targets: std::collections::BTreeSet<&Control> = moved.values().collect();
    if targets.len() != moved.len() {
        return BTreeMap::new();
    }
    moved.into_iter().filter(|(from, to)| from != to).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nintendo_pad_kept_by_label_swaps_a_with_b_and_x_with_y() {
        let expected: BTreeMap<Control, Control> = [
            (Control::A, Control::B),
            (Control::B, Control::A),
            (Control::X, Control::Y),
            (Control::Y, Control::X),
        ]
        .into_iter()
        .collect();
        for id in ["switch", "snes", "wiiu"] {
            assert_eq!(label_faces(id), expected, "{id}");
        }
    }

    #[test]
    fn a_layout_labelled_as_the_360_or_otherwise_keeps_position() {
        for id in [
            "generic", "gamecube", "n64", "ps2", "arcade", "genesis", "nothing",
        ] {
            assert!(label_faces(id).is_empty(), "{id}: {:?}", label_faces(id));
        }
    }

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
    fn every_layout_can_produce_an_sdl_mapping() {
        for layout in all() {
            for control in layout.order() {
                assert!(!control.sdl_field().is_empty());
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
