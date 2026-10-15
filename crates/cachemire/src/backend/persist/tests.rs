use super::*;
use serde::Deserialize;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Sample {
    name: String,
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("cachemire-persist-{name}-{}-{}", std::process::id(), super::super::now_ms()))
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
