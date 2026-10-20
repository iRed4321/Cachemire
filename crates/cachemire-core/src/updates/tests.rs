use super::*;
use serde_json::json;

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn release(tag: &str) -> Value {
    json!({
        "tag_name": tag, "draft": false, "html_url": format!("https://github.com/{REPO}/releases/tag/{tag}"),
        "assets": [{"name": "notes.txt", "browser_download_url": "https://x/notes.txt"}, {"name": "Cachemire.msi", "browser_download_url": format!("https://x/{tag}.msi")}],
    })
}

#[test]
fn a_versions_channel_comes_from_its_pre_release_part() {
    assert_eq!(Channel::of(&version("3.0.0")), Channel::Stable);
    assert_eq!(Channel::of(&version("3.1.0-beta.2")), Channel::Beta);
    assert_eq!(Channel::of(&version("0.0.0-dev")), Channel::Dev);
    for channel in [Channel::Dev, Channel::Beta, Channel::Stable] {
        assert_eq!(Channel::from_code(channel.code()), Some(channel));
    }
    assert_eq!(Channel::from_code("nightly"), None);
}

#[test]
fn stable_skips_betas_while_beta_takes_both() {
    let releases = [release("v3.0.0"), release("v3.1.0-beta.1"), release("v3.0.1")];
    let newest = |current: &str, channel| newest_offered(&releases, &version(current), channel).map(|r| r.version.to_string());
    assert_eq!(newest("3.0.0", Channel::Stable).as_deref(), Some("3.0.1"));
    assert_eq!(newest("3.0.0", Channel::Beta).as_deref(), Some("3.1.0-beta.1"));
    assert_eq!(newest("3.1.0-beta.1", Channel::Beta), None, "nothing newer than the running beta");
    assert_eq!(newest("3.0.0", Channel::Dev), None, "a dev build is never offered an update");
}

#[test]
fn a_beta_is_left_for_its_stable_release() {
    let releases = [release("v3.1.0-beta.3"), release("v3.1.0")];
    let newest = newest_offered(&releases, &version("3.1.0-beta.3"), Channel::Beta).unwrap();
    assert_eq!(newest.version, version("3.1.0"));
}

#[test]
fn drafts_and_unreadable_tags_are_ignored_and_the_msi_is_found() {
    let mut draft = release("v9.0.0");
    draft["draft"] = json!(true);
    let releases = [draft, release("latest"), json!({"tag_name": "v4.0.0"}), release("v3.2.0")];
    let newest = newest_offered(&releases, &version("3.0.0"), Channel::Stable).unwrap();
    assert_eq!(newest.version, version("3.2.0"));
    assert_eq!(newest.installer.as_deref(), Some("https://x/v3.2.0.msi"));
    assert!(newest.page.ends_with("/releases/tag/v3.2.0"));
}

#[test]
fn a_release_without_an_installer_is_still_offered() {
    let mut bare = release("v3.3.0");
    bare["assets"] = json!([]);
    let newest = newest_offered(&[bare], &version("3.0.0"), Channel::Stable).unwrap();
    assert_eq!(newest.installer, None);
}

// Hits GitHub: run with `cargo test -p cachemire-core -- --ignored the_releases_feed`.
#[test]
#[ignore = "needs network access to GitHub"]
fn the_releases_feed_leads_to_a_downloadable_installer() {
    let feed = get_text(&format!("https://github.com/{REPO}/releases.atom"), "application/atom+xml").unwrap();
    let newest = newest_offered(&releases_from_feed(&feed), &version("0.0.0"), Channel::Beta).expect("no release in the feed");
    let installer = newest.installer.expect("no installer for the newest release");
    println!("newest release {}: {installer}", newest.version);
    let status = agent().head(&installer).config().http_status_as_error(false).build().call().unwrap().status();
    assert!(status.is_success(), "{installer} answered {status}");
}
