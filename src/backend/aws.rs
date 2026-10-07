//! The AWS side of the cloud account: the local `~/.aws` profiles, signing in
//! (browser authorization for SSO, the SDK's chain otherwise) and the ElastiCache
//! Redis groups.

use rustc_hash::{FxHashMap, FxHashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use aws_runtime::env_config::file::EnvConfigFiles;
use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_credential_types::Credentials;
use aws_credential_types::provider::ProvideCredentials;
use aws_smithy_http_client::tls;
use aws_sdk_sso::error::ProvideErrorMetadata;
use aws_smithy_runtime_api::client::http::SharedHttpClient;
use aws_types::os_shim_internal::{Env, Fs};
use rust_i18n::t;

use super::LockExt;
use super::connection_store::build_connection_string;
use super::error::AppError;
use super::models::ConnectionItem;

/// Prefix of a cloud connection's id: `aws:<profile>:<name>`.
pub const ID_PREFIX: &str = "aws:";

#[derive(Debug, Clone, PartialEq)]
struct SsoSettings {
    start_url: String,
    region: String,
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
    let (fs, env) = (Fs::real(), Env::real());
    let Ok(set) = aws_config::profile::load(&fs, &env, &EnvConfigFiles::default(), None).await else { return Vec::new() };
    let mut profiles: Vec<Profile> = set
        .profiles()
        .filter_map(|name| {
            let values = set.get_profile(name)?;
            // the start URL and region live in the `sso-session` the profile names, or in the profile itself
            let session = values.get("sso_session").and_then(|s| set.sso_session(s));
            let pick = |key: &str| session.and_then(|s| s.get(key)).or_else(|| values.get(key)).map(str::to_string);
            let sso = match (pick("sso_start_url"), pick("sso_region"), values.get("sso_account_id"), values.get("sso_role_name")) {
                (Some(start_url), Some(region), Some(account_id), Some(role_name)) => {
                    Some(SsoSettings { start_url, region, account_id: account_id.to_string(), role_name: role_name.to_string() })
                }
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

fn incomplete<T>(value: Option<T>) -> Result<T, AppError> {
    value.ok_or_else(|| AppError::Cloud(t!("Incomplete answer from AWS.").into_owned()))
}

/// An AWS call's error as "message (code)", falling back to the full error chain.
fn describe<E: std::error::Error + ProvideErrorMetadata>(error: &E) -> AppError {
    AppError::Cloud(match (error.message(), error.code()) {
        (Some(message), Some(code)) => format!("{message} ({code})"),
        (Some(text), None) | (None, Some(text)) => text.to_string(),
        (None, None) => describe_chain(error),
    })
}

fn describe_chain<E: std::error::Error>(error: &E) -> String {
    aws_sdk_sso::error::DisplayErrorContext(error).to_string()
}

/// A client-side config with no credentials, for the calls that need none.
async fn anonymous_config(region: &str) -> SdkConfig {
    aws_config::defaults(BehaviorVersion::latest()).no_credentials().region(Region::new(region.to_string())).http_client(http_client()).load().await
}

struct SsoToken {
    access_token: String,
    expires: Instant,
}

/// The access tokens of the SSO sign-ins made during this run, by start URL and region.
fn tokens() -> &'static Mutex<FxHashMap<String, SsoToken>> {
    static TOKENS: OnceLock<Mutex<FxHashMap<String, SsoToken>>> = OnceLock::new();
    TOKENS.get_or_init(Default::default)
}

/// An SSO access token: the one already held, or a new one the user grants in
/// the browser (OAuth device authorization, the flow `aws sso login` runs).
async fn sso_access_token(sso: &SsoSettings) -> Result<String, AppError> {
    let key = format!("{}|{}", sso.start_url, sso.region);
    if let Some(token) = tokens().lock_recover().get(&key).filter(|t| t.expires > Instant::now() + Duration::from_secs(60)) {
        return Ok(token.access_token.clone());
    }

    let client = aws_sdk_ssooidc::Client::new(&anonymous_config(&sso.region).await);
    let registration = client.register_client().client_name("cachemire").client_type("public").send().await.map_err(|e| describe(&e))?;
    let (client_id, client_secret) = (incomplete(registration.client_id())?.to_string(), incomplete(registration.client_secret())?.to_string());
    let authorization = client
        .start_device_authorization()
        .client_id(&client_id)
        .client_secret(&client_secret)
        .start_url(&sso.start_url)
        .send()
        .await
        .map_err(|e| describe(&e))?;

    let url = incomplete(authorization.verification_uri_complete().or(authorization.verification_uri()))?;
    open::that(url).map_err(|e| AppError::System(t!("Couldn't open the browser: %{error}", error = e.to_string()).into_owned()))?;

    let deadline = Instant::now() + Duration::from_secs(authorization.expires_in().max(60) as u64);
    let mut interval = Duration::from_secs(authorization.interval().max(1) as u64);
    loop {
        tokio::time::sleep(interval).await;
        let created = client
            .create_token()
            .client_id(&client_id)
            .client_secret(&client_secret)
            .grant_type("urn:ietf:params:oauth:grant-type:device_code")
            .device_code(incomplete(authorization.device_code())?)
            .send()
            .await;
        match created {
            Ok(token) => {
                let held = SsoToken { access_token: incomplete(token.access_token())?.to_string(), expires: Instant::now() + Duration::from_secs(token.expires_in().max(0) as u64) };
                let access_token = held.access_token.clone();
                tokens().lock_recover().insert(key, held);
                return Ok(access_token);
            }
            Err(e) => match e.as_service_error() {
                Some(service) if service.is_authorization_pending_exception() => {}
                Some(service) if service.is_slow_down_exception() => interval += Duration::from_secs(5),
                _ => return Err(describe(&e)),
            },
        }
        if Instant::now() > deadline {
            return Err(AppError::Timeout(t!("The sign-in in the browser took too long.").into_owned()));
        }
    }
}

/// A config for `profile` whose credentials work: the SDK's own chain first
/// (keys, an assumed role, an existing `aws sso login`...), then an SSO sign-in.
async fn sign_in(profile: &Profile) -> Result<SdkConfig, AppError> {
    let config = aws_config::defaults(BehaviorVersion::latest()).profile_name(&profile.name).http_client(http_client()).load().await;
    let chain_error = match config.credentials_provider() {
        Some(provider) => match provider.provide_credentials().await {
            Ok(_) => return Ok(config),
            Err(e) => describe_chain(&e),
        },
        None => t!("No credentials.").into_owned(),
    };
    let Some(sso) = &profile.sso else {
        return Err(AppError::Cloud(t!("No usable credentials for the profile %{name}: %{error}", name = profile.name.as_str(), error = chain_error).into_owned()));
    };

    let access_token = sso_access_token(sso).await?;
    let granted = aws_sdk_sso::Client::new(&anonymous_config(&sso.region).await)
        .get_role_credentials()
        .role_name(&sso.role_name)
        .account_id(&sso.account_id)
        .access_token(access_token)
        .send()
        .await
        .map_err(|e| describe(&e))?;
    let role = granted.role_credentials().ok_or_else(|| AppError::Cloud(t!("No credentials.").into_owned()))?;
    let expires = UNIX_EPOCH + Duration::from_millis(role.expiration().max(0) as u64);
    let credentials = Credentials::new(incomplete(role.access_key_id())?, incomplete(role.secret_access_key())?, role.session_token().map(str::to_string), Some(expires), "cachemire-sso");

    let region = profile.region.clone().unwrap_or_else(|| sso.region.clone());
    Ok(aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(region))
        .credentials_provider(credentials)
        .http_client(http_client())
        .load()
        .await)
}

/// Signs in like `aws login`: reuses a working session, else opens the browser.
/// Profiles are tried in turn (the environment's, `default`, the rest); one
/// lacking what it needs to sign in is skipped.
pub async fn login() -> Result<(), AppError> {
    let mut profiles = profiles().await;
    let wanted = std::env::var("AWS_PROFILE").ok();
    profiles.sort_by_key(|p| (Some(&p.name) != wanted.as_ref(), p.name != "default"));
    let mut last_error = AppError::not_found(t!("No AWS profile found in the .aws folder."));
    for profile in &profiles {
        match sign_in(profile).await {
            Ok(_) => return Ok(()),
            // no SSO settings and no working credentials: nothing to ask the user for
            Err(e) if profile.sso.is_none() => last_error = e,
            Err(e) => return Err(e),
        }
    }
    Err(last_error)
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
pub async fn redis_connections(profile_name: &str) -> Result<Listing, AppError> {
    let profile = profiles().await.into_iter().find(|p| p.name == profile_name).ok_or_else(|| AppError::not_found(t!("AWS profile not found: %{name}", name = profile_name)))?;
    let config = sign_in(&profile).await?;
    if config.region().is_none() {
        return Err(AppError::Cloud(t!("The profile %{name} has no region.", name = profile_name).into_owned()));
    }
    let region = config.region().map(|r| r.to_string()).unwrap_or_default();
    let account = aws_sdk_sts::Client::new(&config)
        .get_caller_identity()
        .send()
        .await
        .map_err(|e| describe(&e))?
        .account()
        .map(str::to_string)
        .ok_or_else(|| AppError::Cloud(t!("No credentials.").into_owned()))?;
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
        let page = client.describe_cache_clusters().show_cache_node_info(true).set_marker(marker.take()).send().await.map_err(|e| describe(&e))?;
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
                    items.extend(item(cluster.cache_cluster_id().unwrap_or_default(), endpoint, cluster.transit_encryption_enabled().unwrap_or(false)));
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
        let page = client.describe_replication_groups().set_marker(marker.take()).send().await.map_err(|e| describe(&e))?;
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
