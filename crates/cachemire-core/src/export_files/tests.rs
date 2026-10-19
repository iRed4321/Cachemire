use super::*;
use crate::redis_value_type::RedisValueType;

fn plan(folders: &[(&str, &[&str])], keys: &[&str], group: bool) -> Plan {
    let owned = |list: &[&str]| list.iter().map(|k| k.to_string()).collect::<Vec<_>>();
    Plan { folders: folders.iter().map(|(prefix, keys)| (prefix.to_string(), owned(keys))).collect(), keys: owned(keys), group }
}

/// Each file of the plan, with the keys it holds.
fn files(plan: Plan) -> Vec<(String, Vec<String>)> {
    outputs(plan)
        .into_iter()
        .map(|output| match output {
            Output::Items { file, keys, .. } => (file, keys),
            Output::Single { file, key } => (file, vec![key]),
        })
        .collect()
}

fn file_names(plan: Plan) -> Vec<String> {
    files(plan).into_iter().map(|(file, _)| file).collect()
}

#[test]
fn a_folder_is_one_file_and_each_lone_key_its_own() {
    let plan = plan(&[("user:", &["user:1", "user:2"])], &["config", "cache:hits"], false);
    assert_eq!(plan.total(), 4);
    assert_eq!(
        files(plan),
        [
            ("redis-folder-user_.json".to_string(), vec!["user:1".to_string(), "user:2".to_string()]),
            ("redis-key-config.json".to_string(), vec!["config".to_string()]),
            ("redis-key-cache_hits.json".to_string(), vec!["cache:hits".to_string()]),
        ]
    );
}

#[test]
fn grouping_puts_lone_keys_together_by_parent_folder() {
    let grouped = files(plan(&[], &["a:1", "b:x:2", "a:3", "root"], true));
    assert_eq!(
        grouped,
        [
            ("redis-folder-root-keys.json".to_string(), vec!["root".to_string()]),
            ("redis-folder-a_.json".to_string(), vec!["a:1".to_string(), "a:3".to_string()]),
            ("redis-folder-b_x_.json".to_string(), vec!["b:x:2".to_string()]),
        ]
    );
    assert_eq!(file_names(plan(&[], &["a:1"], true)), ["redis-key-a_1.json"], "a single key is never grouped");
}

#[test]
fn no_two_files_of_an_export_share_a_name() {
    // `a:b` and `a_b` make the same stem, and `User` and `user` the same file on Windows
    let names = file_names(plan(&[("", &["x"])], &["a:b", "a_b", "a?b", "User", "user"], false));
    assert_eq!(
        names,
        [
            "redis-folder-root.json",
            "redis-key-a_b.json",
            "redis-key-a_b-2.json",
            "redis-key-a_b-3.json",
            "redis-key-User.json",
            "redis-key-user-2.json"
        ]
    );
}

#[test]
fn file_names_hold_no_character_a_file_system_refuses() {
    assert_eq!(file_safe(r#"a<b>c:d"e/f\g|h?i*j"#), "a_b_c_d_e_f_g_h_i_j");
    assert_eq!(file_safe("tab\there\nnew"), "tab_here_new");
    assert_eq!(file_safe("::"), "_", "a run of bad characters is one _");
    assert_eq!(file_safe("   "), "export");
    assert_eq!(file_safe(&"é".repeat(300)).chars().count(), 120);
}

#[test]
fn an_item_holds_the_keys_real_name_and_says_when_it_was_cut() {
    let read = ExportedKey { key_type: RedisValueType::Hash, ttl: -1, value: json!({"f": "v"}), truncated: false };
    assert_eq!(item("user:1", &read), json!({"key": "user:1", "type": "hash", "ttl": -1, "value": {"f": "v"}}));
    let cut = ExportedKey { key_type: RedisValueType::List, ttl: 30, value: json!([]), truncated: true };
    let item = item("back\\\\slash", &cut);
    assert_eq!(item["key"], "back\\slash", "the id's escape is undone");
    assert_eq!(item["truncated"], true);
}
