//! The import / export window for connections: reads and writes a JSON file (`exportedAtUtc`,
//! `includesPasswords`, `count`, `connections`), plus an `ssh` object on
//! a connection with a tunnel. Importing never activates, overwrites or merges with a saved one.

use rustc_hash::FxHashSet;
use std::path::PathBuf;
use std::rc::Rc;

use rust_i18n::t;
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app::App;
use crate::backend::connection_store::build_connection_string;
use crate::backend::error::AppError;
use crate::backend::models::ConnectionPayloadInput;
use crate::backend::secret_vault::staged;
use crate::backend::{LockExt, now_ms, persist};
use crate::{ConnectionTransfer, MainWindow, TransferRow};

/// The SSH tunnel of an imported connection: `privateKey` is the path of the key file.
struct SshDraft {
    host: String,
    port: u16,
    username: String,
    private_key: String,
    password: Option<String>,
    passphrase: Option<String>,
}

/// One connection of an import file.
struct Draft {
    name: String,
    connection_url: String,
    username: String,
    password: Option<String>,
    ssh: Option<SshDraft>,
}

/// What the import list shows, and the saved connection behind each row of the export list.
#[derive(Default)]
pub struct TransferLists {
    drafts: Vec<Draft>,
    exportable: Vec<String>,
}

fn ssh_draft_of(item: &Value) -> Option<SshDraft> {
    let ssh = item.get("ssh").filter(|ssh| ssh.is_object())?;
    let text = |key: &str| ssh.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let secret = |key: &str| ssh.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let host = text("host");
    let port = ssh.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0).unwrap_or(22);
    (!host.is_empty()).then(|| SshDraft { host, port, username: text("username"), private_key: text("privateKey"), password: secret("password"), passphrase: secret("passphrase") })
}

fn draft_of(item: &Value) -> Option<Draft> {
    let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let (name, connection_url, host) = (text("name"), text("connectionUrl"), text("host"));
    if name.is_empty() || (connection_url.is_empty() && host.is_empty()) {
        return None;
    }
    let port = item.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0).unwrap_or(6379);
    let db = item.get("db").and_then(Value::as_u64).and_then(|d| u32::try_from(d).ok()).unwrap_or(0);
    let connection_url = if connection_url.is_empty() { build_connection_string(&host, port, db, "", "") } else { connection_url };
    Some(Draft { name, connection_url, username: text("username"), password: item.get("password").and_then(Value::as_str).map(str::to_string), ssh: ssh_draft_of(item) })
}

/// The connections of a file: its `connections` list, or the file itself when it is a list.
fn parse_drafts(text: &str) -> Result<Vec<Draft>, AppError> {
    let root: Value = serde_json::from_str(text).map_err(|e| AppError::invalid(e.to_string()))?;
    let list = root.get("connections").and_then(Value::as_array).or_else(|| root.as_array());
    let drafts: Vec<Draft> = list.map(|list| list.iter().filter_map(draft_of).collect()).unwrap_or_default();
    if drafts.is_empty() { Err(AppError::invalid(t!("This file has no connections."))) } else { Ok(drafts) }
}

/// UTC date and time of `ms` since the epoch: year, month, day, hour, minute, second, millisecond.
fn utc_parts(ms: i64) -> (i64, i64, i64, i64, i64, i64, i64) {
    let (secs, millis) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (days, in_day) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil date from the day count (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day, in_day / 3600, in_day % 3600 / 60, in_day % 60, millis)
}

fn set_message(window: &MainWindow, text: String, ok: bool) {
    let global = window.global::<ConnectionTransfer>();
    global.set_message(text.into());
    global.set_message_ok(ok);
}

/// How many rows of the list are ticked.
fn ticked(rows: &ModelRc<TransferRow>) -> i32 {
    rows.iter().filter(|row| row.ticked).count() as i32
}

fn refresh_counts(window: &MainWindow) {
    let global = window.global::<ConnectionTransfer>();
    global.set_import_count(ticked(&global.get_import_rows()));
    global.set_export_count(ticked(&global.get_export_rows()));
}

/// A name for an imported connection that no saved connection (or one imported before it) has.
fn free_name(name: &str, taken: &mut FxHashSet<String>) -> String {
    let mut candidate = name.to_string();
    let mut n = 2;
    while !taken.insert(candidate.to_lowercase()) {
        candidate = format!("{name} ({n})");
        n += 1;
    }
    candidate
}

pub fn init(app: &Rc<App>, window: &MainWindow) {
    let (state, rt) = (app.state.clone(), app.rt.clone());
    let global = window.global::<ConnectionTransfer>();

    global.on_open_requested({
        let app = app.clone();
        move || {
            let Some(window) = app.window.upgrade() else { return };
            let connections = app.state.connections.lock_recover().list_state().connections;
            let mut lists = app.transfer.borrow_mut();
            lists.exportable = connections.iter().map(|c| c.id.clone()).collect();
            lists.drafts.clear();
            drop(lists);
            let rows: Vec<TransferRow> = connections
                .iter()
                .map(|c| TransferRow { name: c.name.as_str().into(), detail: c.connection_url.as_str().into(), ticked: true, note: "".into() })
                .collect();
            let global = window.global::<ConnectionTransfer>();
            global.set_export_rows(ModelRc::new(VecModel::from(rows)));
            global.set_import_rows(ModelRc::new(VecModel::from(Vec::<TransferRow>::new())));
            global.set_import_file("".into());
            global.set_export_passwords(false);
            global.set_tab(0);
            global.set_busy(false);
            set_message(&window, String::new(), false);
            refresh_counts(&window);
            global.set_open(true);
        }
    });

    global.on_toggle({
        let weak = window.as_weak();
        move |tab, index| {
            let Some(window) = weak.upgrade() else { return };
            let global = window.global::<ConnectionTransfer>();
            let rows = if tab == 0 { global.get_import_rows() } else { global.get_export_rows() };
            if let Some(mut row) = rows.row_data(index as usize) {
                row.ticked = !row.ticked;
                rows.set_row_data(index as usize, row);
            }
            refresh_counts(&window);
        }
    });

    global.on_toggle_range({
        let weak = window.as_weak();
        move |tab, from, to| {
            let Some(window) = weak.upgrade() else { return };
            let global = window.global::<ConnectionTransfer>();
            let rows = if tab == 0 { global.get_import_rows() } else { global.get_export_rows() };
            let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else { return };
            let Some(on) = rows.row_data(from).map(|row| row.ticked) else { return };
            for i in from.min(to)..=from.max(to).min(rows.row_count().saturating_sub(1)) {
                if let Some(mut row) = rows.row_data(i).filter(|row| row.ticked != on) {
                    row.ticked = on;
                    rows.set_row_data(i, row);
                }
            }
            refresh_counts(&window);
        }
    });

    global.on_set_all({
        let weak = window.as_weak();
        move |tab, all| {
            let Some(window) = weak.upgrade() else { return };
            let global = window.global::<ConnectionTransfer>();
            let rows = if tab == 0 { global.get_import_rows() } else { global.get_export_rows() };
            for (i, mut row) in rows.iter().enumerate() {
                if row.ticked != all {
                    row.ticked = all;
                    rows.set_row_data(i, row);
                }
            }
            refresh_counts(&window);
        }
    });

    // "Choose a file…": reads it off the UI thread and lists its connections, all ticked
    global.on_choose_file({
        let (state, rt) = (state.clone(), rt.clone());
        move || {
            let state = state.clone();
            rt.spawn_blocking(move || {
                let Some(path) = rfd::FileDialog::new().set_title(t!("Select a connections file")).add_filter("JSON", &["json"]).pick_file() else { return };
                let parsed = std::fs::read_to_string(&path).map_err(|e| AppError::File(t!("Couldn't read the file: %{error}", error = e).into_owned())).and_then(|text| parse_drafts(&text));
                let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = slint::invoke_from_event_loop(move || crate::app::with(|app, window| {
                    let global = window.global::<ConnectionTransfer>();
                    match parsed {
                        Ok(drafts) => {
                            let taken: FxHashSet<String> = state.connections.lock_recover().list_state().connections.iter().map(|c| c.name.to_lowercase()).collect();
                            let rows: Vec<TransferRow> = drafts
                                .iter()
                                .map(|d| TransferRow {
                                    name: d.name.as_str().into(),
                                    detail: match &d.ssh {
                                        Some(ssh) => t!("%{address} via SSH %{host}", address = d.connection_url.as_str(), host = ssh.host).into_owned().into(),
                                        None => d.connection_url.as_str().into(),
                                    },
                                    ticked: true,
                                    note: if taken.contains(&d.name.to_lowercase()) { t!("added as a copy").into_owned().into() } else { "".into() },
                                })
                                .collect();
                            app.transfer.borrow_mut().drafts = drafts;
                            global.set_import_rows(ModelRc::new(VecModel::from(rows)));
                            global.set_import_file(file.into());
                            set_message(window, String::new(), false);
                        }
                        Err(e) => {
                            app.transfer.borrow_mut().drafts.clear();
                            global.set_import_rows(ModelRc::new(VecModel::from(Vec::<TransferRow>::new())));
                            global.set_import_file(file.into());
                            set_message(window, e.to_string(), false);
                        }
                    }
                    refresh_counts(window);
                }));
            });
        }
    });

    global.on_run_import({
        let (app, rt) = (app.clone(), rt.clone());
        move || {
            let Some(window) = app.window.upgrade() else { return };
            let global = window.global::<ConnectionTransfer>();
            let ticks: Vec<bool> = global.get_import_rows().iter().map(|row| row.ticked).collect();
            let chosen: Vec<ConnectionPayloadInput> = {
                app.transfer
                    .borrow()
                    .drafts
                    .iter()
                    .zip(&ticks)
                    .filter(|(_, ticked)| **ticked)
                    .map(|(d, _)| ConnectionPayloadInput {
                        name: Some(d.name.clone()),
                        connection_url: Some(d.connection_url.clone()),
                        username: Some(d.username.clone()),
                        password: d.password.clone().filter(|p| !p.is_empty()),
                        ssh_enabled: Some(d.ssh.is_some()),
                        ssh_host: d.ssh.as_ref().map(|s| s.host.clone()),
                        ssh_port: d.ssh.as_ref().map(|s| s.port),
                        ssh_username: d.ssh.as_ref().map(|s| s.username.clone()),
                        ssh_private_key: d.ssh.as_ref().map(|s| s.private_key.clone()),
                        ssh_password: d.ssh.as_ref().and_then(|s| s.password.clone()),
                        ssh_passphrase: d.ssh.as_ref().and_then(|s| s.passphrase.clone()),
                        ..Default::default()
                    })
                    .collect()
            };
            if chosen.is_empty() {
                return;
            }
            global.set_busy(true);
            let state = app.state.clone();
            rt.spawn_blocking(move || {
                // the names are chosen once, so the rehearsal and the real run save the same thing
                let mut taken: FxHashSet<String> = state.connections.lock_recover().list_state().connections.iter().map(|c| c.name.to_lowercase()).collect();
                let items: Vec<(String, ConnectionPayloadInput)> = chosen
                    .into_iter()
                    .map(|mut payload| {
                        payload.name = payload.name.map(|name| free_name(&name, &mut taken));
                        (uuid::Uuid::new_v4().to_string(), payload)
                    })
                    .collect();
                let result = staged(&state.connections, |store| items.iter().try_for_each(|(id, payload)| store.import_with_id(id, payload.clone())));
                let count = items.len();
                let _ = slint::invoke_from_event_loop(move || crate::app::with(|app, window| {
                    window.global::<ConnectionTransfer>().set_busy(false);
                    match result {
                        Ok(()) => {
                            app.transfer.borrow_mut().drafts.clear();
                            let global = window.global::<ConnectionTransfer>();
                            global.set_import_rows(ModelRc::new(VecModel::from(Vec::<TransferRow>::new())));
                            global.set_import_file("".into());
                            refresh_counts(window);
                            crate::connections::refresh(app, window);
                            set_message(window, t!("Imported %{count} connection(s).", count = count).into_owned(), true);
                        }
                        Err(e) => set_message(window, e.to_string(), false),
                    }
                }));
            });
        }
    });

    global.on_run_export({
        let (app, weak, state, rt) = (app.clone(), window.as_weak(), state, rt);
        move || {
            let Some(window) = weak.upgrade() else { return };
            let global = window.global::<ConnectionTransfer>();
            let ticks: Vec<bool> = global.get_export_rows().iter().map(|row| row.ticked).collect();
            let ids: Vec<String> = app.transfer.borrow().exportable.iter().zip(&ticks).filter(|(_, ticked)| **ticked).map(|(id, _)| id.clone()).collect();
            if ids.is_empty() {
                return;
            }
            let passwords = global.get_export_passwords();
            global.set_busy(true);
            let (weak, state) = (weak.clone(), state.clone());
            rt.spawn_blocking(move || {
                let (year, month, day, hour, minute, second, millis) = utc_parts(now_ms());
                let stamp = format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}");
                let outcome = (|| -> Result<Option<(PathBuf, usize)>, AppError> {
                    let Some(path) = rfd::FileDialog::new()
                        .set_title(t!("Export the connections"))
                        .set_file_name(format!("Cachemire-Connections-{stamp}.json"))
                        .add_filter("JSON", &["json"])
                        .save_file()
                    else {
                        return Ok(None);
                    };
                    if passwords {
                        // the passwords are fetched from the system keyring, off the store's lock
                        staged(&state.connections, |store| ids.iter().try_for_each(|id| store.unlock_secrets(id)))?;
                    }
                    let items: Vec<Value> = {
                        let store = state.connections.lock_recover();
                        ids.iter()
                            .filter_map(|id| store.get(id))
                            .map(|c| {
                                let secret = |text: String| if passwords { Value::String(text) } else { Value::Null };
                                // host, port and db as well, for a version that reads them
                                let (host, port, db) = c.endpoint().map_or((String::new(), 6379, 0), |e| (e.host, e.port, e.db));
                                let mut item = json!({
                                    "id": c.id, "name": c.name, "connectionUrl": c.connection_url, "host": host, "port": port,
                                    "db": db, "username": c.username, "password": secret(c.password),
                                });
                                if let Some(ssh) = c.ssh {
                                    item["ssh"] = json!({
                                        "host": ssh.host, "port": ssh.port, "username": ssh.username, "privateKey": ssh.private_key,
                                        "password": secret(ssh.password), "passphrase": secret(ssh.passphrase),
                                    });
                                }
                                item
                            })
                            .collect()
                    };
                    let count = items.len();
                    let exported_at = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z");
                    persist::save(&path, &json!({ "exportedAtUtc": exported_at, "includesPasswords": passwords, "count": count, "connections": items }))?;
                    Ok(Some((path, count)))
                })();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(window) = weak.upgrade() else { return };
                    window.global::<ConnectionTransfer>().set_busy(false);
                    match outcome {
                        Ok(Some((path, count))) => set_message(&window, t!("Exported %{count} connection(s) to %{file}.", count = count, file = path.display()).into_owned(), true),
                        Ok(None) => {}
                        Err(e) => set_message(&window, e.to_string(), false),
                    }
                });
            });
        }
    });
}
