//! The cloud settings form: what an AWS account sets for all its endpoints, and what one
//! endpoint sets over it, with the endpoints listed as a sign-in would have left them.

use super::*;
use crate::{AwsAccount, CloudSettings};
use cachemire_core::connection::{CloudKey, ConnectionItem};
use cachemire_core::settings::{CloudScope, CloudSsh};

const ACCOUNT: &str = "123456789012";
const ENDPOINT: &str = "aws:dev:orders";

/// The form wired to an app signed in to [`ACCOUNT`], whose one endpoint is [`ENDPOINT`].
fn form_ready() -> (Harness, Rc<crate::app::App>, tokio::runtime::Runtime, EventLoopTurn) {
    let h = Harness::new();
    let (app, runtime, turn) = install_app(&h, &test_state());
    crate::cloud::settings::init(&h.window);
    h.window.global::<AwsAccount>().set_profile("dev".into());
    let orders = ConnectionItem {
        id: ENDPOINT.into(),
        name: "orders".into(),
        connection_url: "rediss://orders.cache.amazonaws.com:6379".into(),
        username: String::new(),
        password: String::new(),
        ssh: None,
        profile_id: String::new(),
    };
    app.state.connections.lock().set_cloud_connections(ACCOUNT, "eu-west-1", vec![orders]);
    (h, app, runtime, turn)
}

fn endpoint_scope() -> CloudScope {
    CloudScope::Endpoint(CloudKey { account: ACCOUNT.into(), region: "eu-west-1".into(), name: "orders".into() })
}

fn tunnel(host: &str, port: u16) -> CloudSsh {
    CloudSsh { enabled: true, host: host.into(), port, username: "deploy".into(), password: "pw".into(), ..CloudSsh::none() }
}

#[test]
fn an_endpoint_left_to_its_account_says_what_it_inherits_and_can_take_a_tunnel_of_its_own() {
    let (mut h, app, _rt, _turn) = form_ready();
    let account = CloudScope::Account(ACCOUNT.into());
    app.state.settings.lock().set_cloud_scope(&account, "dev", None, Some(tunnel("bastion", 2222))).unwrap();

    let form = h.window.global::<CloudSettings>();
    form.invoke_open_requested(1, ENDPOINT.into());
    assert!(form.get_open());
    assert_eq!((form.get_scope_name().as_str(), form.get_scope_detail().as_str()), ("orders", "eu-west-1"));
    assert_eq!(form.get_inherit_ssh_hint(), "bastion:2222");
    assert_eq!((form.get_profile_mode(), form.get_ssh_mode()), (0, 0), "an endpoint starts on Inherit");

    form.set_ssh_mode(2);
    form.set_ssh_host(" jump ".into());
    form.set_ssh_port("".into());
    form.set_ssh_username("me".into());
    form.invoke_save();
    assert!(wait_for(&mut h, |h| !h.window.global::<CloudSettings>().get_open()), "the form closes once saved");

    let saved = app.state.settings.lock().cloud_settings(&endpoint_scope()).ssh.unwrap();
    assert_eq!((saved.host.as_str(), saved.port), ("jump", 22), "trimmed, on SSH's own port when none is said");
    let listed = app.state.connections.lock().cloud_connections()[0].ssh.clone().expect("the endpoint now goes through it");
    assert_eq!((listed.host.as_str(), listed.username.as_str()), ("jump", "me"));
}

#[test]
fn the_account_form_opens_on_what_it_sets_and_needs_a_signed_in_account() {
    let (h, app, _rt, _turn) = form_ready();
    let account = CloudScope::Account(ACCOUNT.into());
    app.state.settings.lock().set_cloud_scope(&account, "dev", None, Some(tunnel("bastion", 2222))).unwrap();
    let form = h.window.global::<CloudSettings>();
    form.invoke_open_requested(0, "".into());
    assert_eq!((form.get_scope_name().as_str(), form.get_scope_detail().as_str()), ("dev", ACCOUNT));
    assert_eq!((form.get_ssh_mode(), form.get_ssh_host().as_str(), form.get_ssh_port().as_str()), (2, "bastion", "2222"));
    assert_eq!(form.get_ssh_password(), "", "a secret is never shown back");

    form.set_open(false);
    app.state.connections.lock().set_cloud_connections("", "", Vec::new());
    form.invoke_open_requested(0, "".into());
    assert!(!form.get_open(), "signed out: no account to set anything for");
    form.invoke_open_requested(1, ENDPOINT.into());
    assert!(!form.get_open(), "nor an endpoint that's no longer listed");
}

#[test]
fn a_form_that_cant_be_saved_says_why_and_saves_nothing() {
    let (mut h, app, _rt, _turn) = form_ready();
    let form = h.window.global::<CloudSettings>();
    form.invoke_open_requested(1, ENDPOINT.into());

    form.set_ssh_mode(2);
    form.set_ssh_host("jump".into());
    form.set_ssh_port("70000".into());
    form.invoke_save();
    assert!(form.get_error().contains("SSH port"), "{}", form.get_error());

    form.set_ssh_mode(0);
    form.set_profile_mode(2);
    form.invoke_save();
    assert!(form.get_error().contains("connection profile"), "{}", form.get_error());

    // a host left empty is refused by the settings, the answer coming back from another thread
    form.set_error("".into());
    form.set_profile_mode(0);
    form.set_ssh_mode(2);
    form.set_ssh_host("  ".into());
    form.set_ssh_port("22".into());
    form.invoke_save();
    assert!(wait_for(&mut h, |h| !h.window.global::<CloudSettings>().get_error().is_empty()));
    assert!(h.window.global::<CloudSettings>().get_open());
    assert!(app.state.settings.lock().cloud_settings(&endpoint_scope()).ssh.is_none());
}
