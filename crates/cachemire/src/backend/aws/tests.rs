use super::*;
use crate::test_support::block_on;

/// The profiles of an `.aws/config` holding `config`.
fn profiles_of(config: &str) -> Vec<Profile> {
    let fs = Fs::from_slice(&[("/home/u/.aws/config", config)]);
    let env = Env::from_slice(&[("HOME", "/home/u")]);
    block_on(profiles_in(&fs, &env))
}

fn sso(start_url: &str, region: &str, account_id: &str, role_name: &str) -> Option<SsoSettings> {
    Some(SsoSettings { start_url: start_url.into(), region: region.into(), account_id: account_id.into(), role_name: role_name.into() })
}

#[test]
fn default_comes_first_then_the_others_by_name() {
    let names: Vec<String> = profiles_of("[profile zeta]\nregion = eu-west-1\n[profile alpha]\n[default]\nregion = us-east-1\n").into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["default", "alpha", "zeta"]);
}

#[test]
fn an_sso_profile_takes_its_start_url_and_region_from_its_session() {
    let config = "[profile dev]\nsso_session = corp\nsso_account_id = 111\nsso_role_name = ReadOnly\nregion = eu-west-3\n\
                  [sso-session corp]\nsso_start_url = https://corp.awsapps.com/start\nsso_region = eu-west-1\n";
    let dev = &profiles_of(config)[0];
    assert_eq!(dev.sso, sso("https://corp.awsapps.com/start", "eu-west-1", "111", "ReadOnly"));
    assert_eq!(dev.region.as_deref(), Some("eu-west-3"), "where its caches are, not where SSO signs in");
}

#[test]
fn a_legacy_sso_profile_holds_its_settings_itself() {
    let config = "[profile old]\nsso_start_url = https://old.awsapps.com/start\nsso_region = us-east-1\nsso_account_id = 222\nsso_role_name = Admin\n";
    assert_eq!(profiles_of(config)[0].sso, sso("https://old.awsapps.com/start", "us-east-1", "222", "Admin"));
}

#[test]
fn a_profile_missing_part_of_its_sso_settings_is_no_sso_profile() {
    let config = "[profile keys]\nregion = eu-west-1\n[profile half]\nsso_start_url = https://x/start\nsso_region = eu-west-1\nsso_account_id = 333\n";
    let profiles = profiles_of(config);
    assert!(profiles.iter().all(|p| p.sso.is_none()), "{profiles:?}");
}

#[test]
fn no_aws_folder_means_no_profile() {
    let env = Env::from_slice(&[("HOME", "/home/u")]);
    assert!(block_on(profiles_in(&Fs::from_slice(&[]), &env)).is_empty());
}

#[test]
fn an_endpoint_that_takes_tls_gets_a_rediss_url() {
    assert_eq!(cloud_url("cache.abc.euw1.cache.amazonaws.com", 6379, true), "rediss://cache.abc.euw1.cache.amazonaws.com:6379");
    assert_eq!(cloud_url("10.0.0.5", 6380, false), "redis://10.0.0.5:6380");
}
