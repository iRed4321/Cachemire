//! The dialog that asks whether to trust an SSH server whose host key is in no known-hosts file.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::HostKeyDialog;
use crate::MainWindow;
use crate::app::App;
use cachemire_core::connection::ssh_tunnel::{TunnelError, UnknownHostKey};
use cachemire_core::error::Error as CoreError;

/// What the user decided.
pub enum Answered {
    /// the key is remembered, so the attempt can go on
    Trusted,
    Cancelled,
    /// the user trusted it but it couldn't be remembered
    NotSaved(CoreError),
}

/// Goes on with what waited for the answer.
pub type Answer = Box<dyn FnOnce(&Rc<App>, &MainWindow, Answered)>;

/// The server `error` says is not known yet, if it does.
pub fn unknown(error: &CoreError) -> Option<&UnknownHostKey> {
    match error {
        CoreError::Tunnel(tunnel) => match &**tunnel {
            TunnelError::UnknownHostKey(server) => Some(server),
            _ => None,
        },
        _ => None,
    }
}

/// Shows `server`'s key and asks; `answer` runs with the decision.
pub fn ask(app: &Rc<App>, window: &MainWindow, server: UnknownHostKey, answer: Answer) {
    let dialog = window.global::<HostKeyDialog>();
    dialog.set_server(format!("{}:{}", server.host, server.port).into());
    dialog.set_key_type(server.key_type.as_str().into());
    dialog.set_fingerprint(server.fingerprint.as_str().into());
    app.asked.borrow_mut().host_key = Some((server, answer));
    dialog.set_open(true);
}

fn answer(decide: impl FnOnce(&App, &UnknownHostKey) -> Answered) {
    crate::app::with(|app, window| {
        let asked = app.asked.borrow_mut().host_key.take();
        if let Some((server, answer)) = asked {
            let answered = decide(app, &server);
            answer(app, window, answered);
        }
    });
}

pub fn init(window: &MainWindow) {
    let dialog = window.global::<HostKeyDialog>();
    dialog.on_trusted(|| {
        answer(|app, server| match app.state.redis_connections.host_keys().trust(server) {
            Ok(()) => Answered::Trusted,
            Err(e) => Answered::NotSaved(e.into()),
        });
    });
    dialog.on_cancelled(|| answer(|_, _| Answered::Cancelled));
}
