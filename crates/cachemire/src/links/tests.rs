use super::*;

#[test]
fn the_link_launched_with_is_the_first_argument_that_is_one() {
    let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    assert_eq!(from_args(args(&["--flag", "CacheMire://open?key=k", "cachemire://open?key=2"])).as_deref(), Some("CacheMire://open?key=k"));
    assert_eq!(from_args(args(&["cachemire", "https://cachemire.app"])), None);
    assert_eq!(from_args(args(&[])), None);
}

#[test]
fn links_handed_over_arrive_once_in_order_and_stale_ones_are_dropped() {
    let dir = cachemire_core::test_support::temp_dir("links-inbox");
    assert!(receive(&dir.join("missing")).is_empty(), "no folder yet: nothing");
    send(&dir, "cachemire://open?key=first").unwrap();
    send(&dir, "cachemire://open?key=second").unwrap();
    // one still being written isn't taken
    std::fs::write(dir.join("half.partial"), "cachemire://open?key=half").unwrap();
    assert_eq!(receive(&dir), ["cachemire://open?key=first", "cachemire://open?key=second"]);
    assert!(receive(&dir).is_empty(), "each arrives once");

    let stale = dir.join(format!("0-old.{INBOX_EXTENSION}"));
    std::fs::write(&stale, "cachemire://open?key=old").unwrap();
    let long_ago = SystemTime::now() - INBOX_EXPIRY - Duration::from_secs(1);
    std::fs::File::options().write(true).open(&stale).unwrap().set_modified(long_ago).unwrap();
    assert!(receive(&dir).is_empty(), "too old: dropped");
    assert!(!stale.exists());
}
