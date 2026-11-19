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
use crate::t;
use crate::{AwsAccount, AwsChip, AwsProfileRow, MainWindow};
use cachemire_core::aws;
use cachemire_core::settings::CloudScope;

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
    // what the list's filter box last held, to list again after a pin
    filter: String,
}

impl Cloud {
    /// The local profiles, as a sign-in would have read them.
    #[cfg(test)]
    pub fn set_profiles(&mut self, profiles: Vec<aws::Profile>) {
        self.profiles = profiles;
    }

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

/// The chip of the profiles using no connection profile, and of those whose account isn't known.
const NO_PROFILE_CHIP: &str = "none";
const UNKNOWN_CHIP: &str = "unknown";

/// The chip of the profiles using connection profile `id`.
fn profile_chip(id: &str) -> String {
    format!("profile:{id}")
}

/// What the list says of profile `name`: its account settings, or that its account isn't known;
/// and the key of the chip it falls under.
fn profile_row(app: &App, window: &MainWindow, name: &str, now: i64) -> (AwsProfileRow, String) {
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
        return (row, UNKNOWN_CHIP.to_string());
    };
    let entry = settings.cloud_settings(&CloudScope::Account(account));
    let profile = entry.connection_profile.filter(|id| !id.is_empty()).and_then(|id| settings.connection_profile(&id));
    row.known = true;
    row.has_tint = profile.is_some();
    let chip = match profile {
        Some(profile) => {
            row.tint = crate::connections::profiles::to_tint(profile.color);
            row.profile_label = profile.name.into();
            profile_chip(&profile.id)
        }
        None => {
            row.profile_label = t!("No connection profile").as_ref().into();
            NO_PROFILE_CHIP.to_string()
        }
    };
    row.ssh_label = match entry.ssh.filter(|ssh| ssh.enabled) {
        Some(ssh) => t!("SSH %{host}", host = ssh.host).as_ref().into(),
        None => t!("No SSH tunnel").as_ref().into(),
    };
    (row, chip)
}

/// The room a line of chips has, at zoom 1 (the list's width less its padding), and between chips.
const CHIP_ROOM: f32 = 442.0;
const CHIP_GAP: f32 = 6.0;

/// About how wide `chip` is drawn at zoom 1 (its 11 px text averages under 6.5 px a character).
fn chip_width(chip: &AwsChip) -> f32 {
    let dot = if chip.has_tint { 11.0 } else { 0.0 };
    let characters = chip.label.chars().count() + chip.count.to_string().len();
    20.0 + dot + 5.0 + 6.5 * characters as f32
}

/// The chips over the list, in lines that fit it: all the profiles, each connection profile some
/// use (in the settings' order), those with none, those whose account isn't known yet. None when
/// they all fall under one chip. `keys` holds each profile's chip.
fn chip_lines(app: &App, keys: &[String]) -> Vec<Vec<AwsChip>> {
    let count = |key: &str| keys.iter().filter(|k| *k == key).count() as i32;
    let mut chips: Vec<AwsChip> = Vec::new();
    for profile in app.state.settings.lock().connection_profiles() {
        let key = profile_chip(&profile.id);
        let n = count(&key);
        if n > 0 {
            let tint = crate::connections::profiles::to_tint(profile.color);
            chips.push(AwsChip { key: key.into(), label: profile.name.into(), has_tint: true, tint, count: n });
        }
    }
    for (key, label) in [(NO_PROFILE_CHIP, t!("No profile")), (UNKNOWN_CHIP, t!("Unknown account"))] {
        let n = count(key);
        if n > 0 {
            chips.push(AwsChip { key: key.into(), label: label.as_ref().into(), count: n, ..Default::default() });
        }
    }
    if chips.len() < 2 {
        return Vec::new();
    }
    chips.insert(0, AwsChip { label: t!("All").as_ref().into(), count: keys.len() as i32, ..Default::default() });
    let mut lines: Vec<Vec<AwsChip>> = Vec::new();
    let mut used = CHIP_ROOM;
    for chip in chips {
        let width = chip_width(&chip);
        match lines.last_mut() {
            Some(line) if used + CHIP_GAP + width <= CHIP_ROOM => {
                used += CHIP_GAP + width;
                line.push(chip);
            }
            _ => {
                used = width;
                lines.push(vec![chip]);
            }
        }
    }
    lines
}

/// Fills the list with the profiles whose name holds `filter`, under the chip picked: the pinned
/// ones apart, then without a filter the three last used first, then the others.
fn list(app: &App, window: &MainWindow, filter: &str) {
    app.cloud.borrow_mut().filter = filter.to_string();
    let words = filter.trim().to_lowercase();
    let now = cachemire_core::now_ms();
    let names: Vec<String> = app.cloud.borrow().profiles.iter().map(|p| p.name.clone()).collect();
    let rows: Vec<(AwsProfileRow, String)> = names.iter().map(|name| profile_row(app, window, name, now)).collect();
    let keys: Vec<String> = rows.iter().map(|(_, key)| key.clone()).collect();
    let lines = chip_lines(app, &keys);
    let account = window.global::<AwsAccount>();
    // the chip picked last, unless it isn't there (its connection profile no longer used)
    let mut chip = app.state.settings.lock().cloud_chip().to_string();
    if !lines.iter().flatten().any(|c| c.key.as_str() == chip) {
        chip.clear();
    }
    account.set_chip(chip.as_str().into());
    let unfiltered = chip.is_empty() && words.is_empty();
    let kept = |(row, key): &(AwsProfileRow, String)| (chip.is_empty() || *key == chip) && row.name.to_lowercase().contains(&words);

    let pinned: Vec<String> = app.state.settings.lock().cloud_pinned().to_vec();
    let is_pinned = |row: &AwsProfileRow| pinned.iter().any(|name| row.name.as_str() == name);
    let pins: Vec<AwsProfileRow> = pinned
        .iter()
        .filter_map(|name| rows.iter().find(|(row, _)| row.name.as_str() == name))
        .filter(|entry| kept(entry))
        .map(|(row, _)| row.clone())
        .collect();
    let rest: Vec<&AwsProfileRow> = rows.iter().filter(|entry| !is_pinned(&entry.0) && kept(entry)).map(|(row, _)| row).collect();

    let mut order: Vec<(&AwsProfileRow, Option<String>)> = Vec::new();
    if unfiltered {
        let mut recent: Vec<(&AwsProfileRow, i64)> = {
            let settings = app.state.settings.lock();
            rest.iter().filter_map(|row| settings.cloud_profile_used(&row.name).map(|used| (*row, used))).collect()
        };
        recent.sort_by_key(|(_, used)| std::cmp::Reverse(*used));
        recent.truncate(3);
        let others: Vec<&AwsProfileRow> = rest.iter().filter(|row| !recent.iter().any(|(r, _)| r.name == row.name)).copied().collect();
        let sections = !recent.is_empty() && !others.is_empty();
        for (i, (row, _)) in recent.iter().enumerate() {
            order.push((row, (sections && i == 0).then(|| t!("Recent").to_uppercase())));
        }
        for (i, row) in others.iter().enumerate() {
            order.push((row, (sections && i == 0).then(|| t!("All profiles").to_uppercase())));
        }
    } else {
        order.extend(rest.iter().map(|row| (*row, None)));
    }

    let mut sections_above = 0;
    let list_rows: Vec<AwsProfileRow> = order
        .into_iter()
        .map(|(row, section)| {
            sections_above += i32::from(section.is_some());
            AwsProfileRow { section: section.unwrap_or_default().into(), sections_above, ..row.clone() }
        })
        .collect();
    let count = match (unfiltered, names.len()) {
        (true, 1) => t!("1 profile").into_owned(),
        (true, total) => t!("%{n} profiles", n = total).into_owned(),
        (false, total) => t!("%{shown} of %{total}", shown = pins.len() + list_rows.len(), total = total).into_owned(),
    };
    account.set_has_pinned(rows.iter().any(|(row, _)| is_pinned(row)));
    account.set_pins_title(t!("Pinned").to_uppercase().into());
    account.set_pins(ModelRc::new(VecModel::from(pins)));
    let lines: Vec<ModelRc<AwsChip>> = lines.into_iter().map(|line| ModelRc::new(VecModel::from(line))).collect();
    account.set_chip_lines(ModelRc::new(VecModel::from(lines)));
    account.set_profiles(ModelRc::new(VecModel::from(list_rows)));
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

    // the AWS config edited since signing in: its profiles are read again and listed
    window.global::<AwsAccount>().on_reload_requested({
        let app = app.clone();
        move || {
            crate::app::spawn(&app.rt, aws::profiles(), |app, window, profiles| {
                let Ok(profiles) = profiles else { return };
                let filter = {
                    let mut cloud = app.cloud.borrow_mut();
                    // signed out meanwhile
                    if !cloud.connected {
                        return;
                    }
                    tracing::info!(profiles = profiles.len(), "AWS profiles reloaded");
                    cloud.profiles = profiles;
                    cloud.filter.clone()
                };
                list(app, window, &filter);
            });
        }
    });

    window.global::<AwsAccount>().on_pin_toggled({
        let app = app.clone();
        move |name| {
            let Some(window) = app.window.upgrade() else { return };
            let saved = {
                let mut settings = app.state.settings.lock();
                let pinned = settings.cloud_pinned().iter().any(|p| p.as_str() == name.as_str());
                settings.set_cloud_pinned(&name, !pinned)
            };
            if let Err(e) = saved {
                tracing::warn!("can't save the pinned profiles: {e}");
                crate::toast::show(&window, &e.localized());
            }
            let filter = app.cloud.borrow().filter.clone();
            list(&app, &window, &filter);
        }
    });

    window.global::<AwsAccount>().on_chip_picked({
        let app = app.clone();
        move |key| {
            let Some(window) = app.window.upgrade() else { return };
            if let Err(e) = app.state.settings.lock().set_cloud_chip(&key) {
                tracing::warn!("can't save the picked chip: {e}");
                crate::toast::show(&window, &e.localized());
            }
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
