//! SDL's side of a mapping, held to the byte.

use std::collections::BTreeMap;

use padmap_core::binding::Binding;
use padmap_core::control::{Control, CANONICAL_ORDER};
use padmap_core::fields::Fields;
use padmap_core::sdl::{
    clean_name, crc16, guid, line, mapping_line, parse_line, stick_fields, AxisSpan,
    STICK_REST_TOLERANCE,
};

const REAL_GUID: &str = "0600c9a7790000007918000001000000";
const REAL_NAME: &str = "padmap Player 1";
const BUS_VIRTUAL: u16 = 0x06;
const BUS_USB: u16 = 0x03;

const FORBIDDEN: [char; 11] = [
    ',', '\n', '\r', '\x0b', '\x0c', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}',
];

/// The ten of those that break a line rather than a field.
const LINE_BREAKERS: [char; 10] = [
    '\n', '\r', '\x0b', '\x0c', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}',
];

fn physical_lines(text: &str) -> usize {
    text.split(|c| LINE_BREAKERS.contains(&c)).count()
}

fn word(raw: &str, index: usize) -> &str {
    &raw[index * 4..index * 4 + 4]
}

fn differing_words(left: &str, right: &str) -> Vec<usize> {
    (0..8)
        .filter(|i| word(left, *i) != word(right, *i))
        .collect()
}

fn entries(fields: &Fields) -> Vec<(&str, &str)> {
    fields
        .iter()
        .map(|(name, target)| (name.as_str(), target.as_str()))
        .collect()
}

fn field_names(fields: &Fields) -> Vec<&str> {
    fields.iter().map(|(name, _)| name.as_str()).collect()
}

// ---------------------------------------------------------------------------

#[test]
fn crc16_matches_the_standard_arc_check_vector() {
    // The published check value for CRC-16/ARC. If this moves, every GUID.
    assert_eq!(crc16(b"123456789"), 0xBB3D);
}

#[test]
fn crc16_of_no_bytes_is_zero() {
    assert_eq!(crc16(b""), 0x0000);
}

#[test]
fn crc16_of_nul_bytes_is_indistinguishable_from_nothing() {
    // Zero initial value and a zero byte leave the register alone, so a name.
    assert_eq!(crc16(b"\x00"), 0x0000);
    assert_eq!(crc16(b"\x00\x00\x00"), 0x0000);
}

#[test]
fn crc16_of_a_single_byte_is_pinned() {
    assert_eq!(crc16(b"A"), 0x30C0);
    assert_eq!(crc16(b"B"), 0x3180);
    assert_eq!(crc16(b"\xff"), 0x4040);
}

#[test]
fn crc16_is_byte_order_sensitive() {
    // A checksum that ignored order would give two differently-named pads the.
    assert_eq!(crc16(b"AB"), 0x61B0);
    assert_eq!(crc16(b"BA"), 0x90F0);
    assert_ne!(crc16(b"AB"), crc16(b"BA"));
}

#[test]
fn crc16_hashes_utf8_bytes_and_not_characters() {
    // "Pokémon" is seven characters and eight bytes.
    assert_eq!("Pokémon".chars().count(), 7);
    assert_eq!(
        "Pokémon".len(),
        8,
        "len() is bytes, which is what the CRC eats"
    );
    assert_eq!(crc16("Pokémon".as_bytes()), 0xADCA);
    assert_eq!(crc16("Pokemon".as_bytes()), 0x9E5F);
}

#[test]
fn a_trailing_nul_still_changes_the_checksum_of_a_non_empty_name() {
    // The NUL is transparent from a zero register, not transparent in general.
    assert_eq!(crc16(b"A\x00"), 0x5030);
    assert_ne!(crc16(b"A\x00"), crc16(b"A"));
}

#[test]
fn crc16_is_deterministic_for_a_long_input() {
    let long = "x".repeat(10_000);
    assert_eq!(crc16(long.as_bytes()), crc16(long.as_bytes()));
    assert_ne!(crc16(long.as_bytes()), crc16(b""));
}

#[test]
fn the_guid_matches_one_sdl_wrote_itself() {
    assert_eq!(
        guid(BUS_VIRTUAL, 0x0079, 0x1879, 0x0001, REAL_NAME),
        REAL_GUID
    );
}

#[test]
fn a_second_guid_sdl_wrote_also_matches() {
    assert_eq!(
        guid(BUS_USB, 0x0079, 0x1830, 0x0110, "Arcade Fightstick F300"),
        "03006cc5790000003018000010010000"
    );
}

#[test]
fn the_same_inputs_always_give_the_same_guid() {
    // Regenerating a mapping must key on the same row, or the previous line.
    let once = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, REAL_NAME);
    let twice = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, REAL_NAME);
    assert_eq!(once, twice);
}

#[test]
fn every_word_is_stored_little_endian() {
    // SDL writes the bytes of each 16-bit field low byte first.
    assert_eq!(
        guid(0x1234, 0x5678, 0x9abc, 0xdef0, ""),
        "3412000078560000bc9a0000f0de0000"
    );
}

#[test]
fn the_bus_occupies_the_first_four_digits_and_nothing_else() {
    let usb = guid(BUS_USB, 0x0079, 0x1830, 0x0110, "Arcade Fightstick F300");
    let virt = guid(
        BUS_VIRTUAL,
        0x0079,
        0x1830,
        0x0110,
        "Arcade Fightstick F300",
    );
    assert_eq!(word(&usb, 0), "0300");
    assert_eq!(word(&virt, 0), "0600");
    assert_eq!(
        differing_words(&usb, &virt),
        vec![0],
        "the bus moved a second field"
    );
}

#[test]
fn the_name_checksum_occupies_digits_four_to_seven_and_nothing_else() {
    let one = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, "padmap Player 1");
    let two = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, "padmap Player 2");
    assert_eq!(word(&one, 1), "c9a7");
    assert_eq!(word(&two, 1), "89a6");
    assert_eq!(
        differing_words(&one, &two),
        vec![1],
        "renaming a pad must move the checksum and nothing else"
    );
}

#[test]
fn the_vendor_occupies_digits_eight_to_eleven_and_nothing_else() {
    let one = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, REAL_NAME);
    let two = guid(BUS_VIRTUAL, 0x045e, 0x1879, 1, REAL_NAME);
    assert_eq!(word(&one, 2), "7900");
    assert_eq!(word(&two, 2), "5e04");
    assert_eq!(differing_words(&one, &two), vec![2]);
}

#[test]
fn the_product_occupies_digits_sixteen_to_nineteen_and_nothing_else() {
    let one = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, REAL_NAME);
    let two = guid(BUS_VIRTUAL, 0x0079, 0x028e, 1, REAL_NAME);
    assert_eq!(word(&one, 4), "7918");
    assert_eq!(word(&two, 4), "8e02");
    assert_eq!(differing_words(&one, &two), vec![4]);
}

#[test]
fn the_version_occupies_digits_twenty_four_to_twenty_seven_and_nothing_else() {
    let one = guid(BUS_VIRTUAL, 0x0079, 0x1879, 0x0001, REAL_NAME);
    let two = guid(BUS_VIRTUAL, 0x0079, 0x1879, 0x0110, REAL_NAME);
    assert_eq!(word(&one, 6), "0100");
    assert_eq!(word(&two, 6), "1001");
    assert_eq!(differing_words(&one, &two), vec![6]);
}

#[test]
fn the_padding_words_are_always_zero() {
    // SDL's GUID is sixteen bytes with three 16-bit holes in it.
    let cases = [
        guid(0x0000, 0x0000, 0x0000, 0x0000, ""),
        guid(0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, "anything at all"),
        guid(BUS_VIRTUAL, 0x0079, 0x1879, 0x0001, REAL_NAME),
    ];
    for computed in cases {
        for index in [3, 5, 7] {
            assert_eq!(
                word(&computed, index),
                "0000",
                "padding word {index} of {computed} is not zero"
            );
        }
    }
}

#[test]
fn an_all_zero_device_gives_an_all_zero_guid() {
    assert_eq!(guid(0, 0, 0, 0, ""), "00000000000000000000000000000000");
}

#[test]
fn a_guid_is_always_thirty_two_lowercase_hex_characters() {
    // A short or upper-case GUID does not match, and a caller comparing it.
    let control_chars = "\u{0}\u{1}\u{2}\u{1f}\u{7f}";
    let very_long = "x".repeat(10_000);
    let names: [&str; 5] = ["", REAL_NAME, &very_long, control_chars, "Pokémon"];
    for name in names {
        for (bus, vendor, product, version) in [
            (0x0000, 0x0000, 0x0000, 0x0000),
            (0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF),
        ] {
            let computed = guid(bus, vendor, product, version, name);
            assert_eq!(computed.len(), 32, "{computed:?} is not 32 characters");
            assert!(
                computed
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{computed:?} is not lowercase hex"
            );
        }
    }
}

#[test]
fn a_name_that_differs_only_beyond_the_ascii_range_gets_a_different_guid() {
    let plain = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, "Pokemon");
    let accented = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, "Pokémon");
    assert_eq!(accented, "0600caad790000007918000001000000");
    assert_ne!(plain, accented);
}

#[test]
fn an_ordinary_name_passes_through_clean_name_untouched() {
    assert_eq!(clean_name(REAL_NAME), REAL_NAME);
    assert_eq!(clean_name("8BitDo SN30 Pro+"), "8BitDo SN30 Pro+");
}

#[test]
fn every_forbidden_character_is_removed_from_a_name() {
    for bad in FORBIDDEN {
        let cleaned = clean_name(&format!("Pad{bad}Two"));
        assert_eq!(cleaned, "PadTwo", "{bad:?} survived clean_name");
    }
}

#[test]
fn every_forbidden_character_leaves_exactly_one_physical_line_with_the_right_field_count() {
    let fields: Fields = [("a", "b1"), ("b", "b2")].into_iter().collect();
    for bad in FORBIDDEN {
        let name = format!("Pad{bad}Two");
        let built = line(REAL_GUID, &name, &fields, "Linux");
        assert_eq!(built.lines().count(), 1, "{bad:?} split the line for SDL");
        assert_eq!(
            physical_lines(&built),
            1,
            "{bad:?} split the line for padmap's own rewriter, which would then \
             keep half of it as a line it does not own"
        );
        let parts: Vec<&str> = built.split(',').collect();
        assert_eq!(parts.len(), 6, "{bad:?} changed the field count: {built:?}");
        assert_eq!(parts[1], "PadTwo", "{bad:?} survived into the name field");
        assert_eq!(parts[2], "a:b1", "{bad:?} shifted the bindings along");
        assert_eq!(parts[3], "b:b2");
        assert_eq!(parts[4], "platform:Linux");
    }
}

#[test]
fn a_comma_in_a_name_cannot_shift_every_field_after_it() {
    let fields: Fields = [("a", "b1"), ("b", "b2")].into_iter().collect();
    let built = line(REAL_GUID, "Evil, Pad", &fields, "Linux");
    let parts: Vec<&str> = built.split(',').collect();
    assert_eq!(parts[0], REAL_GUID);
    assert_eq!(parts[1], "Evil Pad");
    assert_eq!(parts[2], "a:b1");
    assert_eq!(parts[3], "b:b2");
    assert_eq!(parts[4], "platform:Linux");
    assert_eq!(parts[5], "", "the line must end with a comma");
    assert_eq!(parts.len(), 6);
}

#[test]
fn a_newline_in_a_name_cannot_produce_two_physical_lines() {
    for bad in LINE_BREAKERS {
        let built = line(REAL_GUID, &format!("Pad{bad}Two"), &Fields::new(), "Linux");
        assert_eq!(physical_lines(&built), 1, "{bad:?} split the line");
        assert_eq!(built.split(',').count(), 4, "guid, name, platform, tail");
    }
}

#[test]
fn a_carriage_return_is_stripped_rather_than_riding_along_inside_the_name() {
    // SDL trims CR when it splits lines, so a stray one inside the name would.
    let built = line(REAL_GUID, "Pad\r", &Fields::new(), "Linux");
    assert!(!built.contains('\r'));
    assert!(built.contains(",Pad,"));
}

#[test]
fn a_name_of_nothing_but_forbidden_characters_becomes_empty() {
    let hostile: String = FORBIDDEN.iter().collect();
    assert_eq!(clean_name(&hostile), "");
    let built = line(REAL_GUID, &hostile, &Fields::new(), "Linux");
    assert_eq!(built, format!("{REAL_GUID},,platform:Linux,"));
    assert_eq!(physical_lines(&built), 1);
}

#[test]
fn an_empty_name_still_produces_a_well_formed_line() {
    let built = line(REAL_GUID, "", &Fields::new(), "Linux");
    assert_eq!(built, format!("{REAL_GUID},,platform:Linux,"));
    let (_, name, _) = parse_line(&built).expect("an empty-named line still parses");
    assert_eq!(name, "");
}

#[test]
fn a_colon_in_a_name_survives_and_does_not_become_a_field_separator() {
    let built = line(
        REAL_GUID,
        "Mayflash Stick: 2 player",
        &Fields::new(),
        "Linux",
    );
    assert!(built.contains(",Mayflash Stick: 2 player,"));
    let (_, name, fields) = parse_line(&built).expect("parse");
    assert_eq!(name, "Mayflash Stick: 2 player");
    assert_eq!(
        field_names(&fields),
        ["platform"],
        "the name became a field"
    );
}

#[test]
fn characters_outside_the_forbidden_set_survive_a_name() {
    // The set is deliberately the framing characters and nothing else.
    for keep in [
        '\t', '\u{0}', '\u{1f}', '\u{7f}', '🎮', '\u{200f}', '\u{202e}', '\u{a0}',
    ] {
        let name = format!("Pad{keep}Two");
        assert_eq!(clean_name(&name), name, "{keep:?} was stripped");
        let built = line(REAL_GUID, &name, &Fields::new(), "Linux");
        assert!(built.contains(keep), "{keep:?} was dropped from the line");
        assert_eq!(physical_lines(&built), 1, "{keep:?} is not a line breaker");
    }
}

#[test]
fn the_fields_are_written_in_the_order_they_were_set() {
    // Not sorted: regenerating a mapping must not reshuffle the file, or every.
    let fields: Fields = [("dpup", "h0.1"), ("a", "b1"), ("start", "b7")]
        .into_iter()
        .collect();
    let built = line(REAL_GUID, REAL_NAME, &fields, "Linux");
    assert_eq!(
        built,
        format!("{REAL_GUID},{REAL_NAME},dpup:h0.1,a:b1,start:b7,platform:Linux,")
    );
}

#[test]
fn the_platform_is_written_last_and_is_whatever_the_caller_said() {
    let built = line(REAL_GUID, REAL_NAME, &Fields::new(), "Windows");
    assert!(built.ends_with("platform:Windows,"));
}

#[test]
fn the_guid_is_written_verbatim_rather_than_reformatted() {
    let odd = "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF";
    let built = line(odd, REAL_NAME, &Fields::new(), "Linux");
    assert!(built.starts_with(odd));
}

#[test]
fn the_parser_ignores_blank_lines_and_comments() {
    for raw in [
        "",
        "   ",
        "\t",
        "\n",
        "# a comment",
        "  # indented comment",
        "#",
    ] {
        assert_eq!(parse_line(raw), None, "{raw:?} was read as a mapping");
    }
}

#[test]
fn a_first_field_that_is_not_exactly_thirty_two_characters_is_refused() {
    let short = "0".repeat(31);
    let long = "0".repeat(33);
    assert_eq!(
        parse_line(&format!("{short},Pad,a:b0,")),
        None,
        "31 was accepted"
    );
    assert_eq!(
        parse_line(&format!("{long},Pad,a:b0,")),
        None,
        "33 was accepted"
    );
    assert!(
        parse_line(&format!("{REAL_GUID},Pad,a:b0,")).is_some(),
        "32 was refused"
    );
}

#[test]
fn a_thirty_two_character_first_field_of_non_ascii_is_refused() {
    // The length test is over bytes here, where the Python counted characters.
    let accented = "é".repeat(32);
    assert_eq!(parse_line(&format!("{accented},Pad,a:b0,")), None);
}

#[test]
fn a_line_with_no_fields_at_all_is_not_a_line() {
    assert_eq!(
        parse_line(REAL_GUID),
        None,
        "a guid with no name is not a line"
    );
    assert_eq!(parse_line("not,a,guid"), None);
    assert_eq!(
        parse_line(",,,"),
        None,
        "an empty first field is not a guid"
    );
}

#[test]
fn a_guid_and_a_name_with_nothing_after_them_parse_to_no_fields() {
    let (parsed_guid, name, fields) =
        parse_line(&format!("{REAL_GUID},Pad,")).expect("a bindingless line is still a line");
    assert_eq!(parsed_guid, REAL_GUID);
    assert_eq!(name, "Pad");
    assert!(fields.is_empty());
}

#[test]
fn the_parser_lowercases_the_guid_so_two_spellings_are_one_key() {
    let upper = REAL_GUID.to_uppercase();
    let (parsed, _, _) = parse_line(&format!("{upper},Pad,a:b0,")).expect("parse");
    assert_eq!(parsed, REAL_GUID);
    let mixed = "0600C9a7790000007918000001000000";
    let (parsed, _, _) = parse_line(&format!("{mixed},Pad,a:b0,")).expect("parse");
    assert_eq!(parsed, REAL_GUID);
}

#[test]
fn the_name_is_returned_verbatim_and_is_not_lowercased_or_trimmed() {
    let (_, name, _) = parse_line(&format!("{REAL_GUID}, padmap Player 1 ,a:b0,")).expect("parse");
    assert_eq!(name, " padmap Player 1 ");
}

#[test]
fn a_field_with_no_colon_is_skipped_without_losing_the_rest() {
    let (_, _, fields) =
        parse_line(&format!("{REAL_GUID},Pad,a:b0,broken,start:b7,")).expect("parse");
    assert_eq!(entries(&fields), [("a", "b0"), ("start", "b7")]);
}

#[test]
fn a_field_with_an_empty_name_or_an_empty_target_is_dropped() {
    // An empty target stored as a binding would be written straight back out as.
    let (_, _, fields) = parse_line(&format!("{REAL_GUID},Pad,a:b0,:b1,b:,")).expect("parse");
    assert_eq!(entries(&fields), [("a", "b0")]);
    assert_eq!(fields.get("b"), None);
    assert!(!fields.contains_key(""));
}

#[test]
fn a_field_whose_name_or_target_is_only_whitespace_is_dropped() {
    let (_, _, fields) = parse_line(&format!("{REAL_GUID},Pad, :b0,a:   ,x:b3,")).expect("parse");
    assert_eq!(entries(&fields), [("x", "b3")]);
}

#[test]
fn whitespace_around_a_field_name_and_target_is_trimmed() {
    let (_, _, fields) =
        parse_line(&format!("{REAL_GUID},Pad,  a  :  b0  , start :b7,")).expect("parse");
    assert_eq!(entries(&fields), [("a", "b0"), ("start", "b7")]);
}

#[test]
fn only_the_first_colon_separates_a_field_from_its_target() {
    let (_, _, fields) = parse_line(&format!("{REAL_GUID},Pad,a:b:c,")).expect("parse");
    assert_eq!(fields.get("a"), Some("b:c"));
}

#[test]
fn a_trailing_comma_does_not_create_an_empty_field() {
    let (_, _, fields) =
        parse_line(&format!("{REAL_GUID},Pad,a:b0,platform:Linux,")).expect("parse");
    assert_eq!(entries(&fields), [("a", "b0"), ("platform", "Linux")]);
}

#[test]
fn a_duplicate_field_keeps_its_first_position_and_its_last_value() {
    let (_, _, fields) =
        parse_line(&format!("{REAL_GUID},Pad,a:b0,start:b7,a:b5,")).expect("parse");
    assert_eq!(entries(&fields), [("a", "b5"), ("start", "b7")]);
}

#[test]
fn field_order_is_preserved_exactly_as_written() {
    let raw = format!("{REAL_GUID},Pad,dpup:h0.1,a:b1,leftx:a0,back:b6,platform:Linux,");
    let (_, _, fields) = parse_line(&raw).expect("parse");
    assert_eq!(
        field_names(&fields),
        ["dpup", "a", "leftx", "back", "platform"],
        "a sorted or hashed map would reshuffle the file on every rewrite"
    );
}

#[test]
fn platform_is_an_ordinary_field_and_not_special_cased() {
    let (_, _, fields) =
        parse_line(&format!("{REAL_GUID},Pad,a:b0,platform:Linux,")).expect("parse");
    assert_eq!(fields.get("platform"), Some("Linux"));
    assert!(fields.contains_key("platform"));
}

#[test]
fn a_line_with_leading_and_trailing_whitespace_still_parses() {
    let raw = format!("   {REAL_GUID},Pad,a:b0,  \n");
    let (parsed, name, fields) = parse_line(&raw).expect("parse");
    assert_eq!(parsed, REAL_GUID);
    assert_eq!(name, "Pad");
    assert_eq!(fields.get("a"), Some("b0"));
}

#[test]
fn a_line_round_trips_through_the_parser() {
    let fields: Fields = [("a", "b1"), ("dpup", "h0.1"), ("lefttrigger", "+a4")]
        .into_iter()
        .collect();
    let built = line(REAL_GUID, REAL_NAME, &fields, "Linux");
    let (parsed_guid, parsed_name, parsed_fields) =
        parse_line(&built).expect("the line we just built must parse");
    assert_eq!(parsed_guid, REAL_GUID);
    assert_eq!(parsed_name, REAL_NAME);
    assert_eq!(
        field_names(&parsed_fields),
        ["a", "dpup", "lefttrigger", "platform"],
        "a round trip that reorders the fields makes every rewrite a diff"
    );
    assert_eq!(parsed_fields.get("platform"), Some("Linux"));
}

#[test]
fn a_name_that_gets_cleaned_round_trips_as_the_cleaned_name() {
    for bad in FORBIDDEN {
        let dirty = format!("Pad{bad}Two");
        let built = line(REAL_GUID, &dirty, &Fields::new(), "Linux");
        let (_, name, _) = parse_line(&built).expect("parse");
        assert_eq!(
            name,
            clean_name(&dirty),
            "{bad:?} did not survive the round trip"
        );
        assert_eq!(name, "PadTwo");
    }
}

#[test]
fn a_wide_range_of_names_and_fields_round_trips() {
    let names = [
        "",
        "Pad",
        "padmap Player 1",
        "Pokémon 64 Controller",
        "Mayflash Stick: 2 player",
        "8BitDo SN30 Pro+",
        "Pad\twith\ttabs",
        "🎮 Player 1",
        "a name that is quite a lot longer than any descriptor ought to be",
    ];
    let field_sets: [Fields; 3] = [
        Fields::new(),
        [("a", "b1")].into_iter().collect(),
        [
            ("a", "b1"),
            ("b", "b2"),
            ("dpup", "h0.1"),
            ("-righty", "b11"),
            ("leftx", "a0"),
        ]
        .into_iter()
        .collect(),
    ];
    for name in names {
        for fields in &field_sets {
            let built = line(REAL_GUID, name, fields, "Linux");
            let (parsed_guid, parsed_name, parsed_fields) =
                parse_line(&built).expect("a line we built must parse");
            assert_eq!(parsed_guid, REAL_GUID);
            assert_eq!(parsed_name, clean_name(name));
            let mut expected: Vec<&str> = field_names(fields);
            expected.push("platform");
            assert_eq!(
                field_names(&parsed_fields),
                expected,
                "{name:?} lost or reordered a field"
            );
        }
    }
}

#[test]
fn a_computed_guid_round_trips_through_a_line_unchanged() {
    let computed = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, REAL_NAME);
    let built = line(&computed, REAL_NAME, &Fields::new(), "Linux");
    let (parsed, _, _) = parse_line(&built).expect("parse");
    assert_eq!(parsed, computed);
}

#[test]
fn an_empty_capture_still_writes_a_well_formed_line() {
    // A pad with no bindings yet must not produce a malformed line that breaks.
    let built = mapping_line(REAL_GUID, REAL_NAME, &BTreeMap::new(), "Linux", None);
    assert_eq!(built, format!("{REAL_GUID},{REAL_NAME},platform:Linux,"));
    assert!(parse_line(&built).is_some());
}

#[test]
fn controls_supplied_out_of_order_come_out_in_canonical_order() {
    let bindings: BTreeMap<Control, Binding> = [
        (Control::RightStickRight, Binding::button(14)),
        (Control::DpadUp, Binding::hat(0, 1)),
        (Control::Start, Binding::button(7)),
        (Control::A, Binding::button(1)),
        (Control::LeftTrigger, Binding::axis(4, 1)),
    ]
    .into_iter()
    .collect();
    let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", None);
    assert_eq!(
        built,
        format!(
            "{REAL_GUID},{REAL_NAME},a:b1,start:b7,lefttrigger:+a4,dpup:h0.1,\
             +rightx:b14,platform:Linux,"
        )
    );
}

#[test]
fn a_capture_of_every_control_writes_every_field() {
    let bindings: BTreeMap<Control, Binding> = [
        (Control::A, Binding::button(1)),
        (Control::B, Binding::button(2)),
        (Control::X, Binding::button(3)),
        (Control::Y, Binding::button(4)),
        (Control::Back, Binding::button(6)),
        (Control::Start, Binding::button(7)),
        (Control::LeftShoulder, Binding::button(9)),
        (Control::RightShoulder, Binding::button(10)),
        (Control::LeftTrigger, Binding::axis(4, 1)),
        (Control::RightTrigger, Binding::axis(5, 1)),
        (Control::DpadUp, Binding::hat(0, 1)),
        (Control::DpadDown, Binding::hat(0, 4)),
        (Control::DpadLeft, Binding::hat(0, 8)),
        (Control::DpadRight, Binding::hat(0, 2)),
        (Control::RightStickUp, Binding::button(11)),
        (Control::RightStickDown, Binding::button(12)),
        (Control::RightStickLeft, Binding::button(13)),
        (Control::RightStickRight, Binding::button(14)),
        // The analog stick, as a capture of a GameCube layout records it.
        (Control::LeftStickUp, Binding::axis(1, -1)),
        (Control::LeftStickDown, Binding::axis(1, 1)),
        (Control::LeftStickLeft, Binding::axis(0, -1)),
        (Control::LeftStickRight, Binding::axis(0, 1)),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        bindings.len(),
        CANONICAL_ORDER.len(),
        "the capture is not complete"
    );
    let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", None);
    assert_eq!(
        built,
        format!(
            "{REAL_GUID},{REAL_NAME},a:b1,b:b2,x:b3,y:b4,back:b6,start:b7,\
             leftshoulder:b9,rightshoulder:b10,lefttrigger:+a4,righttrigger:+a5,\
             dpup:h0.1,dpdown:h0.4,dpleft:h0.8,dpright:h0.2,-righty:b11,\
             +righty:b12,-rightx:b13,+rightx:b14,-lefty:-a1,+lefty:+a1,\
             -leftx:-a0,+leftx:+a0,platform:Linux,"
        )
    );
    let (_, _, fields) = parse_line(&built).expect("parse");
    assert_eq!(
        fields.len(),
        CANONICAL_ORDER.len() + 1,
        "every control plus the platform"
    );
}

#[test]
fn a_binding_sdl_cannot_express_is_left_out_rather_than_failing_the_whole_line() {
    let bindings: BTreeMap<Control, Binding> = [
        (Control::A, Binding::button(1)),
        (Control::DpadUp, Binding::hat(0, 3)),
        (Control::Start, Binding::button(7)),
    ]
    .into_iter()
    .collect();
    let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", None);
    assert_eq!(
        built,
        format!("{REAL_GUID},{REAL_NAME},a:b1,start:b7,platform:Linux,")
    );
    assert!(
        !built.contains("dpup"),
        "an inexpressible hat reached the file"
    );
}

#[test]
fn every_inexpressible_hat_value_is_left_out_and_the_rest_survive() {
    for value in [0, 3, 5, 6, 7, 9, 12, 15, -1] {
        let bindings: BTreeMap<Control, Binding> = [
            (Control::A, Binding::button(1)),
            (Control::DpadUp, Binding::hat(0, value)),
        ]
        .into_iter()
        .collect();
        let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", None);
        assert!(
            built.contains("a:b1"),
            "hat {value} cost the other controls"
        );
        assert!(!built.contains("dpup"), "hat {value} reached the file");
    }
}

#[test]
fn stick_fields_are_appended_after_the_captured_controls() {
    let bindings: BTreeMap<Control, Binding> = [
        (Control::A, Binding::button(1)),
        (Control::Start, Binding::button(7)),
    ]
    .into_iter()
    .collect();
    let sticks: Fields = [("leftx", "a0"), ("lefty", "a1")].into_iter().collect();
    let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", Some(&sticks));
    assert_eq!(
        built,
        format!("{REAL_GUID},{REAL_NAME},a:b1,start:b7,leftx:a0,lefty:a1,platform:Linux,")
    );
}

#[test]
fn a_stick_field_does_not_collide_with_the_half_axis_spelling_of_a_c_button() {
    let bindings: BTreeMap<Control, Binding> = [
        (Control::RightStickUp, Binding::button(11)),
        (Control::RightStickDown, Binding::button(12)),
        (Control::RightStickLeft, Binding::button(13)),
        (Control::RightStickRight, Binding::button(14)),
    ]
    .into_iter()
    .collect();
    let sticks: Fields = [("rightx", "a2"), ("righty", "a3")].into_iter().collect();
    let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", Some(&sticks));
    let (_, _, fields) = parse_line(&built).expect("parse");
    assert_eq!(
        field_names(&fields),
        ["-righty", "+righty", "-rightx", "+rightx", "rightx", "righty", "platform"]
    );
    assert_eq!(fields.get("-righty"), Some("b11"));
    assert_eq!(fields.get("righty"), Some("a3"));
}

#[test]
fn an_empty_stick_set_changes_nothing() {
    let bindings: BTreeMap<Control, Binding> =
        [(Control::A, Binding::button(1))].into_iter().collect();
    let with_none = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", None);
    let with_empty = mapping_line(
        REAL_GUID,
        REAL_NAME,
        &bindings,
        "Linux",
        Some(&Fields::new()),
    );
    assert_eq!(with_none, with_empty);
}

#[test]
fn a_mapping_line_cleans_the_name_the_same_way_a_plain_line_does() {
    let bindings: BTreeMap<Control, Binding> =
        [(Control::A, Binding::button(1))].into_iter().collect();
    let built = mapping_line(REAL_GUID, "Evil,\nPad", &bindings, "Linux", None);
    assert_eq!(physical_lines(&built), 1);
    assert_eq!(
        built.split(',').count(),
        5,
        "guid, name, a:b1, platform, empty tail"
    );
    assert!(built.contains(",EvilPad,"));
}

#[test]
fn a_degenerate_span_is_refused_rather_than_dividing_by_zero() {
    // absinfo off a driver that reports nothing useful.
    assert!(!AxisSpan::new(0, 0, 0).rests_centred());
    assert!(!AxisSpan::new(5, 5, 5).rests_centred());
    assert!(
        !AxisSpan::new(10, 5, 7).rests_centred(),
        "an inverted range is not a stick"
    );
    assert!(!AxisSpan::new(i32::MAX, i32::MIN, 0).rests_centred());
}

#[test]
fn the_tolerance_boundary_is_inclusive() {
    assert_eq!(STICK_REST_TOLERANCE, 0.5);
    assert!(
        AxisSpan::new(0, 200, 150).rests_centred(),
        "0.5 must be inclusive"
    );
    assert!(
        AxisSpan::new(0, 200, 50).rests_centred(),
        "-0.5 must be inclusive"
    );
}

#[test]
fn just_past_the_tolerance_is_not_a_stick() {
    assert!(!AxisSpan::new(0, 200, 151).rests_centred());
    assert!(!AxisSpan::new(0, 200, 49).rests_centred());
}

#[test]
fn a_worn_n64_stick_resting_well_off_centre_is_still_a_stick() {
    // 174 on 0..255 is 36% deflected -- a stick whose spring has aged, not a.
    assert!(AxisSpan::new(0, 255, 174).rests_centred());
    assert!(AxisSpan::new(0, 255, 81).rests_centred());
}

#[test]
fn an_axis_resting_at_either_end_is_a_trigger_and_not_a_stick() {
    assert!(!AxisSpan::new(0, 255, 0).rests_centred());
    assert!(!AxisSpan::new(0, 255, 255).rests_centred());
    assert!(!AxisSpan::new(0, 255, 20).rests_centred());
    assert!(!AxisSpan::new(0, 255, 235).rests_centred());
}

#[test]
fn a_centred_axis_on_a_signed_range_is_a_stick() {
    assert!(AxisSpan::new(-32768, 32767, 0).rests_centred());
    assert!(AxisSpan::new(-32768, 32767, -1).rests_centred());
    assert!(!AxisSpan::new(-32768, 32767, 32767).rests_centred());
}

#[test]
fn a_rest_outside_the_declared_range_is_not_a_stick() {
    // A driver that lies about either bound should not have its axis promoted.
    assert!(!AxisSpan::new(0, 255, 1000).rests_centred());
    assert!(!AxisSpan::new(0, 255, -1000).rests_centred());
}

#[test]
fn a_one_step_range_is_decided_rather_than_crashing() {
    assert!(!AxisSpan::new(0, 1, 0).rests_centred());
    assert!(!AxisSpan::new(0, 1, 1).rests_centred());
}

#[test]
fn a_pad_with_no_axes_gets_no_stick_fields() {
    let fields = stick_fields(&[], &BTreeMap::new(), None);
    assert!(
        fields.is_empty(),
        "a pad with no axes must not be given sticks"
    );
}

#[test]
fn a_pad_with_only_a_left_stick_gets_only_the_left_stick() {
    let fields = stick_fields(&[0x00, 0x01], &BTreeMap::new(), None);
    assert_eq!(entries(&fields), [("leftx", "a0"), ("lefty", "a1")]);
}

#[test]
fn a_pad_with_only_a_right_stick_numbers_it_from_zero() {
    let fields = stick_fields(&[0x03, 0x04], &BTreeMap::new(), None);
    assert_eq!(entries(&fields), [("rightx", "a0"), ("righty", "a1")]);
}

#[test]
fn a_pad_with_four_axes_gets_both_sticks_in_a_fixed_order() {
    let fields = stick_fields(&[0x00, 0x01, 0x03, 0x04], &BTreeMap::new(), None);
    assert_eq!(
        entries(&fields),
        [
            ("leftx", "a0"),
            ("lefty", "a1"),
            ("rightx", "a2"),
            ("righty", "a3")
        ],
        "the output order must be leftx, lefty, rightx, righty"
    );
}

#[test]
fn the_output_order_does_not_follow_the_order_the_codes_arrive_in() {
    let shuffled = stick_fields(&[0x04, 0x00, 0x03, 0x01], &BTreeMap::new(), None);
    let ascending = stick_fields(&[0x00, 0x01, 0x03, 0x04], &BTreeMap::new(), None);
    assert_eq!(entries(&shuffled), entries(&ascending));
    assert_eq!(
        field_names(&shuffled),
        ["leftx", "lefty", "rightx", "righty"]
    );
}

#[test]
fn hat_codes_among_the_axes_do_not_shift_the_stick_indices() {
    let fields = stick_fields(
        &[0x00, 0x01, 0x03, 0x04, 0x10, 0x11],
        &BTreeMap::new(),
        None,
    );
    assert_eq!(
        entries(&fields),
        [
            ("leftx", "a0"),
            ("lefty", "a1"),
            ("rightx", "a2"),
            ("righty", "a3")
        ]
    );
}

#[test]
fn an_unclaimed_code_between_the_sticks_shifts_the_ones_after_it() {
    // ABS_Z (0x02) is a common analogue trigger.
    let fields = stick_fields(&[0x00, 0x01, 0x02, 0x03, 0x04], &BTreeMap::new(), None);
    assert_eq!(
        entries(&fields),
        [
            ("leftx", "a0"),
            ("lefty", "a1"),
            ("rightx", "a3"),
            ("righty", "a4")
        ]
    );
}

#[test]
fn an_axis_a_capture_already_claims_is_not_also_a_stick() {
    let bindings: BTreeMap<Control, Binding> = [(Control::LeftTrigger, Binding::axis(2, 1))]
        .into_iter()
        .collect();
    let fields = stick_fields(&[0x00, 0x01, 0x03, 0x04], &bindings, None);
    assert_eq!(
        entries(&fields),
        [("leftx", "a0"), ("lefty", "a1"), ("righty", "a3")]
    );
}

#[test]
fn a_button_or_hat_binding_does_not_claim_an_axis_of_the_same_number() {
    let bindings: BTreeMap<Control, Binding> = [
        (Control::A, Binding::button(0)),
        (Control::DpadUp, Binding::hat(1, 1)),
    ]
    .into_iter()
    .collect();
    let fields = stick_fields(&[0x00, 0x01], &bindings, None);
    assert_eq!(entries(&fields), [("leftx", "a0"), ("lefty", "a1")]);
}

#[test]
fn a_capture_claiming_every_axis_leaves_no_sticks() {
    let bindings: BTreeMap<Control, Binding> = [
        (Control::LeftTrigger, Binding::axis(0, 1)),
        (Control::RightTrigger, Binding::axis(1, 1)),
        (Control::RightStickUp, Binding::axis(2, -1)),
        (Control::RightStickDown, Binding::axis(3, 1)),
    ]
    .into_iter()
    .collect();
    let fields = stick_fields(&[0x00, 0x01, 0x03, 0x04], &bindings, None);
    assert!(fields.is_empty());
}

#[test]
fn an_axis_that_rests_at_one_end_is_refused_as_a_stick() {
    let axes: BTreeMap<u16, AxisSpan> = [
        (0x00, AxisSpan::new(0, 255, 128)),
        (0x01, AxisSpan::new(0, 255, 127)),
        (0x03, AxisSpan::new(0, 255, 20)),
        (0x04, AxisSpan::new(0, 255, 235)),
    ]
    .into_iter()
    .collect();
    let fields = stick_fields(&[0x00, 0x01, 0x03, 0x04], &BTreeMap::new(), Some(&axes));
    assert_eq!(entries(&fields), [("leftx", "a0"), ("lefty", "a1")]);
}

#[test]
fn no_absinfo_at_all_leaves_the_evdev_guess_standing() {
    let fields = stick_fields(&[0x00, 0x01, 0x03, 0x04], &BTreeMap::new(), None);
    assert_eq!(field_names(&fields), ["leftx", "lefty", "rightx", "righty"]);
}

#[test]
fn an_axis_missing_from_the_absinfo_keeps_the_guess_for_that_axis_alone() {
    let axes: BTreeMap<u16, AxisSpan> = [(0x03, AxisSpan::new(0, 255, 0))].into_iter().collect();
    let fields = stick_fields(&[0x00, 0x01, 0x03, 0x04], &BTreeMap::new(), Some(&axes));
    assert_eq!(
        entries(&fields),
        [("leftx", "a0"), ("lefty", "a1"), ("righty", "a3")]
    );
}

#[test]
fn an_empty_absinfo_map_is_the_same_as_none() {
    let empty: BTreeMap<u16, AxisSpan> = BTreeMap::new();
    let with_empty = stick_fields(&[0x00, 0x01, 0x03, 0x04], &BTreeMap::new(), Some(&empty));
    let with_none = stick_fields(&[0x00, 0x01, 0x03, 0x04], &BTreeMap::new(), None);
    assert_eq!(entries(&with_empty), entries(&with_none));
}

#[test]
fn a_degenerate_span_costs_that_axis_its_stick() {
    let axes: BTreeMap<u16, AxisSpan> = [
        (0x00, AxisSpan::new(0, 0, 0)),
        (0x01, AxisSpan::new(0, 255, 128)),
    ]
    .into_iter()
    .collect();
    let fields = stick_fields(&[0x00, 0x01], &BTreeMap::new(), Some(&axes));
    assert_eq!(entries(&fields), [("lefty", "a1")]);
}

#[test]
fn stick_fields_feed_straight_into_a_mapping_line() {
    let bindings: BTreeMap<Control, Binding> =
        [(Control::A, Binding::button(1))].into_iter().collect();
    let sticks = stick_fields(&[0x00, 0x01, 0x03, 0x04], &bindings, None);
    let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", Some(&sticks));
    assert_eq!(
        built,
        format!(
            "{REAL_GUID},{REAL_NAME},a:b1,leftx:a0,lefty:a1,rightx:a2,righty:a3,\
             platform:Linux,"
        )
    );
}

#[test]
fn fields_iterate_in_insertion_order_and_are_never_sorted() {
    let fields: Fields = [("z", "1"), ("a", "2"), ("m", "3")].into_iter().collect();
    assert_eq!(
        field_names(&fields),
        ["z", "a", "m"],
        "a sorted map would reshuffle the file and make every rewrite a diff"
    );
}

#[test]
fn re_setting_a_field_keeps_its_original_position() {
    let mut fields: Fields = [("a", "1"), ("b", "2"), ("c", "3")].into_iter().collect();
    fields.insert("a", "9");
    assert_eq!(entries(&fields), [("a", "9"), ("b", "2"), ("c", "3")]);
    assert_eq!(fields.len(), 3, "re-setting must not add a second entry");
}

#[test]
fn extend_overwrites_shared_keys_in_place_and_appends_new_ones_at_the_end() {
    let mut fields: Fields = [("a", "1"), ("b", "2")].into_iter().collect();
    let other: Fields = [("b", "9"), ("c", "3"), ("d", "4")].into_iter().collect();
    fields.extend(&other);
    assert_eq!(
        entries(&fields),
        [("a", "1"), ("b", "9"), ("c", "3"), ("d", "4")]
    );
}

#[test]
fn extend_takes_the_other_sides_order_for_the_keys_it_adds() {
    let mut fields: Fields = [("a", "1")].into_iter().collect();
    let other: Fields = [("z", "2"), ("m", "3")].into_iter().collect();
    fields.extend(&other);
    assert_eq!(field_names(&fields), ["a", "z", "m"]);
}

#[test]
fn extending_with_nothing_changes_nothing() {
    let mut fields: Fields = [("a", "1"), ("b", "2")].into_iter().collect();
    fields.extend(&Fields::new());
    assert_eq!(entries(&fields), [("a", "1"), ("b", "2")]);
}

#[test]
fn an_absent_field_reads_as_absent_rather_than_as_empty() {
    // `get` answering `Some("")` for a missing field would be written back out.
    let fields: Fields = [("a", "1")].into_iter().collect();
    assert_eq!(fields.get("b"), None);
    assert!(!fields.contains_key("b"));
    assert_eq!(fields.get(""), None);
    assert!(!fields.contains_key(""));
}

#[test]
fn a_field_set_to_an_empty_string_is_present_and_empty() {
    // Distinct from absent: the caller said so, and `contains_key` must agree.
    let fields: Fields = [("a", "")].into_iter().collect();
    assert_eq!(fields.get("a"), Some(""));
    assert!(fields.contains_key("a"));
}

#[test]
fn an_empty_field_set_is_empty_and_iterates_over_nothing() {
    let fields = Fields::new();
    assert!(fields.is_empty());
    assert!(entries(&fields).is_empty());
    assert_eq!(fields, Fields::default());
}

#[test]
fn field_lookup_is_case_sensitive_and_exact() {
    // SDL's field names are lower case and exact; a near-miss match would bind.
    let fields: Fields = [("dpup", "h0.1"), ("-righty", "b11")].into_iter().collect();
    assert_eq!(fields.get("dpup"), Some("h0.1"));
    assert_eq!(fields.get("DPUP"), None);
    assert_eq!(fields.get("dpu"), None);
    assert_eq!(fields.get("righty"), None, "-righty is not righty");
}
