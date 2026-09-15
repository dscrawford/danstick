//! Hold the title table to what the Python answered.
//!
//! `titles.py` reads two things padmap did not write: a 43MB MAME dump, and a
//! JSON table built from it at package time. Both arrive damaged sometimes --
//! a truncated write, a half-copied file, a hand-edited one -- and what the
//! Python does with each damaged shape is the specification, not an accident
//! to be tidied up in the port. One of those behaviours is visibly odd (see
//! `an_unclosed_game_tag_absorbs_the_next_one`) and is reproduced anyway,
//! because both implementations are installed and have to agree.

use std::collections::BTreeMap;
use std::path::Path;

use padmap_core::titles::{self, Title};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

#[test]
fn a_wrapped_description_is_joined_the_way_the_python_joins_it() {
    for case in corpus("flatten") {
        let raw = case["raw"].as_str().expect("raw");
        let want = case["flat"].as_str().expect("flat");
        assert_eq!(titles::flatten(raw), want, "{raw:?}");
    }
}

#[test]
fn every_cell_renders_as_the_python_renders_it() {
    for case in corpus("title_column") {
        let want = case["column"].as_str().expect("column");
        assert_eq!(titles::column(&case["raw"]), want, "{:?}", case["raw"]);
    }
}

fn as_map(raw: &Value) -> BTreeMap<String, Title> {
    raw.as_object()
        .expect("a table")
        .iter()
        .map(|(name, row)| {
            (
                name.clone(),
                Title {
                    title: row["title"].as_str().unwrap_or_default().to_owned(),
                    year: row["year"].as_str().unwrap_or_default().to_owned(),
                    manufacturer: row["manufacturer"].as_str().unwrap_or_default().to_owned(),
                    status: row["status"].as_str().unwrap_or_default().to_owned(),
                },
            )
        })
        .collect()
}

#[test]
fn every_damaged_table_loads_to_the_same_rows() {
    for case in corpus("title_tables") {
        let text = case["file"].as_str().expect("file");
        match case.get("raises") {
            // A table that is a list is not a table, and that is the contract:
            // it raises rather than answering with nothing, because "no games"
            // and "this file is not the file" are different problems.
            Some(_) => assert!(
                titles::load_json(text).is_err(),
                "{text:?} should not load at all"
            ),
            None => {
                let loaded = titles::load_json(text).expect("loads");
                assert_eq!(loaded.titles, as_map(&case["table"]), "for {text:?}");
            }
        }
    }
}

#[test]
fn working_is_false_only_for_a_preliminary_driver() {
    for case in corpus("title_tables") {
        let Some(table) = case["table"].as_object() else {
            continue;
        };
        let loaded = titles::load_json(case["file"].as_str().expect("file")).expect("loads");
        for (name, row) in table {
            let want = row["working"].as_bool().expect("working");
            assert_eq!(loaded.titles[name].working(), want, "{name}");
        }
    }
}

#[test]
fn the_mame_dump_parses_to_the_same_table() {
    for case in corpus("mame_xml") {
        let xml = case["xml"].as_str().expect("xml");
        let parsed = titles::parse_mame_xml(xml);
        let want = case["table"].as_object().expect("a table");
        assert_eq!(
            parsed.len(),
            want.len(),
            "different sets: ours {:?}, theirs {:?}",
            parsed.keys().collect::<Vec<_>>(),
            want.keys().collect::<Vec<_>>()
        );
        for (name, row) in want {
            let fields = row.as_array().expect("[title, year, maker, status]");
            let ours = &parsed[name];
            assert_eq!(ours.title, fields[0].as_str().expect("title"), "{name}");
            assert_eq!(ours.year, fields[1].as_str().expect("year"), "{name}");
            assert_eq!(
                ours.manufacturer,
                fields[2].as_str().expect("manufacturer"),
                "{name}"
            );
            assert_eq!(ours.status, fields[3].as_str().expect("status"), "{name}");
        }
    }
}

#[test]
fn an_unclosed_game_tag_absorbs_the_next_one() {
    // Recorded, not corrected. The scan runs from a `<game name="...">` to the
    // next `</game>`, so a dump whose tag is never closed reads through to the
    // *following* game's close and takes its year, manufacturer and driver
    // status. It is wrong in the sense that no sane reader would want it, and
    // right in the only sense that matters here: the Python does it, both are
    // installed, and a port that quietly disagreed would produce a different
    // arcade tab depending on which one built the table.
    //
    // The case is named so that whoever fixes it fixes both at once.
    let parsed = titles::parse_mame_xml(
        r#"<game name="unclosed">
  <description>Never Ends</description>
 <game name="after">
  <description>After</description>
  <driver status="good"/>
 </game>"#,
    );
    assert_eq!(parsed["unclosed"].title, "Never Ends");
    assert_eq!(
        parsed["unclosed"].status, "good",
        "the status belongs to the game after it"
    );
}

#[test]
fn a_rom_status_is_never_read_as_a_driver_status() {
    // <rom status="baddump"> appears thousands of times per file. Matching
    // `status="..."` anywhere would grade every one of those games by the
    // condition of one of its ROM images.
    let parsed = titles::parse_mame_xml(
        r#"<game name="x">
  <description>X</description>
  <rom name="a" status="baddump"/>
  <rom name="b" status="nodump"/>
  <driver status="imperfect"/>
 </game>"#,
    );
    assert_eq!(parsed["x"].status, "imperfect");
}

#[test]
fn a_playlist_entry_resolves_by_its_rom_name() {
    let table = BTreeMap::from([(
        "pacman".to_owned(),
        Title {
            title: "Pac-Man".to_owned(),
            year: "1980".to_owned(),
            manufacturer: "Namco".to_owned(),
            status: "good".to_owned(),
        },
    )]);
    for case in corpus("title_resolve") {
        let label = case["label"].as_str().expect("label");
        let path = case["path"].as_str().expect("path");
        let want = case["title"].as_str().expect("title");
        assert_eq!(
            titles::resolve(label, path, &table).title,
            want,
            "label {label:?}, path {path:?}"
        );
    }
}

#[test]
fn a_table_round_trips_through_its_dumped_form() {
    // `to_json` writes what `load_json` reads; the build writes one and the
    // runtime reads it, so a disagreement between them is a table that loads
    // as nothing on a machine where it was just built.
    let table = titles::parse_mame_xml(
        r#"<game name="pacman">
  <description>Pac-Man (Midway)</description>
  <year>1980</year>
  <manufacturer>Namco</manufacturer>
  <driver status="good"/>
 </game>"#,
    );
    let loaded = titles::load_json(&titles::to_json(&table)).expect("loads");
    assert_eq!(loaded.titles, table);
    assert!(loaded.dropped.is_empty());
}

#[test]
fn a_row_that_cannot_be_read_is_counted_rather_than_dropped_in_silence() {
    let loaded = titles::load_json(r#"{"a": ["A"], "b": 7, "c": [], "d": ["", "1"]}"#)
        .expect("the table itself is fine");
    assert_eq!(loaded.titles.keys().collect::<Vec<_>>(), vec!["a"]);
    assert_eq!(loaded.dropped, vec!["b", "c", "d"]);
}

#[test]
fn the_real_mame_dump_parses_to_the_table_the_python_built() {
    // The synthetic XML above covers the awkward shapes; this covers the
    // 43MB of real ones. Verified once by hand at 8833 sets with zero
    // differing rows, and gated on the environment so it can be repeated:
    //
    //     PADMAP_MAME_XML=$(nix eval --raw ...) \
    //     PADMAP_MAME_TABLE=$(nix build --print-out-paths .#mame-titles)/...
    //
    // Skipped, not failed, without them -- a build sandbox has neither, and a
    // 43MB fixture has no business living in the repository.
    let (Ok(xml), Ok(table)) = (
        std::env::var("PADMAP_MAME_XML"),
        std::env::var("PADMAP_MAME_TABLE"),
    ) else {
        eprintln!("skipping: set PADMAP_MAME_XML and PADMAP_MAME_TABLE to run");
        return;
    };
    let ours = titles::parse_mame_xml(&std::fs::read_to_string(xml).expect("the dump"));
    let theirs: BTreeMap<String, Vec<Value>> =
        serde_json::from_str(&std::fs::read_to_string(table).expect("the table"))
            .expect("the table is JSON");

    assert_eq!(ours.len(), theirs.len(), "different numbers of sets");
    for (name, row) in &theirs {
        let mine = ours
            .get(name)
            .unwrap_or_else(|| panic!("{name} is in the Python's table and not ours"));
        let field = |i: usize| row.get(i).and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            [
                mine.title.as_str(),
                mine.year.as_str(),
                mine.manufacturer.as_str(),
                mine.status.as_str()
            ],
            [field(0), field(1), field(2), field(3)],
            "{name}"
        );
    }
}
