//! The AWS side of the cloud account: the local `~/.aws` profiles, signing in
//! (SSO in the browser, see [`sso`], or the SDK's chain) and the ElastiCache
//! Redis groups.

use rustc_hash::FxHashSet;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_credential_types::Credentials;
use aws_credential_types::provider::ProvideCredentials;
use aws_runtime::env_config::file::EnvConfigFiles;
use aws_sdk_sso::error::ProvideErrorMetadata;
use aws_smithy_http_client::tls;
use aws_smithy_runtime_api::client::http::SharedHttpClient;
use aws_types::os_shim_internal::{Env, Fs};

use crate::connection::ConnectionItem;
use crate::connection::store::build_connection_string;
use crate::error::{Error, ErrorKind};

mod sso;

pub use sso::SignInPage;

/// Prefix of a cloud connection's id: `aws:<profile>:<name>`.
pub const ID_PREFIX: &str = "aws:";

#[derive(Debug, Clone, PartialEq)]
struct SsoSettings {
    /// the `sso-session` the settings come from; none for a profile that holds them itself
    session: Option<String>,
    start_url: String,
    region: String,
    /// what the session's `sso_registration_scopes` asks for
    scopes: Vec<String>,
    account_id: String,
    role_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    region: Option<String>,
    sso: Option<SsoSettings>,
}

/// The `.aws` folder of the user, if there is one.
pub fn aws_dir() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".aws");
    dir.is_dir().then_some(dir)
}

/// The profiles of the local `.aws` folder, as the SDK reads them, `default` first.
pub async fn profiles() -> Vec<Profile> {
    profiles_in(&Fs::real(), &Env::real()).await
}

/// The profiles of the `.aws` files `fs` holds where `env` says they are, `default` first.
async fn profiles_in(fs: &Fs, env: &Env) -> Vec<Profile> {
    let Ok(set) = aws_config::profile::load(fs, env, &EnvConfigFiles::default(), None).await else { return Vec::new() };
    let mut profiles: Vec<Profile> = set
        .profiles()
        .filter_map(|name| {
            let values = set.get_profile(name)?;
            // the start URL and region live in the `sso-session` the profile names, or in the profile itself
            let session_name = values.get("sso_session");
            let session = session_name.and_then(|s| set.sso_session(s));
            let pick = |key: &str| session.and_then(|s| s.get(key)).or_else(|| values.get(key)).map(str::to_string);
            let scopes = pick("sso_registration_scopes")
                .map_or_else(Vec::new, |scopes| scopes.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect());
            let sso = match (pick("sso_start_url"), pick("sso_region"), values.get("sso_account_id"), values.get("sso_role_name")) {
                (Some(start_url), Some(region), Some(account_id), Some(role_name)) => Some(SsoSettings {
                    session: session_name.filter(|_| session.is_some()).map(str::to_string),
                    start_url,
                    region,
                    scopes,
                    account_id: account_id.to_string(),
                    role_name: role_name.to_string(),
                }),
                _ => None,
            };
            Some(Profile { region: values.get("region").map(str::to_string), name: name.to_string(), sso })
        })
        .collect();
    profiles.sort_by(|a, b| (a.name != "default", &a.name).cmp(&(b.name != "default", &b.name)));
    profiles
}

fn http_client() -> SharedHttpClient {
    aws_smithy_http_client::Builder::new().tls_provider(tls::Provider::Rustls(tls::rustls_provider::CryptoMode::Ring)).build_https()
}

/// Why signing in to AWS, or listing what an account holds, failed.
#[derive(Debug, thiserror::Error)]
pub enum AwsError {
    #[error("Incomplete answer from AWS.")]
    Incomplete,
    /// a failed call, with AWS's own message ("message (code)")
    #[error("{message}")]
    Service { message: String, source: Box<dyn std::error::Error + Send + Sync> },
    /// `reason` is the SDK's, when it gave one
    #[error("No usable credentials for the profile {profile}: {}", reason.as_deref().unwrap_or("No credentials."))]
    NoUsableCredentials { profile: String, reason: Option<String> },
    #[error("The profile {profile} can't take the role {role} in the account {account}: {source}")]
    RoleRefused { profile: String, role: String, account: String, source: Box<AwsError> },
    #[error("No credentials.")]
    NoCredentials,
    #[error("No AWS profile found in the .aws folder.")]
    NoProfile,
    #[error("AWS profile not found: {0}")]
    ProfileNotFound(String),
    #[error("The profile {0} has no region.")]
    NoRegion(String),
    #[error("The sign-in in the browser took too long.")]
    SignInTimedOut,
    /// what the browser said, when the user or the identity provider said no
    #[error("The sign-in was refused in the browser: {0}")]
    SignInRefused(String),
    #[error("Couldn't wait for the browser's answer: {0}")]
    Callback(#[source] std::io::Error),
    #[error("Couldn't open the browser: {0}")]
    Browser(#[source] std::io::Error),
}

impl AwsError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            AwsError::NoProfile | AwsError::ProfileNotFound(_) => ErrorKind::NotFound,
            AwsError::SignInTimedOut => ErrorKind::Timeout,
            AwsError::Browser(_) | AwsError::Callback(_) => ErrorKind::System,
            _ => ErrorKind::Cloud,
        }
    }
}

fn incomplete<T>(value: Option<T>) -> Result<T, AwsError> {
    value.ok_or(AwsError::Incomplete)
}

/// An AWS call's error, its message "message (code)", falling back to the full error chain.
fn describe<E: std::error::Error + ProvideErrorMetadata + Send + Sync + 'static>(error: E) -> AwsError {
    let message = match (error.message(), error.code()) {
        (Some(message), Some(code)) => format!("{message} ({code})"),
        (Some(text), None) | (None, Some(text)) => text.to_string(),
        (None, None) => describe_chain(&error),
    };
    AwsError::Service { message, source: Box::new(error) }
}

fn describe_chain<E: std::error::Error>(error: &E) -> String {
    aws_sdk_sso::error::DisplayErrorContext(error).to_string()
}

/// A client-side config with no credentials, for the calls that need none.
async fn anonymous_config(region: &str) -> SdkConfig {
    aws_config::defaults(BehaviorVersion::latest())
        .no_credentials()
        .region(Region::new(region.to_string()))
        .http_client(http_client())
        .load()
        .await
}

/// The SDK's config for `profile` if its own chain (keys, an assumed role, an
/// existing `aws sso login`...) yields credentials, else the SDK's reason, if it gave one.
async fn chain_config(profile: &Profile) -> Result<SdkConfig, Option<String>> {
    let config = aws_config::defaults(BehaviorVersion::latest()).profile_name(&profile.name).http_client(http_client()).load().await;
    match config.credentials_provider() {
        Some(provider) => match provider.provide_credentials().await {
            Ok(_) => Ok(config),
            Err(e) => Err(Some(describe_chain(&e))),
        },
        None => Err(None),
    }
}

/// A config for `profile` whose credentials work: the SDK's own chain first,
/// then the profile's SSO role, signing in in the browser if need be.
async fn sign_in(profile: &Profile, page: &SignInPage) -> Result<SdkConfig, Error> {
    let chain_error = match chain_config(profile).await {
        Ok(config) => return Ok(config),
        Err(e) => e,
    };
    let Some(sso) = &profile.sso else {
        return Err(AwsError::NoUsableCredentials { profile: profile.name.clone(), reason: chain_error }.into());
    };

    let access_token = sso::access_token(sso, page).await?;
    let granted = aws_sdk_sso::Client::new(&anonymous_config(&sso.region).await)
        .get_role_credentials()
        .role_name(&sso.role_name)
        .account_id(&sso.account_id)
        .access_token(access_token)
        .send()
        .await
        .map_err(|e| AwsError::RoleRefused {
            profile: profile.name.clone(),
            role: sso.role_name.clone(),
            account: sso.account_id.clone(),
            source: Box::new(describe(e)),
        })?;
    let role = granted.role_credentials().ok_or(AwsError::NoCredentials)?;
    let expires = UNIX_EPOCH + Duration::from_millis(role.expiration().max(0) as u64);
    let credentials = Credentials::new(
        incomplete(role.access_key_id())?,
        incomplete(role.secret_access_key())?,
        role.session_token().map(str::to_string),
        Some(expires),
        "cachemire-sso",
    );

    let region = profile.region.clone().unwrap_or_else(|| sso.region.clone());
    Ok(aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(region))
        .credentials_provider(credentials)
        .http_client(http_client())
        .load()
        .await)
}

/// Signs in like `aws sso login`: reuses a working session, else gets an SSO token
/// in the browser; each profile's role is checked once it is picked. Profiles are
/// tried in turn (the environment's, `default`, the rest), skipping those with neither.
pub async fn login(page: &SignInPage) -> Result<(), Error> {
    let mut profiles = profiles().await;
    let wanted = std::env::var("AWS_PROFILE").ok();
    profiles.sort_by_key(|p| (Some(&p.name) != wanted.as_ref(), p.name != "default"));
    let mut last_error = AwsError::NoProfile;
    for profile in &profiles {
        let chain_error = match chain_config(profile).await {
            Ok(_) => return Ok(()),
            Err(e) => e,
        };
        match &profile.sso {
            Some(sso) => return sso::access_token(sso, page).await.map(drop),
            None => last_error = AwsError::NoUsableCredentials { profile: profile.name.clone(), reason: chain_error },
        }
    }
    Err(last_error.into())
}

/// The URL of an ElastiCache endpoint: `rediss://` when it takes TLS.
fn cloud_url(host: &str, port: u16, tls: bool) -> String {
    let url = build_connection_string(host, port, 0, "", "");
    if tls { url.replacen("redis://", "rediss://", 1) } else { url }
}

/// The connections of a profile's account and region, as listed.
pub struct Listing {
    pub account: String,
    pub region: String,
    pub items: Vec<ConnectionItem>,
}

/// The ElastiCache Redis groups and standalone nodes that `profile` sees, as
/// connections. Memcached (and Valkey) clusters are left out.
pub async fn redis_connections(profile_name: &str, page: &SignInPage) -> Result<Listing, Error> {
    let profile =
        profiles().await.into_iter().find(|p| p.name == profile_name).ok_or_else(|| AwsError::ProfileNotFound(profile_name.to_string()))?;
    let config = sign_in(&profile, page).await?;
    if config.region().is_none() {
        return Err(AwsError::NoRegion(profile_name.to_string()).into());
    }
    let region = config.region().map(|r| r.to_string()).unwrap_or_default();
    let account = aws_sdk_sts::Client::new(&config)
        .get_caller_identity()
        .send()
        .await
        .map_err(describe)?
        .account()
        .map(str::to_string)
        .ok_or(AwsError::NoCredentials)?;
    let client = aws_sdk_elasticache::Client::new(&config);

    let item = |name: &str, endpoint: Option<&aws_sdk_elasticache::types::Endpoint>, tls: bool| {
        let endpoint = endpoint?;
        Some(ConnectionItem {
            id: format!("{ID_PREFIX}{profile_name}:{name}"),
            name: name.to_string(),
            connection_url: cloud_url(endpoint.address()?, endpoint.port().and_then(|p| u16::try_from(p).ok()).unwrap_or(6379), tls),
            username: String::new(),
            password: String::new(),
            ssh: None,
            profile_id: String::new(),
        })
    };

    let mut items = Vec::new();
    let mut groups = FxHashSet::default();
    let mut marker: Option<String> = None;
    loop {
        let page = client.describe_cache_clusters().show_cache_node_info(true).set_marker(marker.take()).send().await.map_err(describe)?;
        for cluster in page.cache_clusters() {
            if cluster.engine() != Some("redis") {
                continue;
            }
            match cluster.replication_group_id() {
                Some(group) => {
                    groups.insert(group.to_string());
                }
                None => {
                    let endpoint = cluster.cache_nodes().first().and_then(|n| n.endpoint());
                    items.extend(item(
                        cluster.cache_cluster_id().unwrap_or_default(),
                        endpoint,
                        cluster.transit_encryption_enabled().unwrap_or(false),
                    ));
                }
            }
        }
        marker = page.marker().map(str::to_string);
        if marker.is_none() {
            break;
        }
    }

    let mut marker: Option<String> = None;
    loop {
        let page = client.describe_replication_groups().set_marker(marker.take()).send().await.map_err(describe)?;
        for group in page.replication_groups() {
            let id = group.replication_group_id().unwrap_or_default();
            if !groups.contains(id) {
                continue;
            }
            let endpoint = group.configuration_endpoint().or_else(|| group.node_groups().first().and_then(|n| n.primary_endpoint()));
            items.extend(item(id, endpoint, group.transit_encryption_enabled().unwrap_or(false)));
        }
        marker = page.marker().map(str::to_string);
        if marker.is_none() {
            break;
        }
    }

    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Listing { account, region, items })
}

#[cfg(test)]
mod tests;
