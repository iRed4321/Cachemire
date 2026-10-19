//! The connection form: saving a new connection or changes to one, and what it refuses.

use super::*;
use cachemire_core::LockExt;
use crate::{ConnectionEditor, ConnectionState};

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
    app.state.connections.lock_recover().list_state().connections.into_iter().map(|c| c.name).collect()
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
    sweep(h, 430..870, 200..740, 8, refill, saved).expect("the Save button");
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
    let saved = app.state.connections.lock_recover().list_state().connections.into_iter().find(|c| c.name == "Orders cache").unwrap();
    assert_eq!(saved.password, "s3cret");
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
    app.state.connections.lock_recover().create_with_id("prod", payload).unwrap();
    let form = ("prod", "Prod (renamed)", "redis://prod:6390", "");
    fill(&h, form.0, form.1, form.2, form.3);
    h.frame();
    press_save(&mut h, form, || app.state.connections.lock_recover().get("prod").is_some_and(|c| c.name == "Prod (renamed)"));
    let prod = app.state.connections.lock_recover().get("prod").unwrap();
    assert_eq!((prod.connection_url.as_str(), prod.password.as_str()), ("redis://prod:6390", "kept"));
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

