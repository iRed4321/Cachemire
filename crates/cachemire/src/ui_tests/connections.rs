//! The connection form: saving a new connection or changes to one, and what it refuses; the
//! connection profiles, their tint and read-only setting.

use super::*;
use crate::{ConnectionEditor, ConnectionState};
use cachemire_core::connection::ConnectionPayloadInput;

/// The app with its connection form wired, the "Browse…" button kept from opening a dialog.
fn form_ready() -> (Harness, Rc<crate::app::App>, (tokio::runtime::Runtime, EventLoopTurn)) {
    let state = test_state();
    let h = Harness::new();
    let (app, runtime, turn) = install_app(&h, &state);
    crate::connections::init(&app, &h.window);
    h.window.global::<ConnectionEditor>().on_browse_key(|| {});
    (h, app, (runtime, turn))
}

/// Opens the form holding these, with no tunnel and no profile.
fn fill(h: &Harness, id: &str, name: &str, url: &str, password: &str) {
    let editor = h.window.global::<ConnectionEditor>();
    editor.set_ssh_enabled(false);
    editor.set_profile_enabled(false);
    editor.set_id(id.into());
    editor.set_name(name.into());
    editor.set_connection_url(url.into());
    editor.set_password(password.into());
    editor.set_open(true);
}

fn saved_names(app: &crate::app::App) -> Vec<String> {
    app.state.connections.lock().list_state().connections.into_iter().map(|c| c.name).collect()
}

/// Clicks over the form until its Save button has been pressed and the save has landed (`done`).
/// After a miss, the form is put back as `fill` left it: a click may have ticked the tunnel,
/// which moves the buttons and makes the save fail.
fn press_save(h: &mut Harness, form: (&str, &str, &str, &str), done: impl Fn() -> bool) {
    let refill = |h: &Harness| fill(h, form.0, form.1, form.2, form.3);
    let saved = |h: &Harness| {
        // the save runs on another thread: give it a moment
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _ = h;
        done()
    };
    // the form's bottom-right corner, where its buttons are: each miss waits a little
    // (a Windows sleep lasts a timer tick, ~16 ms), so the fewer points the better
    sweep(h, 640..870, 480..740, 8, refill, saved).expect("the Save button");
}

#[test]
fn saving_the_form_adds_the_connection_closes_the_form_and_lists_it() {
    let (mut h, app, _rt) = form_ready();
    let before = saved_names(&app).len();
    let form = ("", "Orders cache", "redis://orders.example:6380/2", "s3cret");
    fill(&h, form.0, form.1, form.2, form.3);
    h.frame();
    press_save(&mut h, form, || saved_names(&app).len() != before);

    assert!(saved_names(&app).contains(&"Orders cache".to_string()));
    assert!(wait_for(&mut h, |h| !h.window.global::<ConnectionEditor>().get_open()), "the form closes once saved");
    let listed = h.window.global::<ConnectionState>().get_connections();
    let row = (0..listed.row_count()).filter_map(|i| listed.row_data(i)).find(|r| r.name == "Orders cache").expect("the list shows it");
    assert_eq!(row.connection_url, "redis://orders.example:6380/2");
    let saved = app.state.connections.lock().list_state().connections.into_iter().find(|c| c.name == "Orders cache").unwrap();
    assert_eq!(saved.password.expose(), "s3cret");
}

#[test]
fn editing_a_connection_with_a_blank_password_keeps_the_stored_one() {
    let (mut h, app, _rt) = form_ready();
    let payload = cachemire_core::connection::ConnectionPayloadInput {
        name: Some("Prod".into()),
        connection_url: Some("redis://prod:6379".into()),
        password: Some("kept".into()),
        ..Default::default()
    };
    app.state.connections.lock().create_with_id("prod", payload, cachemire_core::secret_vault::SecretsPolicy::PlainText).unwrap();
    let form = ("prod", "Prod (renamed)", "redis://prod:6390", "");
    fill(&h, form.0, form.1, form.2, form.3);
    h.frame();
    press_save(&mut h, form, || app.state.connections.lock().get("prod").is_some_and(|c| c.name == "Prod (renamed)"));
    let prod = app.state.connections.lock().get("prod").unwrap();
    assert_eq!((prod.connection_url.as_str(), prod.password.expose()), ("redis://prod:6390", "kept"));
}

#[test]
fn a_form_that_cant_be_saved_says_why_and_stays_open() {
    let (mut h, app, _rt) = form_ready();
    let before = saved_names(&app);
    // no name: the store refuses it, and the answer comes back from another thread
    fill(&h, "", "", "redis://h:6379", "");
    h.window.global::<ConnectionEditor>().invoke_saved(h.window.global::<ConnectionEditor>().invoke_form());
    assert!(wait_for(&mut h, |h| !h.window.global::<ConnectionEditor>().get_error().is_empty()), "the reason is shown");
    assert!(h.window.global::<ConnectionEditor>().get_open());
    assert_eq!(saved_names(&app), before);

    // a bad SSH port is refused before anything is saved
    let editor = h.window.global::<ConnectionEditor>();
    editor.set_error("".into());
    fill(&h, "", "Tunnel", "redis://h:6379", "");
    editor.set_ssh_enabled(true);
    editor.set_ssh_host("bastion".into());
    editor.set_ssh_port("ssh".into());
    editor.invoke_saved(editor.invoke_form());
    assert!(editor.get_error().contains("SSH port"), "{}", editor.get_error());
    assert_eq!(saved_names(&app), before);
}

/// The app with the connection profiles wired, and connection `c` (no profile yet) in use.
fn profiles_ready() -> (Harness, Rc<crate::app::App>, (tokio::runtime::Runtime, EventLoopTurn)) {
    let (h, app, rt) = form_ready();
    crate::connections::profiles::init(&app, &h.window);
    let payload =
        ConnectionPayloadInput { name: Some("Cache".into()), connection_url: Some("redis://c:6379".into()), ..Default::default() };
    app.state.connections.lock().create_with_id("c", payload, cachemire_core::secret_vault::SecretsPolicy::PlainText).unwrap();
    let view = crate::ConnectionView { connected: true, connection_id: "c".into(), ..Default::default() };
    h.window.global::<ConnectionState>().set_view(view);
    (h, app, rt)
}

fn save_profile(h: &Harness, id: &str, name: &str, tint: crate::ProfileTint, read_only: bool) {
    let form = crate::ConnProfileForm { id: id.into(), name: name.into(), tint, read_only, light_tint: false };
    h.window.global::<crate::Preferences>().invoke_conn_profile_saved(form);
}

/// The id of the profile listed as `name`.
fn profile_id(h: &Harness, name: &str) -> Option<String> {
    h.window.global::<crate::Preferences>().get_conn_profiles().iter().find(|p| p.name == name).map(|p| p.id.to_string())
}

/// Gives connection `c` the profile `id`, as its form would.
fn use_profile(app: &crate::app::App, id: &str) {
    let payload = ConnectionPayloadInput { profile_id: Some(id.to_string()), ..Default::default() };
    app.state.connections.lock().update("c", payload, cachemire_core::secret_vault::SecretsPolicy::PlainText).unwrap();
}

#[test]
fn the_profile_of_the_connection_in_use_tints_the_window_and_can_make_it_read_only() {
    let (h, app, _rt) = profiles_ready();
    let preferences = h.window.global::<crate::Preferences>();
    preferences.set_conn_profile_editing(true);
    save_profile(&h, "", "Staging", crate::ProfileTint::Orange, false);
    assert!(!preferences.get_conn_profile_editing(), "the form closes");
    let id = profile_id(&h, "Staging").expect("listed in the settings");
    assert!(!h.window.global::<crate::Theme>().get_env_tinted(), "no connection uses it yet");

    use_profile(&app, &id);
    save_profile(&h, &id, "Staging", crate::ProfileTint::Blue, true);
    let theme = h.window.global::<crate::Theme>();
    assert!(theme.get_env_tinted());
    assert_eq!(theme.get_env_tint(), crate::ProfileTint::Blue, "recolored at once");
    assert!(h.window.global::<ConnectionState>().get_read_only());
    assert!(!h.window.global::<KeyPanel>().get_editable(), "nothing can be written through it");
    assert!(h.window.global::<ConnectionEditor>().invoke_profile_read_only(id.as_str().into()), "the form's box follows the profile");
}

#[test]
fn a_profile_still_used_by_a_connection_is_not_deleted_and_says_by_which() {
    let (h, app, _rt) = profiles_ready();
    save_profile(&h, "", "Staging", crate::ProfileTint::Green, false);
    let id = profile_id(&h, "Staging").unwrap();
    use_profile(&app, &id);
    let preferences = h.window.global::<crate::Preferences>();
    preferences.invoke_conn_profile_deleted(id.as_str().into());
    assert!(preferences.get_error().contains("Cache"), "{}", preferences.get_error());
    assert!(profile_id(&h, "Staging").is_some());

    use_profile(&app, "");
    preferences.set_conn_profile_editing(true);
    preferences.invoke_conn_profile_deleted(id.as_str().into());
    assert_eq!(preferences.get_error(), "");
    assert!(profile_id(&h, "Staging").is_none());
    assert!(!preferences.get_conn_profile_editing());
}

#[test]
fn a_profile_without_a_name_is_refused_with_the_reason() {
    let (h, _app, _rt) = profiles_ready();
    let before = h.window.global::<crate::Preferences>().get_conn_profiles().row_count();
    save_profile(&h, "", "  ", crate::ProfileTint::Red, false);
    assert!(!h.window.global::<crate::Preferences>().get_error().is_empty());
    assert_eq!(h.window.global::<crate::Preferences>().get_conn_profiles().row_count(), before);
}

#[test]
fn the_title_bars_copy_button_copies_the_connection_string_in_use() {
    let (mut h, app, (rt, _turn)) = form_ready();
    let payload = ConnectionPayloadInput {
        name: Some("Copied".into()),
        connection_url: Some("redis://marker-conn.test:6390/2".into()),
        password: Some("pw".into()),
        ..Default::default()
    };
    app.state.connections.lock().create_with_id("cp", payload, cachemire_core::secret_vault::SecretsPolicy::PlainText).unwrap();
    h.window.global::<ConnectionState>().invoke_copy_connection_string();
    assert!(!crate::value_actions::HANDED.lock().iter().any(|(_, text)| text.contains("marker-conn")), "nothing in use: nothing copied");

    let view = crate::ConnectionView { connected: true, connection_id: "cp".into(), ..Default::default() };
    h.window.global::<ConnectionState>().set_view(view);
    h.window.global::<ConnectionState>().invoke_copy_connection_string();
    let (target, text) = handed_out(&mut h, &rt, "marker-conn");
    assert_eq!(target, "clipboard");
    assert!(text.contains("marker-conn.test:6390") && text.contains("pw"), "the login goes with it: {text}");
}
