use super::*;
use serde::Deserialize;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Sample {
    name: String,
}

fn temp_dir(name: &str) -> PathBuf {
    crate::test_support::temp_dir(&format!("persist-{name}"))
}

#[test]
fn a_saved_file_reads_back_and_leaves_nothing_else_behind() {
    let dir = temp_dir("save");
    let file = dir.join("data.json");
    assert!(matches!(load::<Sample>(&file), Loaded::Missing));

    save(&file, &Sample { name: "one".into() }).unwrap();
    save(&file, &Sample { name: "two".into() }).unwrap();
    assert!(matches!(load::<Sample>(&file), Loaded::Read(Sample { name }) if name == "two"));
    let files: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(files, vec!["data.json".to_string()], "the temporary file is gone");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_cannot_be_used_is_kept_aside() {
    let dir = temp_dir("aside");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("data.json");
    fs::write(&file, "{ not json").unwrap();
    assert!(matches!(load::<Sample>(&file), Loaded::Unusable(_)));
    fs::write(&file, r#"{"other": 1}"#).unwrap();
    assert!(matches!(load::<Sample>(&file), Loaded::Unusable(_)), "the wrong shape is unusable too");

    let kept = set_aside(&file).expect("moved");
    assert!(!file.exists());
    assert_eq!(fs::read_to_string(kept).unwrap(), r#"{"other": 1}"#);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_save_that_fails_leaves_the_saved_file_as_it_was() {
    let dir = temp_dir("failed-save");
    let file = dir.join("settings.json");
    save(&file, &serde_json::json!({"kept": true})).unwrap();
    // a map with keys JSON can't have: it fails to serialize
    let unwritable: std::collections::BTreeMap<(i32, i32), i32> = [((1, 2), 3)].into_iter().collect();
    assert!(save(&file, &unwritable).is_err());
    assert!(matches!(load::<serde_json::Value>(&file), Loaded::Read(v) if v == serde_json::json!({"kept": true})));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temporary file is left behind");
}

#[test]
fn a_save_replaces_the_whole_file() {
    let file = temp_dir("replace").join("settings.json");
    save(&file, &serde_json::json!({"long": "x".repeat(1000)})).unwrap();
    save(&file, &serde_json::json!({"short": 1})).unwrap();
    assert!(matches!(load::<serde_json::Value>(&file), Loaded::Read(v) if v == serde_json::json!({"short": 1})));
}

#[test]
fn a_file_of_another_shape_is_unusable_not_missing() {
    let file = temp_dir("shape").join("history.json");
    fs::write(&file, r#"{"not": "a list"}"#).unwrap();
    assert!(matches!(load::<Vec<String>>(&file), Loaded::Unusable(_)));
    assert!(matches!(load::<Vec<String>>(&file.with_file_name("absent.json")), Loaded::Missing));
}
