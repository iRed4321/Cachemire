//! The dialogs about secrets: what to do when the system keyring can't take a password
//! being saved, and the passwords asked for when connecting to a connection saved without them.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::app::App;
use crate::{KeyringDialog, MainWindow, SecretsPrompt};
use cachemire_core::error::Error as CoreError;
use cachemire_core::secret_vault::{KeyringError, LeftOut, Secrets, SecretsPolicy};

/// Runs a save again, under the policy the user chose.
pub type Retry = Box<dyn FnOnce(&Rc<App>, &MainWindow, SecretsPolicy)>;

/// Goes on with what waited for the passwords.
pub type Resume = Box<dyn FnOnce(&Rc<App>, &MainWindow)>;

/// What the open dialogs will do with the answer.
#[derive(Default)]
pub struct Asked {
    retry: Option<Retry>,
    /// the connection the passwords are for, and what follows
    passwords: Option<(String, Resume)>,
}

/// Whether `error` is the credential store refusing a save, which the user can decide on.
pub fn is_unavailable(error: &CoreError) -> bool {
    matches!(error, CoreError::Keyring(KeyringError::CantStore(_)))
}

/// Asks what to do about the keyring being unavailable; `retry` runs the save again with the
/// answer. `can_leave_out` offers saving without the password, for what asks for it later.
pub fn ask_keyring(app: &Rc<App>, window: &MainWindow, can_leave_out: bool, retry: Retry) {
    app.asked.borrow_mut().retry = Some(retry);
    let dialog = window.global::<KeyringDialog>();
    dialog.set_can_leave_out(can_leave_out);
    dialog.set_open(true);
}

/// Asks for the passwords of connection `id` (`name`) that are `needed`, then runs `resume`.
pub fn ask_secrets(app: &Rc<App>, window: &MainWindow, id: &str, name: &str, needed: LeftOut, resume: Resume) {
    app.asked.borrow_mut().passwords = Some((id.to_string(), resume));
    let prompt = window.global::<SecretsPrompt>();
    prompt.set_name(name.into());
    prompt.set_ask_password(needed.password);
    prompt.set_ask_ssh_password(needed.ssh_password);
    prompt.set_ask_ssh_passphrase(needed.ssh_passphrase);
    prompt.set_password("".into());
    prompt.set_ssh_password("".into());
    prompt.set_ssh_passphrase("".into());
    prompt.set_open(true);
}

fn answer_keyring(policy: SecretsPolicy) {
    crate::app::with(|app, window| {
        let retry = app.asked.borrow_mut().retry.take();
        if let Some(retry) = retry {
            retry(app, window, policy);
        }
    });
}

pub fn init(window: &MainWindow) {
    let dialog = window.global::<KeyringDialog>();
    dialog.on_save_in_file(|| answer_keyring(SecretsPolicy::PlainText));
    dialog.on_save_without(|| answer_keyring(SecretsPolicy::LeaveOut));
    dialog.on_cancelled(|| crate::app::with(|app, _| drop(app.asked.borrow_mut().retry.take())));

    let prompt = window.global::<SecretsPrompt>();
    prompt.on_connect(|| {
        crate::app::with(|app, window| {
            let prompt = window.global::<SecretsPrompt>();
            let given = Secrets {
                password: prompt.get_password().to_string(),
                ssh_password: prompt.get_ssh_password().to_string(),
                ssh_passphrase: prompt.get_ssh_passphrase().to_string(),
            };
            prompt.set_password("".into());
            prompt.set_ssh_password("".into());
            prompt.set_ssh_passphrase("".into());
            let asked = app.asked.borrow_mut().passwords.take();
            if let Some((id, resume)) = asked {
                app.state.connections.lock().provide_secrets(&id, given);
                resume(app, window);
            }
        });
    });
    prompt.on_cancelled(|| {
        crate::app::with(|app, window| {
            let prompt = window.global::<SecretsPrompt>();
            prompt.set_password("".into());
            prompt.set_ssh_password("".into());
            prompt.set_ssh_passphrase("".into());
            drop(app.asked.borrow_mut().passwords.take());
        });
    });
}
