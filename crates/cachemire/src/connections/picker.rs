//! The title bar's connection list: the saved connections and the cloud account's, filtered by
//! what is typed, a source and a connection profile chip, the pinned ones apart. Also what the
//! title bar says of the connection it shows (its profile, its server).

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app::App;
use crate::connections::profiles::to_tint;
use crate::error::Localize;
use crate::t;
use crate::{AwsAccount, AwsChip, CloudSettings, ConnectionEditor, ConnectionState, MainWindow, PickerRow, ProfileTint};
use cachemire_core::connection::{ConnectionItem, KubeSettings, ServerKind, kube_tunnel};
use cachemire_core::state::AppState;

/// The source keys: the saved connections, and the cloud account's.
const SAVED: &str = "saved";
const CLOUD: &str = "cloud";

/// The chip of the connections using no connection profile.
const NO_PROFILE_CHIP: &str = "none";

/// The room a line of chips has at zoom 1 (the list's width less its padding).
const CHIP_ROOM: f32 = 502.0;

/// "Valkey 9.0.0" or "Redis", as the hover card names a server.
fn product_version(kind: &ServerKind) -> String {
    if kind.version.is_empty() { kind.product.clone() } else { format!("{} {}", kind.product, kind.version) }
}

/// What the title bar's details say of a server: "Valkey 9.0.0 · cluster of 3 primaries".
fn server_label(kind: &ServerKind) -> String {
    let server = product_version(kind);
    match kind.primaries {
        0 => t!("%{server} · standalone", server = server).into_owned(),
        1 => t!("%{server} · cluster of 1 primary", server = server).into_owned(),
        n => t!("%{server} · cluster of %{n} primaries", server = server, n = n).into_owned(),
    }
}

/// What a row says of a server's kind: "Cluster · 3 primaries" or "Standalone".
fn kind_label(kind: &ServerKind) -> String {
    match kind.primaries {
        0 => t!("Standalone").into_owned(),
        1 => t!("Cluster · 1 primary").into_owned(),
        n => t!("Cluster · %{n} primaries", n = n).into_owned(),
    }
}

/// What the title bar shows of connection `id`: its name, address, SSH server, profile and
/// server (as last seen), whatever state it is in.
pub(crate) struct Shown {
    pub name: String,
    pub address: String,
    pub ssh_via: String,
    pub kubernetes: bool,
    pub profile: Option<(String, ProfileTint)>,
    pub server: String,
}

pub(crate) fn shown(state: &AppState, id: &str) -> Shown {
    let (item, kind) = {
        let store = state.connections.lock();
        (store.get(id), store.server_kind(id))
    };
    let Some(item) = item else {
        return Shown {
            name: String::new(),
            address: String::new(),
            ssh_via: String::new(),
            kubernetes: false,
            profile: None,
            server: String::new(),
        };
    };
    let profile = (!item.profile_id.is_empty())
        .then(|| state.settings.lock().connection_profile(&item.profile_id))
        .flatten()
        .map(|p| (p.name, to_tint(p.color)));
    Shown {
        address: item.address(),
        ssh_via: item
            .kubernetes
            .as_ref()
            .map(kube_place)
            .or_else(|| item.ssh.as_ref().map(super::connect::ssh_server_label))
            .unwrap_or_default(),
        kubernetes: item.kubernetes.is_some(),
        name: item.name,
        profile,
        server: kind.as_ref().map(server_label).unwrap_or_default(),
    }
}

/// "context/namespace" of a Kubernetes connection, a part left out when empty.
pub(crate) fn kube_place(kube: &KubeSettings) -> String {
    match (kube.context.as_str(), kube.namespace.as_str()) {
        ("", "") => t!("current context").into_owned(),
        (context, "") => context.to_string(),
        ("", namespace) => namespace.to_string(),
        (context, namespace) => format!("{context}/{namespace}"),
    }
}

/// The row's tunnel label: "SSH via host" or "Kubernetes: place", empty without a tunnel.
fn tunnel_label(item: &ConnectionItem) -> String {
    match (&item.kubernetes, &item.ssh) {
        (Some(kube), _) => t!("Kubernetes: %{place}", place = kube_place(kube)).into_owned(),
        (None, Some(ssh)) => t!("SSH via %{host}", host = ssh.host).into_owned(),
        (None, None) => String::new(),
    }
}

/// A connection as a row of the list, with its chip's key, its source and when it was last used.
fn row_of(app: &App, window: &MainWindow, item: &ConnectionItem, cloud: bool, now: i64) -> (PickerRow, String, Option<i64>) {
    let state = &app.state;
    let (kind, used) = {
        let store = state.connections.lock();
        (store.server_kind(&item.id), store.last_connected_at(&item.id))
    };
    let profile = (!item.profile_id.is_empty()).then(|| state.settings.lock().connection_profile(&item.profile_id)).flatten();
    let active = window.connection_id().as_str() == item.id;
    let kept = !active && app.tabs.borrow().parked_connections().contains(&item.id);
    let when = if active {
        t!("in use").into_owned()
    } else if kept {
        t!("open").into_owned()
    } else {
        used.map(|used| crate::redisql::ago(now, used)).unwrap_or_default()
    };
    let read_only = cachemire_core::state::is_read_only(state, Some(&item.id)).unwrap_or(false);
    let mut row = PickerRow {
        id: item.id.as_str().into(),
        name: item.name.as_str().into(),
        address: item.address().into(),
        when: when.into(),
        active,
        kept,
        cloud,
        kind_label: kind.as_ref().map(kind_label).unwrap_or_default().into(),
        cluster: kind.as_ref().is_some_and(|k| k.primaries > 0),
        ssh_label: tunnel_label(item).into(),
        read_only,
        ..Default::default()
    };
    let chip = match profile {
        Some(profile) => {
            row.has_tint = true;
            row.tint = to_tint(profile.color);
            row.profile_label = profile.name.into();
            profile.id
        }
        None => {
            row.profile_label = t!("No profile").as_ref().into();
            NO_PROFILE_CHIP.to_string()
        }
    };
    (row, chip, used)
}

/// The connection profile chips: all the connections, each profile some use (in the settings'
/// order), those with none. None when they all fall under one chip.
fn chip_lines(app: &App, keys: &[&str]) -> Vec<Vec<AwsChip>> {
    let count = |key: &str| keys.iter().filter(|k| **k == key).count() as i32;
    let mut chips: Vec<AwsChip> = Vec::new();
    for profile in app.state.settings.lock().connection_profiles() {
        let n = count(&profile.id);
        if n > 0 {
            let tint = to_tint(profile.color);
            chips.push(AwsChip { key: profile.id.into(), label: profile.name.into(), has_tint: true, tint, count: n });
        }
    }
    let none = count(NO_PROFILE_CHIP);
    if none > 0 {
        chips.push(AwsChip { key: NO_PROFILE_CHIP.into(), label: t!("No profile").as_ref().into(), count: none, ..Default::default() });
    }
    if chips.len() < 2 {
        return Vec::new();
    }
    chips.insert(0, AwsChip { label: t!("All").as_ref().into(), count: keys.len() as i32, ..Default::default() });
    crate::cloud::in_lines(chips, CHIP_ROOM)
}

/// Whether `row` holds every word of `words` (lowercased) in its name, address, profile,
/// kind or SSH server.
fn matches(row: &PickerRow, words: &[String]) -> bool {
    let text = [&row.name, &row.address, &row.profile_label, &row.kind_label, &row.ssh_label].map(|t| t.to_lowercase()).join("\n");
    words.iter().all(|word| text.contains(word.as_str()))
}

/// A connection of the list: its row, its chip's key, when it was last used, whether it is a
/// cloud one.
struct Entry {
    row: PickerRow,
    chip: String,
    used: Option<i64>,
    cloud: bool,
}

/// Fills the list with the connections matching `filter`, under the source and chip picked: the
/// pinned ones apart, then the saved ones and the cloud account's, each last used first.
pub(crate) fn list(app: &App, window: &MainWindow, filter: &str) {
    let now = cachemire_core::now_ms();
    let (saved, cloud) = {
        let store = app.state.connections.lock();
        (store.list_state().connections, store.cloud_connections().to_vec())
    };
    let mut all: Vec<Entry> = Vec::new();
    for (items, is_cloud) in [(&saved, false), (&cloud, true)] {
        for item in items {
            let (row, chip, used) = row_of(app, window, item, is_cloud, now);
            all.push(Entry { row, chip, used, cloud: is_cloud });
        }
    }
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let state = window.global::<ConnectionState>();

    // the source and chip picked last, unless they aren't there any more
    let (mut source, mut chip) = {
        let settings = app.state.settings.lock();
        let (source, chip) = settings.connection_filters();
        (source.to_string(), chip.to_string())
    };
    if cloud.is_empty() || ![SAVED, CLOUD].contains(&source.as_str()) {
        source.clear();
    }
    let in_source = |is_cloud: bool, source: &str| source.is_empty() || (source == CLOUD) == is_cloud;
    let chip_keys: Vec<&str> =
        all.iter().filter(|e| in_source(e.cloud, &source) && matches(&e.row, &words)).map(|e| e.chip.as_str()).collect();
    let lines = chip_lines(app, &chip_keys);
    if !lines.iter().flatten().any(|c| c.key.as_str() == chip) {
        chip.clear();
    }

    let sources: Vec<AwsChip> = if cloud.is_empty() {
        Vec::new()
    } else {
        let count = |key: &str| {
            all.iter().filter(|e| in_source(e.cloud, key) && (chip.is_empty() || e.chip == chip) && matches(&e.row, &words)).count() as i32
        };
        // (the saved ones' label is the list's own, in Slint)
        [("", t!("All").into_owned()), (SAVED, String::new()), (CLOUD, "AWS".to_string())]
            .into_iter()
            .map(|(key, label)| AwsChip { key: key.into(), label: label.into(), count: count(key), ..Default::default() })
            .collect()
    };
    let kept = |e: &&Entry| in_source(e.cloud, &source) && (chip.is_empty() || e.chip == chip) && matches(&e.row, &words);

    let pinned: Vec<String> = app.state.settings.lock().pinned_connections().to_vec();
    let is_pinned = |row: &PickerRow| pinned.iter().any(|id| row.id.as_str() == id);
    let pins: Vec<PickerRow> = pinned
        .iter()
        .filter_map(|id| all.iter().find(|e| e.row.id.as_str() == id))
        .filter(kept)
        .map(|e| PickerRow { pinned: true, ..e.row.clone() })
        .collect();
    let mut sections: Vec<(String, Vec<&Entry>)> = Vec::new();
    let cloud_title = t!("AWS · %{profile}", profile = window.global::<AwsAccount>().get_profile().as_str()).to_uppercase();
    for (is_cloud, title) in [(false, t!("Saved connections").to_uppercase()), (true, cloud_title)] {
        let mut rows: Vec<&Entry> = all.iter().filter(|e| e.cloud == is_cloud && !is_pinned(&e.row)).filter(kept).collect();
        // last used first, then by name
        rows.sort_by(|a, b| b.used.cmp(&a.used).then_with(|| a.row.name.to_lowercase().cmp(&b.row.name.to_lowercase())));
        if !rows.is_empty() {
            sections.push((title, rows));
        }
    }
    // a title only tells the sections apart
    let titled = sections.len() > 1;
    let mut sections_above = 0;
    let mut rows: Vec<PickerRow> = Vec::new();
    for (title, entries) in sections {
        for (i, entry) in entries.into_iter().enumerate() {
            let section = if titled && i == 0 { title.clone() } else { String::new() };
            sections_above += i32::from(!section.is_empty());
            rows.push(PickerRow { section: section.into(), sections_above, ..entry.row.clone() });
        }
    }

    let total = all.len();
    let unfiltered = words.is_empty() && source.is_empty() && chip.is_empty();
    let count = match (unfiltered, total) {
        (true, 1) => t!("1 connection").into_owned(),
        (true, total) => t!("%{n} connections", n = total).into_owned(),
        (false, total) => t!("%{shown} of %{total}", shown = pins.len() + rows.len(), total = total).into_owned(),
    };
    state.set_source(source.as_str().into());
    state.set_chip(chip.as_str().into());
    state.set_sources(ModelRc::new(VecModel::from(sources)));
    let lines: Vec<ModelRc<AwsChip>> = lines.into_iter().map(|line| ModelRc::new(VecModel::from(line))).collect();
    state.set_chip_lines(ModelRc::new(VecModel::from(lines)));
    state.set_has_pinned(!pinned.is_empty());
    state.set_pins(ModelRc::new(VecModel::from(pins)));
    state.set_rows(ModelRc::new(VecModel::from(rows)));
    state.set_highlighted(0);
    state.set_count_label(count.into());
}

/// Keeps the source or the chip picked over the list, for the next times it opens.
fn save_filters(app: &App, window: &MainWindow, source: Option<&str>, chip: Option<&str>) {
    let saved = {
        let mut settings = app.state.settings.lock();
        let (old_source, old_chip) = settings.connection_filters();
        let (source, chip) = (source.unwrap_or(old_source).to_string(), chip.unwrap_or(old_chip).to_string());
        settings.set_connection_filters(&source, &chip)
    };
    if let Err(e) = saved {
        tracing::warn!("can't save the connection list's filters: {e}");
        crate::toast::show(window, &e.localized());
    }
}

/// Lists the kubeconfig's contexts in the form, and names the current one for the field's placeholder.
pub(super) fn fill_kube_contexts(editor: &ConnectionEditor) {
    let (contexts, current) = kube_tunnel::contexts().unwrap_or_default();
    let label = if current.is_empty() { t!("current context").into_owned() } else { t!("current: %{name}", name = current).into_owned() };
    editor.set_kube_contexts(ModelRc::new(VecModel::from(contexts.into_iter().map(SharedString::from).collect::<Vec<_>>())));
    editor.set_kube_current_label(label.into());
}

/// Opens the form on saved connection `item`, its passwords left blank (blank keeps them).
fn open_editor(window: &MainWindow, item: &ConnectionItem) {
    let editor = window.global::<ConnectionEditor>();
    let ssh = item.ssh.as_ref();
    editor.set_id(item.id.as_str().into());
    editor.set_name(item.name.as_str().into());
    editor.set_connection_url(item.connection_url.as_str().into());
    editor.set_username(item.username.as_str().into());
    editor.set_password("".into());
    editor.set_ssh_enabled(ssh.is_some());
    editor.set_ssh_host(ssh.map(|s| s.host.as_str()).unwrap_or_default().into());
    editor.set_ssh_port(ssh.map_or(22, |s| s.port).to_string().into());
    editor.set_ssh_username(ssh.map(|s| s.username.as_str()).unwrap_or_default().into());
    editor.set_ssh_password("".into());
    editor.set_ssh_key(ssh.map(|s| s.private_key.as_str()).unwrap_or_default().into());
    editor.set_ssh_passphrase("".into());
    let kube = item.kubernetes.as_ref();
    editor.set_kube_enabled(kube.is_some());
    editor.set_kube_context(kube.map(|k| k.context.as_str()).unwrap_or_default().into());
    editor.set_kube_namespace(kube.map(|k| k.namespace.as_str()).unwrap_or_default().into());
    editor.set_kube_relay_pod(kube.map(|k| k.relay_pod.as_str()).unwrap_or_default().into());
    fill_kube_contexts(&editor);
    editor.set_profile_enabled(!item.profile_id.is_empty());
    editor.set_profile_id(item.profile_id.as_str().into());
    editor.set_read_only(item.read_only);
    editor.set_error("".into());
    editor.set_test_status("".into());
    editor.set_is_edit(true);
    editor.set_open(true);
}

/// Edits connection `id` ("" for the one the title bar shows): the form for a saved one, the
/// settings of a cloud one.
fn edit(app: &App, window: &MainWindow, id: &str) {
    let id = if id.is_empty() { app.picked.borrow().clone() } else { id.to_string() };
    if id.is_empty() {
        return;
    }
    let (item, cloud) = {
        let store = app.state.connections.lock();
        (store.get(&id), store.cloud_key(&id).is_some())
    };
    match item {
        Some(_) if cloud => window.global::<CloudSettings>().invoke_open_requested(1, id.into()),
        Some(item) => open_editor(window, &item),
        None => {}
    }
}

pub(crate) fn init(window: &MainWindow) {
    let state = window.global::<ConnectionState>();
    state.on_list_requested(|filter| crate::app::with(|app, window| list(app, window, &filter)));
    state.on_source_picked(|key| crate::app::with(|app, window| save_filters(app, window, Some(&key), None)));
    state.on_chip_picked(|key| crate::app::with(|app, window| save_filters(app, window, None, Some(&key))));
    state.on_pin_toggled(|id| {
        crate::app::with(|app, window| {
            let saved = {
                let mut settings = app.state.settings.lock();
                let pinned = settings.pinned_connections().iter().any(|p| p.as_str() == id.as_str());
                settings.set_connection_pinned(&id, !pinned)
            };
            if let Err(e) = saved {
                tracing::warn!("can't save the pinned connections: {e}");
                crate::toast::show(window, &e.localized());
            }
        });
    });
    state.on_edit_requested(|id| crate::app::with(|app, window| edit(app, window, &id)));
}
