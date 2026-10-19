use super::*;

#[test]
fn every_name_reads_back_from_its_id_to_the_same_bytes() {
    let names: [&[u8]; 7] = [b"", b"user:42", b"caf\xc3\xa9", b"back\\slash", b"\\x41 is not an escape", b"bin\xff\x00\xfe", b"\\\\\xc3"];
    for bytes in names {
        let name = KeyName::from(bytes.to_vec());
        assert_eq!(KeyName::from_id(&name.id()), name, "{:?} -> {:?}", bytes, name.id());
    }
}

#[test]
fn an_ordinary_name_is_its_own_id() {
    assert_eq!(KeyName::from("orders:2024:café").id(), "orders:2024:café");
}

#[test]
fn bytes_that_arent_utf8_are_written_as_hex_and_shown_so() {
    let name = KeyName::from(b"a\\b\xff".to_vec());
    assert_eq!(name.id(), "a\\\\b\\xff");
    assert_eq!(KeyName::display_id(&name.id()), "a\\b\\xff");
    assert_eq!(name.to_string_lossy(), "a\\b\u{fffd}");
}

#[test]
fn a_backslash_that_starts_no_escape_is_kept_as_written() {
    assert_eq!(KeyName::from_id("a\\q").as_bytes(), b"a\\q");
    assert_eq!(KeyName::from_id("end\\").as_bytes(), b"end\\");
    assert_eq!(KeyName::from_id("\\xg1").as_bytes(), b"\\xg1");
    assert_eq!(KeyName::from_id("\\x4").as_bytes(), b"\\x4");
    assert_eq!(KeyName::from_id("\\xé").as_bytes(), "\\xé".as_bytes());
}

#[test]
fn a_prefix_keeps_the_raw_bytes() {
    let name = KeyName::from(b"\xffns:key".to_vec());
    assert_eq!(name.prefix(4).as_bytes(), b"\xffns:");
}
