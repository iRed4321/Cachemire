//! Passwords when the system keyring can't take them: kept in the settings file when the user
//! says so, or left out and asked for each time the connection is opened.

use super::*;
use crate::{ConnectionEditor, KeyringDialog, SecretsPrompt};
use cachemire_core::connection::{ConnectionPayloadInput, store::ConnectionStore};
use cachemire_core::secret_vault::{MemoryVault, SecretsPolicy};
use std::cell::RefCell;
use std::sync::atomic::Ordering;

/// The saved connections, read from their file over a keyring that refuses every password.
fn store_without_keyring(state: &cachemire_core::state::AppState) -> ConnectionStore {
    let vault = MemoryVault::default();
    vault.broken.store(true, Ordering::SeqCst);
    ConnectionStore::with_memory_vault(state.settings.lock().file_path().with_file_name("connections.json"), vault)
}

/// The app connected as usual, over a keyring that refuses every password.
fn without_keyring() -> Running {
    let url = Rc::new(RefCell::new(String::new()));
    let given = url.clone();
    let prepare = move |state: &cachemire_core::state::AppState| {
        let mut store = store_without_keyring(state);
        let payload =
            ConnectionPayloadInput { name: Some("End-to-end".into()), connection_url: Some(given.borrow().clone()), ..Default::default() };
        store.create_with_id("e2e", payload, SecretsPolicy::PlainText).unwrap();
        store.set_active("e2e").unwrap();
        *state.connections.lock() = store;
    };
    Running::launch(
        |redis| *url.borrow_mut() = redis.url.clone(),
        prepare,
        |app, window| crate::connections::connect_to(window, app.state.clone(), &app.rt, "e2e".into(), true),
    )
}

/// An ACL user of the test's database, logging in with `password`: its name.
fn acl_user(r: &mut Running, password: &str) -> String {
    let database = r.redis.url.rsplit('/').next().unwrap_or_default().to_string();
    let name = format!("e2e-{database}-secrets");
    r.redis.run(&["ACL", "SETUSER", &name, "reset", "on", &format!(">{password}"), "~*", "+@all"]);
    name
}

/// Fills the connection form for a new connection and presses Save.
fn save_new(r: &mut Running, name: &str, username: &str, password: &str) {
    let editor = r.h.window.global::<ConnectionEditor>();
    editor.set_ssh_enabled(false);
    editor.set_profile_enabled(false);
    editor.set_id("".into());
    editor.set_name(name.into());
    editor.set_connection_url(r.redis.url.as_str().into());
    editor.set_username(username.into());
    editor.set_password(password.into());
    editor.set_open(true);
    editor.invoke_saved(editor.invoke_form());
    r.until("the keyring's refusal", |h| h.window.global::<KeyringDialog>().get_open());
}

fn saved(r: &Running, name: &str) -> Option<cachemire_core::connection::ConnectionItem> {
    r.app.state.connections.lock().list_state().connections.into_iter().find(|c| c.name == name)
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_password_the_keyring_refuses_can_be_kept_in_the_settings_file() {
    let mut r = without_keyring();
    save_new(&mut r, "Kept", "", "pw");
    assert!(r.h.window.global::<KeyringDialog>().get_can_leave_out(), "a connection can ask for it later");
    assert!(saved(&r, "Kept").is_none(), "nothing is saved before the user answers");

    r.h.window.global::<KeyringDialog>().invoke_save_in_file();
    r.until("the save", |h| !h.window.global::<ConnectionEditor>().get_open());
    assert_eq!(saved(&r, "Kept").unwrap().password.expose(), "pw");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn cancelling_the_keyring_question_saves_nothing() {
    let mut r = without_keyring();
    save_new(&mut r, "Dropped", "", "pw");
    r.h.window.global::<KeyringDialog>().invoke_cancelled();
    r.h.window.global::<KeyringDialog>().invoke_save_in_file();
    r.h.frame();
    assert!(saved(&r, "Dropped").is_none(), "the answer no longer saves anything");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_password_left_out_is_asked_for_when_connecting_and_kept_for_the_session() {
    let mut r = without_keyring();
    let user = acl_user(&mut r, "s3cret");
    save_new(&mut r, "Secured", &user, "s3cret");
    r.h.window.global::<KeyringDialog>().invoke_save_without();
    r.until("the save", |h| !h.window.global::<ConnectionEditor>().get_open());
    let id = saved(&r, "Secured").unwrap().id;
    assert!(!r.app.state.connections.lock().missing_secrets(&id).password, "known until the app is closed");
    // the next start reads the connections back: the password is saved nowhere
    let reread = store_without_keyring(&r.app.state);
    *r.app.state.connections.lock() = reread;
    assert!(r.app.state.connections.lock().missing_secrets(&id).password);

    // asked when picked; a cancel leaves the connection in use as it was
    r.h.window.global::<ConnectionState>().invoke_selected(id.as_str().into());
    r.until("the question", |h| h.window.global::<SecretsPrompt>().get_open());
    let prompt = r.h.window.global::<SecretsPrompt>();
    assert_eq!(prompt.get_name(), "Secured");
    assert!(prompt.get_ask_password() && !prompt.get_ask_ssh_password() && !prompt.get_ask_ssh_passphrase());
    prompt.set_password("typed".into());
    prompt.invoke_cancelled();
    assert_eq!(prompt.get_password(), "", "what was typed is wiped");
    assert_eq!(r.h.window.connection_id(), "e2e");

    r.h.window.global::<ConnectionState>().invoke_selected(id.as_str().into());
    r.until("the question again", |h| h.window.global::<SecretsPrompt>().get_open());
    let prompt = r.h.window.global::<SecretsPrompt>();
    prompt.set_password("s3cret".into());
    prompt.invoke_connect();
    r.until("the connection", |h| h.window.connection_id() == id && h.window.global::<ConnectionState>().get_view().connected);
    assert!(!r.app.state.connections.lock().missing_secrets(&id).password, "known for the rest of the session");
}
