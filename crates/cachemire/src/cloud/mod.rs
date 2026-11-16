//! The title bar's cloud account (AWS): its pill signs in, then lists the local profiles
//! (with their account's settings) to pick one or sign out, and the picked profile's
//! ElastiCache connections show in the connection dropdown (see `cachemire_core::aws`).

use crate::error::Localize;
use rustc_hash::FxHashSet;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::UserAttentionType;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::App;
use crate::{AwsAccount, AwsProfileRow, MainWindow};
use cachemire_core::aws;
use cachemire_core::settings::CloudScope;
use rust_i18n::t;

pub mod settings;

/// Whether the cloud account is signed in, and with which profile.
#[derive(Default)]
pub struct Cloud {
    connected: bool,
    // the local profiles, read when signing in
    profiles: Vec<aws::Profile>,
    profile: String,
    // bumped whenever the account or profile changes: a listing still on its way is then stale
    generation: u64,
    // the last session's connection, reached once signed in and listed (see `resume`)
    resume: Option<Resume>,
}

impl Cloud {
    /// The profile whose connections are listed ("" for none).
    pub fn profile(&self) -> &str {
        &self.profile
    }
}

/// A cloud connection to get back to: its profile, its id, and the connect attempt
/// counted when it was asked for (one made since by the user wins).
struct Resume {
    profile: String,
    connection: String,
    attempt: u64,
}

/// Signs in (in the browser when the session has expired) unless signed in, lists `profile`'s
/// connections and connects to `connection`, as if picked one after the other.
pub fn resume(app: &App, window: &MainWindow, profile: String, connection: String) {
    if !window.global::<AwsAccount>().get_available() {
        return;
    }
    let attempt = app.state.connect_attempt.load(Ordering::SeqCst);
    let signed_in = app.cloud.borrow().connected;
    app.cloud.borrow_mut().resume = Some(Resume { profile: profile.clone(), connection, attempt });
    if signed_in {
        window.global::<AwsAccount>().invoke_profile_selected(profile.into());
    } else {
        window.global::<AwsAccount>().invoke_clicked();
    }
}

/// Raises the window and gives it the focus, for when the browser sign-in is done. A window
/// manager may refuse to let a window take the focus: the taskbar entry then asks for attention.
pub(crate) fn bring_to_front(window: &MainWindow) {
    window.window().with_winit_window(|winit_window| {
        winit_window.set_minimized(false);
        winit_window.focus_window();
        if !winit_window.has_focus() {
            winit_window.request_user_attention(Some(UserAttentionType::Informational));
        }
    });
}

/// The page the browser shows once an AWS sign-in there is over.
fn sign_in_page() -> aws::SignInPage {
    aws::SignInPage {
        title: "Cachemire".to_string(),
        signed_in: t!("You're signed in to AWS. You can close this tab and go back to Cachemire.").into_owned(),
        refused: t!("The AWS sign-in didn't go through. You can close this tab and try again from Cachemire.").into_owned(),
    }
}

/// The account profile `name` signs in to: the one it lists now or last listed, else the one
/// its SSO settings name.
pub(crate) fn account_of(app: &App, window: &MainWindow, name: &str) -> Option<String> {
    if name == window.global::<AwsAccount>().get_profile().as_str() {
        let listed = app.state.connections.lock().cloud_account().0.to_string();
        if !listed.is_empty() {
            return Some(listed);
        }
    }
    let remembered = app.state.settings.lock().cloud_profile_account(name);
    remembered.or_else(|| app.cloud.borrow().profiles.iter().find(|p| p.name == name).and_then(|p| p.account_id()).map(str::to_string))
}

/// What the list says of profile `name`: its account settings, or that its account isn't known.
fn profile_row(app: &App, window: &MainWindow, name: &str, now: i64) -> AwsProfileRow {
    let account = account_of(app, window, name);
    let settings = app.state.settings.lock();
    let when = if name == window.global::<AwsAccount>().get_profile().as_str() {
        t!("now").into_owned()
    } else {
        settings.cloud_profile_used(name).map(|used| crate::redisql::ago(now, used)).unwrap_or_default()
    };
    let mut row = AwsProfileRow { name: name.into(), when: when.into(), ..Default::default() };
    let Some(account) = account else {
        row.profile_label = t!("Account not known yet").as_ref().into();
        return row;
    };
    let entry = settings.cloud_settings(&CloudScope::Account(account));
    let profile = entry.connection_profile.filter(|id| !id.is_empty()).and_then(|id| settings.connection_profile(&id));
    row.known = true;
    row.has_tint = profile.is_some();
    match profile {
        Some(profile) => {
            row.tint = crate::connections::profiles::to_tint(profile.color);
            row.profile_label = profile.name.into();
        }
        None => row.profile_label = t!("No connection profile").as_ref().into(),
    }
    row.ssh_label = match entry.ssh.filter(|ssh| ssh.enabled) {
        Some(ssh) => t!("SSH %{host}", host = ssh.host).as_ref().into(),
        None => t!("No SSH tunnel").as_ref().into(),
    };
    row
}

/// Fills the list with the profiles whose name holds `filter`: without one, the three last
/// used first, then the others.
fn list(app: &App, window: &MainWindow, filter: &str) {
    let filter = filter.trim().to_lowercase();
    let names: Vec<String> = app.cloud.borrow().profiles.iter().map(|p| p.name.clone()).collect();
    let mut order: Vec<(&str, Option<String>)> = Vec::new();
    if filter.is_empty() {
        let mut recent: Vec<(&str, i64)> = {
            let settings = app.state.settings.lock();
            names.iter().filter_map(|n| settings.cloud_profile_used(n).map(|used| (n.as_str(), used))).collect()
        };
        recent.sort_by_key(|(_, used)| std::cmp::Reverse(*used));
        recent.truncate(3);
        let rest: Vec<&str> = names.iter().map(String::as_str).filter(|n| !recent.iter().any(|(r, _)| r == n)).collect();
        let sections = !recent.is_empty() && !rest.is_empty();
        for (i, (name, _)) in recent.iter().enumerate() {
            order.push((name, (sections && i == 0).then(|| t!("Recent").to_uppercase())));
        }
        for (i, name) in rest.iter().enumerate() {
            order.push((name, (sections && i == 0).then(|| t!("All profiles").to_uppercase())));
        }
    } else {
        order.extend(names.iter().filter(|n| n.to_lowercase().contains(&filter)).map(|n| (n.as_str(), None)));
    }

    let now = cachemire_core::now_ms();
    let mut sections_above = 0;
    let rows: Vec<AwsProfileRow> = order
        .into_iter()
        .map(|(name, section)| {
            sections_above += i32::from(section.is_some());
            AwsProfileRow { section: section.unwrap_or_default().into(), sections_above, ..profile_row(app, window, name, now) }
        })
        .collect();
    let count = match (filter.is_empty(), names.len()) {
        (true, 1) => t!("1 profile").into_owned(),
        (true, total) => t!("%{n} profiles", n = total).into_owned(),
        (false, total) => t!("%{shown} of %{total}", shown = rows.len(), total = total).into_owned(),
    };
    let account = window.global::<AwsAccount>();
    account.set_profiles(ModelRc::new(VecModel::from(rows)));
    account.set_highlighted(0);
    account.set_count_label(count.into());
}

/// Forgets the account's connections, dropping the one in use if it is one of them.
fn clear_connections(app: &App, window: &MainWindow) {
    if window.connection_id().starts_with(aws::ID_PREFIX) {
        crate::connections::disconnect(app, window);
    }
    app.state.connections.lock().set_cloud_connections("", "", Vec::new());
    window.global::<AwsAccount>().set_account_known(false);
    crate::connections::refresh(app, window);
}

pub fn init(app: &Rc<App>, window: &MainWindow) {
    window.global::<AwsAccount>().set_available(aws::aws_dir().is_some());

    window.global::<AwsAccount>().on_list_requested({
        let app = app.clone();
        move |filter| {
            let Some(window) = app.window.upgrade() else { return };
            list(&app, &window, &filter);
        }
    });

    window.global::<AwsAccount>().on_clicked({
        let app = app.clone();
        move || {
            let Some(window) = app.window.upgrade() else { return };
            if window.global::<AwsAccount>().get_busy() {
                return;
            }
            // signed in: the button signs out
            if app.cloud.borrow().connected {
                {
                    let mut cloud = app.cloud.borrow_mut();
                    *cloud = Cloud { generation: cloud.generation + 1, ..Cloud::default() };
                }
                clear_connections(&app, &window);
                window.global::<AwsAccount>().set_connected(false);
                window.global::<AwsAccount>().set_profile("".into());
                window.global::<AwsAccount>().set_error("".into());
                return;
            }

            window.global::<AwsAccount>().set_error("".into());
            window.global::<AwsAccount>().set_busy(true);
            let page = sign_in_page();
            let signed_in = async move {
                aws::login(&page).await?;
                Ok(aws::profiles().await)
            };
            crate::app::spawn(&app.rt, signed_in, |app, window, signed_in| {
                window.global::<AwsAccount>().set_busy(false);
                bring_to_front(window);
                match signed_in.flatten() {
                    Ok(profiles) => {
                        tracing::info!(profiles = profiles.len(), "signed in to AWS");
                        let resumed = {
                            let mut cloud = app.cloud.borrow_mut();
                            cloud.resume = cloud.resume.take().filter(|r| profiles.iter().any(|p| p.name == r.profile));
                            cloud.profiles = profiles;
                            cloud.resume.as_ref().map(|r| r.profile.clone())
                        };
                        app.cloud.borrow_mut().connected = true;
                        window.global::<AwsAccount>().set_connected(true);
                        if let Some(profile) = resumed {
                            window.global::<AwsAccount>().invoke_profile_selected(profile.into());
                        }
                    }
                    Err(e) => {
                        tracing::warn!("aws sign-in failed: {e}");
                        app.cloud.borrow_mut().resume = None;
                        window.global::<AwsAccount>().set_error(e.localized().into());
                    }
                }
            });
        }
    });

    window.global::<AwsAccount>().on_profile_selected({
        let app = app.clone();
        move |name| {
            let Some(window) = app.window.upgrade() else { return };
            let name = name.to_string();
            {
                let mut cloud = app.cloud.borrow_mut();
                if cloud.resume.as_ref().is_some_and(|r| r.profile != name) {
                    cloud.resume = None;
                }
                if cloud.profile == name {
                    return;
                }
            }
            clear_connections(&app, &window);
            let generation = {
                let mut cloud = app.cloud.borrow_mut();
                cloud.profile.clone_from(&name);
                cloud.generation += 1;
                cloud.generation
            };
            window.global::<AwsAccount>().set_profile(name.as_str().into());
            window.global::<AwsAccount>().set_error("".into());
            window.global::<AwsAccount>().set_busy(true);

            let page = sign_in_page();
            let profile = name.clone();
            let listed = async move { aws::redis_connections(&profile, &page).await };
            crate::app::spawn(&app.rt, listed, move |app, window, result| {
                if app.cloud.borrow().generation != generation {
                    return;
                }
                let state = &app.state;
                window.global::<AwsAccount>().set_busy(false);
                bring_to_front(window);
                match result.flatten() {
                    Ok(aws::Listing { account, region, mut items }) => {
                        tracing::info!(%account, %region, endpoints = items.len(), "listed the cloud connections");
                        // a complete listing: counts the endpoints with settings that it lacks
                        let present: FxHashSet<String> = items.iter().map(|item| item.name.clone()).collect();
                        let cleanups = {
                            let mut settings = state.settings.lock();
                            let cleanups = settings.observe_cloud_listing(&account, &region, &present).unwrap_or_else(|e| {
                                tracing::error!("pruning the cloud settings failed: {e}");
                                Vec::new()
                            });
                            settings.apply_cloud(&account, &region, &mut items);
                            cleanups
                        };
                        app.rt.spawn_blocking(move || cleanups.into_iter().for_each(|cleanup| cleanup.run()));
                        if let Err(e) = state.settings.lock().remember_cloud_profile(&name, &account, &region) {
                            tracing::warn!("can't save the profile's account: {e}");
                        }
                        state.connections.lock().set_cloud_connections(&account, &region, items);
                        window.global::<AwsAccount>().set_account_known(true);
                        crate::connections::refresh(app, window);
                        // the last session's connection, unless one was picked since
                        let resume = app.cloud.borrow_mut().resume.take().filter(|r| {
                            r.attempt == state.connect_attempt.load(Ordering::SeqCst)
                                && state.connections.lock().get(&r.connection).is_some()
                        });
                        if let Some(resume) = resume {
                            crate::connections::connect_to(window, state.clone(), &app.rt, resume.connection, true);
                        }
                        // a link that waited for this account
                        crate::links::retry_pending(app, window);
                    }
                    Err(e) => {
                        app.cloud.borrow_mut().resume = None;
                        tracing::warn!("listing the cloud connections failed: {e}");
                        window.global::<AwsAccount>().set_error(e.localized().into());
                    }
                }
            });
        }
    });
}
