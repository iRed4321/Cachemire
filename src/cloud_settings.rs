//! The cloud settings form: what is set for an AWS account (all its endpoints) or for one
//! endpoint, which overrides the account's. Opens it with the stored values and saves what it
//! says through `backend::settings_store`; the endpoints themselves are never saved.

use std::cell::RefCell;
use std::rc::Rc;

use rust_i18n::t;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::App;
use crate::backend::LockExt;
use crate::backend::secret_vault::staged;
use crate::backend::settings_store::{CloudScope, CloudSsh};
use crate::backend::state::AppState;
use crate::{AwsAccount, CloudSettings, ConnProfileRowData, MainWindow};

/// What the form is open on.
#[derive(Clone)]
struct Open {
    scope: CloudScope,
    label: String,
}

/// The name of a connection profile, or "none".
fn profile_name(state: &AppState, id: Option<&str>) -> String {
    id.filter(|id| !id.is_empty()).and_then(|id| state.settings.lock_recover().connection_profile(id)).map_or_else(|| t!("none").into_owned(), |p| p.name)
}

/// Gives the endpoints the settings they now have, and the open pools the tunnel
/// they now need.
fn reapply(app: &App, window: &MainWindow) {
    let state = &app.state;
    let (account, region, mut items) = {
        let store = state.connections.lock_recover();
        let (account, region) = store.cloud_account();
        (account.to_string(), region.to_string(), store.cloud_connections().to_vec())
    };
    state.settings.lock_recover().apply_cloud(&account, &region, &mut items);
    for item in &items {
        state.redis_connections.invalidate(&item.id);
    }
    state.connections.lock_recover().set_cloud_connections(&account, &region, items);
    crate::connections::refresh(app, window);
    crate::connection_profiles::apply_tint(window, state);
}

pub fn init(app: &Rc<App>, window: &MainWindow) {
    let (state, rt) = (app.state.clone(), app.rt.clone());
    let open: Rc<RefCell<Option<Open>>> = Rc::new(RefCell::new(None));
    let global = window.global::<CloudSettings>();

    global.on_open_requested({
        let (weak, state, open) = (window.as_weak(), state.clone(), open.clone());
        move |kind, id| {
            let Some(window) = weak.upgrade() else { return };
            let (scope, name, detail) = if kind == 0 {
                let account = state.connections.lock_recover().cloud_account().0.to_string();
                if account.is_empty() {
                    return;
                }
                (CloudScope::Account(account.clone()), window.global::<AwsAccount>().get_profile().to_string(), account)
            } else {
                let Some(key) = state.connections.lock_recover().cloud_key(&id) else { return };
                let (name, detail) = (key.name.clone(), key.region.clone());
                (CloudScope::Endpoint(key), name, detail)
            };
            let endpoint = matches!(scope, CloudScope::Endpoint(_));
            let (entry, account_entry) = {
                let settings = state.settings.lock_recover();
                let account_entry = match &scope {
                    CloudScope::Endpoint(key) => Some(settings.cloud_settings(&CloudScope::Account(key.account.clone()))),
                    CloudScope::Account(_) => None,
                };
                (settings.cloud_settings(&scope), account_entry)
            };

            let global = window.global::<CloudSettings>();
            let rows: Vec<ConnProfileRowData> = state
                .settings
                .lock_recover()
                .connection_profiles()
                .into_iter()
                .map(|p| ConnProfileRowData { id: p.id.into(), name: p.name.into(), tint: crate::connection_profiles::to_tint(p.color) })
                .collect();
            global.set_profiles(ModelRc::new(VecModel::from(rows)));
            global.set_scope_kind(kind);
            global.set_scope_name(name.as_str().into());
            global.set_scope_detail(detail.into());
            global.set_error("".into());
            global.set_inherit_profile_hint(profile_name(&state, account_entry.as_ref().and_then(|a| a.connection_profile.as_deref())).into());
            global.set_inherit_ssh_hint(
                account_entry
                    .as_ref()
                    .and_then(|a| a.ssh.as_ref())
                    .filter(|ssh| ssh.enabled)
                    .map_or_else(|| t!("none").into_owned(), |ssh| format!("{}:{}", ssh.host, ssh.port))
                    .into(),
            );

            let default_mode = if endpoint { 0 } else { 1 };
            let (profile_mode, profile_id) = match entry.connection_profile.as_deref() {
                None => (default_mode, ""),
                Some("") => (1, ""),
                Some(id) => (2, id),
            };
            global.set_profile_mode(profile_mode);
            global.set_profile_id(profile_id.into());
            let ssh = entry.ssh.as_ref().filter(|ssh| ssh.enabled);
            global.set_ssh_mode(match &entry.ssh {
                None => default_mode,
                Some(ssh) if !ssh.enabled => 1,
                Some(_) => 2,
            });
            global.set_ssh_host(ssh.map_or("", |s| s.host.as_str()).into());
            global.set_ssh_port(ssh.map_or(22, |s| s.port).to_string().into());
            global.set_ssh_username(ssh.map_or("", |s| s.username.as_str()).into());
            global.set_ssh_key(ssh.map_or("", |s| s.private_key.as_str()).into());
            global.set_ssh_password("".into());
            global.set_ssh_passphrase("".into());

            *open.borrow_mut() = Some(Open { scope, label: name });
            global.set_open(true);
        }
    });

    global.on_save({
        let (weak, state, rt, open) = (window.as_weak(), state.clone(), rt.clone(), open.clone());
        move || {
            let Some(window) = weak.upgrade() else { return };
            let global = window.global::<CloudSettings>();
            let Some(Open { scope, label }) = open.borrow().clone() else { return };
            let endpoint = matches!(scope, CloudScope::Endpoint(_));
            let fail = |message: String| global.set_error(message.into());

            // an endpoint can leave a setting to its account (0) or say none (1); an account has no parent
            let profile = match global.get_profile_mode() {
                0 if endpoint => None,
                2 => match global.get_profile_id().to_string() {
                    id if id.is_empty() => return fail(t!("Pick a connection profile.").into_owned()),
                    id => Some(id),
                },
                _ => endpoint.then(String::new),
            };
            let ssh = match global.get_ssh_mode() {
                0 if endpoint => None,
                2 => {
                    let port = match global.get_ssh_port().trim() {
                        "" => Some(22),
                        text => text.parse::<u16>().ok().filter(|port| *port > 0),
                    };
                    let Some(port) = port else { return fail(t!("SSH port must be a number between 1 and 65535.").into_owned()) };
                    Some(CloudSsh {
                        enabled: true,
                        host: global.get_ssh_host().trim().to_string(),
                        port,
                        username: global.get_ssh_username().trim().to_string(),
                        private_key: global.get_ssh_key().trim().to_string(),
                        password: global.get_ssh_password().to_string(),
                        passphrase: global.get_ssh_passphrase().to_string(),
                        in_vault: false,
                    })
                }
                _ => endpoint.then(CloudSsh::none),
            };

            // off the UI thread: the secrets go through the OS credential store
            let state = state.clone();
            rt.spawn_blocking(move || {
                let saved = staged(&state.settings, |settings| settings.set_cloud_scope(&scope, &label, profile.clone(), ssh.clone()));
                let result = saved.map(|cleanup| {
                    if let Some(cleanup) = cleanup {
                        cleanup.run();
                    }
                });
                let _ = slint::invoke_from_event_loop(move || crate::app::with(|app, window| {
                    let global = window.global::<CloudSettings>();
                    match result {
                        Ok(()) => {
                            reapply(app, window);
                            global.set_open(false);
                        }
                        Err(e) => global.set_error(e.into()),
                    }
                }));
            });
        }
    });

    // "Browse…" beside the private key
    global.on_browse_key({
        let (weak, rt) = (window.as_weak(), rt);
        move || crate::connections::browse_ssh_key(&rt, weak.clone(), |window, path| window.global::<CloudSettings>().set_ssh_key(path.into()))
    });
}
