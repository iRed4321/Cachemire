//! The cloud settings form: what is set for an AWS account (all its endpoints) or for one
//! endpoint, which overrides the account's. Opens it with the stored values and saves what it
//! says through `cachemire_core::settings`; the endpoints themselves are never saved.

use std::cell::RefCell;
use std::rc::Rc;

use rust_i18n::t;
use slint::{ComponentHandle, ModelRc, VecModel};


use crate::app::App;
use cachemire_core::LockExt;
use cachemire_core::secret_vault::staged;
use cachemire_core::settings::{CloudScope, CloudSsh};
use cachemire_core::state::AppState;
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
    crate::connections::profiles::apply_tint(window, state);
}

pub fn init(window: &MainWindow) {
    let open: Rc<RefCell<Option<Open>>> = Rc::default();
    let global = window.global::<CloudSettings>();
    global.on_open_requested({
        let open = open.clone();
        move |kind, id| {
            crate::app::with(|app, window| {
                if let Some(opened) = open_form(&app.state, window, kind, &id) {
                    *open.borrow_mut() = Some(opened);
                    window.global::<CloudSettings>().set_open(true);
                }
            });
        }
    });
    global.on_save(move || {
        let Some(open) = open.borrow().clone() else { return };
        crate::app::with(|app, window| save(app, window, open));
    });
    // "Browse…" beside the private key
    global.on_browse_key(|| {
        crate::app::with(|app, window| crate::connections::browse_ssh_key(&app.rt, window.as_weak(), |window, path| window.global::<CloudSettings>().set_ssh_key(path.into())));
    });
}

/// What the form opens on for `kind` (0: the signed-in account; else the endpoint
/// `id`), with the name and detail it shows; `None` when there is no such thing.
fn scope_of(state: &AppState, window: &MainWindow, kind: i32, id: &str) -> Option<(CloudScope, String, String)> {
    if kind == 0 {
        let account = state.connections.lock_recover().cloud_account().0.to_string();
        if account.is_empty() {
            return None;
        }
        Some((CloudScope::Account(account.clone()), window.global::<AwsAccount>().get_profile().to_string(), account))
    } else {
        let key = state.connections.lock_recover().cloud_key(id)?;
        let (name, detail) = (key.name.clone(), key.region.clone());
        Some((CloudScope::Endpoint(key), name, detail))
    }
}

/// Fills the form with what is stored for `kind`/`id` (see [`scope_of`]); what it is open on.
fn open_form(state: &AppState, window: &MainWindow, kind: i32, id: &str) -> Option<Open> {
    let (scope, name, detail) = scope_of(state, window, kind, id)?;
    let endpoint = matches!(scope, CloudScope::Endpoint(_));
    let (entry, account_entry, profiles) = {
        let settings = state.settings.lock_recover();
        let account_entry = match &scope {
            CloudScope::Endpoint(key) => Some(settings.cloud_settings(&CloudScope::Account(key.account.clone()))),
            CloudScope::Account(_) => None,
        };
        (settings.cloud_settings(&scope), account_entry, settings.connection_profiles())
    };

    let global = window.global::<CloudSettings>();
    let rows: Vec<ConnProfileRowData> = profiles
        .into_iter()
        .map(|p| ConnProfileRowData { id: p.id.into(), name: p.name.into(), tint: crate::connections::profiles::to_tint(p.color) })
        .collect();
    global.set_profiles(ModelRc::new(VecModel::from(rows)));
    global.set_scope_kind(kind);
    global.set_scope_name(name.as_str().into());
    global.set_scope_detail(detail.into());
    global.set_error("".into());
    global.set_inherit_profile_hint(profile_name(state, account_entry.as_ref().and_then(|a| a.connection_profile.as_deref())).into());
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
    Some(Open { scope, label: name })
}

/// The connection profile and SSH tunnel the form sets (`None`: left to the account),
/// or why they can't be saved.
fn form_values(global: &CloudSettings, endpoint: bool) -> Result<(Option<String>, Option<CloudSsh>), String> {
    // an endpoint can leave a setting to its account (0) or say none (1); an account has no parent
    let profile = match global.get_profile_mode() {
        0 if endpoint => None,
        2 => match global.get_profile_id().to_string() {
            id if id.is_empty() => return Err(t!("Pick a connection profile.").into_owned()),
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
            let Some(port) = port else { return Err(t!("SSH port must be a number between 1 and 65535.").into_owned()) };
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
    Ok((profile, ssh))
}

/// Saves what the form says for what it is open on, then closes it.
fn save(app: &Rc<App>, window: &MainWindow, Open { scope, label }: Open) {
    let global = window.global::<CloudSettings>();
    let (profile, ssh) = match form_values(&global, matches!(scope, CloudScope::Endpoint(_))) {
        Ok(values) => values,
        Err(message) => return global.set_error(message.into()),
    };
    // off the UI thread: the secrets go through the OS credential store
    let state = app.state.clone();
    app.rt.spawn_blocking(move || {
        let saved = staged(&state.settings, |settings| settings.set_cloud_scope(&scope, &label, profile.clone(), ssh.clone()));
        let result = saved.map(|cleanup| {
            if let Some(cleanup) = cleanup {
                cleanup.run();
            }
        });
        crate::app::later(move |app, window| {
            let global = window.global::<CloudSettings>();
            match result {
                Ok(()) => {
                    reapply(app, window);
                    global.set_open(false);
                }
                Err(e) => global.set_error(e.message().into()),
            }
        });
    });
}
