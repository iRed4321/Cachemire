use super::*;

fn form() -> ConnectionForm {
    ConnectionForm {
        id: "".into(),
        name: "prod".into(),
        connection_url: "redis://10.0.0.5:6379".into(),
        username: "".into(),
        password: "".into(),
        ssh_enabled: true,
        ssh_host: "bastion".into(),
        ssh_port: "2222".into(),
        ssh_username: "deploy".into(),
        ssh_password: "".into(),
        ssh_key: "~/.ssh/id_ed25519".into(),
        ssh_passphrase: "".into(),
        profile_id: "production".into(),
    }
}

#[test]
fn a_blank_password_box_keeps_the_stored_password_and_everything_else_is_taken_as_typed() {
    let payload = payload_from_form(&form()).unwrap();
    assert_eq!((payload.password, payload.ssh_password, payload.ssh_passphrase), (None, None, None));
    assert_eq!((payload.ssh_enabled, payload.ssh_port), (Some(true), Some(2222)));
    assert_eq!(payload.ssh_private_key.as_deref(), Some("~/.ssh/id_ed25519"));
    assert_eq!(payload.profile_id.as_deref(), Some("production"));
    assert_eq!(payload_from_form(&ConnectionForm { profile_id: "".into(), ..form() }).unwrap().profile_id.as_deref(), Some(""), "no profile is an empty one, not a kept one");

    let typed = payload_from_form(&ConnectionForm { password: "a".into(), ssh_password: "b".into(), ssh_passphrase: "c".into(), ..form() }).unwrap();
    assert_eq!((typed.password.as_deref(), typed.ssh_password.as_deref(), typed.ssh_passphrase.as_deref()), (Some("a"), Some("b"), Some("c")));
}

#[test]
fn the_ssh_port_defaults_to_22_and_must_be_a_number() {
    assert_eq!(payload_from_form(&ConnectionForm { ssh_port: "".into(), ..form() }).unwrap().ssh_port, Some(22));
    assert_eq!(payload_from_form(&ConnectionForm { ssh_port: " 2200 ".into(), ..form() }).unwrap().ssh_port, Some(2200));
    for bad in ["ssh", "0", "65536", "-1", "70000"] {
        assert!(payload_from_form(&ConnectionForm { ssh_port: bad.into(), ..form() }).is_err(), "{bad}");
    }
}
