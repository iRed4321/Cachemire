//! The value editor on a real server: a hash field's value written, renamed and deleted,
//! and a string replaced with its TTL kept.

use super::*;

fn profile(redis: &mut TestRedis) {
    redis.run(&["HSET", "user:1", "name", "Ada", "profile", r#"{"age":36,"langs":["en"]}"#, "city", "London"]);
    redis.run(&["SET", "greeting", "hi", "EX", "1000"]);
}

/// Opens the editor on field `name` of the key shown, as its row's edit button does.
fn edit(r: &mut Running, name: &str) -> String {
    r.h.window.global::<KeyPanel>().invoke_edit_value(name.into());
    let editor = r.h.window.global::<ValueEditor>();
    assert!(editor.get_open(), "the editor opens on {name}");
    editor.get_text().to_string()
}

fn save(r: &mut Running, text: &str, name: &str) {
    r.h.window.global::<ValueEditor>().invoke_save(text.into(), name.into());
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_json_field_is_written_back_on_one_line_and_a_text_one_as_text() {
    let mut r = Running::start(profile);
    r.click_row("user:");
    r.open_key("user:1");
    assert_eq!(r.fields(), ["city", "name", "profile"]);

    let shown = edit(&mut r, "profile");
    assert!(shown.contains('\n'), "the JSON is laid out to be edited: {shown}");
    save(&mut r, "{\n  \"age\": 37,\n  \"langs\": [\"en\", \"fr\"]\n}", "profile");
    r.editor_closes();
    assert_eq!(r.redis.query::<String>(&["HGET", "user:1", "profile"]), r#"{"age":37,"langs":["en","fr"]}"#, "as it was: on one line");

    assert_eq!(edit(&mut r, "name"), "\"Ada\"", "text is shown as a JSON string");
    save(&mut r, "\"Ada Lovelace\"", "name");
    r.editor_closes();
    assert_eq!(r.redis.query::<String>(&["HGET", "user:1", "name"]), "Ada Lovelace", "and written back as its text");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_field_renamed_in_the_editor_moves_unless_the_name_is_taken_and_can_be_deleted() {
    let mut r = Running::start(profile);
    r.click_row("user:");
    r.open_key("user:1");

    edit(&mut r, "city");
    let editor = r.h.window.global::<ValueEditor>();
    assert_eq!(editor.invoke_check_name("name".into()), "This hash already has a field with this name");
    assert_eq!(editor.invoke_check_name("town".into()), "");
    r.redis.run(&["HSET", "user:1", "town", "written meanwhile"]);
    save(&mut r, "\"Paris\"", "town");
    r.until("the refusal", |h| !h.window.global::<ValueEditor>().get_error().is_empty());
    assert_eq!(r.h.window.global::<ValueEditor>().get_error(), "This hash already has a field with this name");
    assert_eq!(r.redis.query::<String>(&["HGET", "user:1", "city"]), "London", "nothing changed");

    r.redis.run(&["HDEL", "user:1", "town"]);
    save(&mut r, "\"Paris\"", "town");
    r.editor_closes();
    assert_eq!(r.redis.query::<Option<String>>(&["HGET", "user:1", "city"]), None);
    assert_eq!(r.redis.query::<String>(&["HGET", "user:1", "town"]), "Paris");
    r.until("the key read again", |h| h.window.global::<KeyPanel>().get_all_values_rows().iter().any(|row| row.field == "town"));
    assert_eq!(r.fields(), ["name", "profile", "town"]);

    edit(&mut r, "town");
    r.h.window.global::<ValueEditor>().invoke_delete();
    r.editor_closes();
    assert_eq!(r.redis.query::<i64>(&["HEXISTS", "user:1", "town"]), 0);
    r.until("the key read again", |h| h.window.global::<KeyPanel>().get_all_values_rows().row_count() == 2);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_string_is_replaced_its_ttl_kept_and_an_unchanged_save_writes_nothing() {
    let mut r = Running::start(profile);
    r.open_key("greeting");
    let field = r.fields().into_iter().next().expect("the string's row");

    assert_eq!(edit(&mut r, &field), "\"hi\"");
    r.redis.run(&["SET", "greeting", "changed elsewhere", "KEEPTTL"]);
    save(&mut r, "\"hi\"", &field);
    assert!(!r.h.window.global::<ValueEditor>().get_open(), "nothing to save closes it at once");
    assert_eq!(r.redis.query::<String>(&["GET", "greeting"]), "changed elsewhere", "nothing was written");

    edit(&mut r, &field);
    save(&mut r, "\"hello\"", &field);
    r.until("the editor to say the value changed", |h| !h.window.global::<ValueEditor>().get_conflict().is_empty());
    assert_eq!(r.redis.query::<String>(&["GET", "greeting"]), "changed elsewhere", "nothing was written");
    r.h.window.global::<ValueEditor>().invoke_overwrite("\"hello\"".into(), field.as_str().into());
    r.editor_closes();
    assert_eq!(r.redis.query::<String>(&["GET", "greeting"]), "hello");
    let ttl: i64 = r.redis.query(&["TTL", "greeting"]);
    assert!(ttl > 900, "the TTL is kept ({ttl})");
}
