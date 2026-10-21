//! The SSO access token, kept like `aws sso login` keeps it, in `~/.aws/sso/cache`: the cached
//! one while it lasts, refreshed when it can be, else granted in the browser (OAuth
//! authorization code with PKCE, the browser's answer caught on a local port).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::{AwsError, SsoSettings, anonymous_config, describe, incomplete};
use crate::error::Error;

/// What the browser tab shows once the sign-in is over, in the interface language.
pub struct SignInPage {
    pub title: String,
    pub signed_in: String,
    pub refused: String,
}

/// The scope `aws sso login` asks for when the `sso-session` names none.
const DEFAULT_SCOPE: &str = "sso:account:access";
const CALLBACK_PATH: &str = "/oauth/callback";
/// How long the browser has to send its answer, as for `aws sso login`.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);
/// A token this close to its end is taken as gone.
const EXPIRY_MARGIN: i64 = 60;

/// A cache file, as the AWS CLI and SDKs write and read it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedToken {
    start_url: String,
    region: String,
    access_token: String,
    expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    registration_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn timestamp(secs: i64) -> String {
    DateTime::from_secs(secs).fmt(Format::DateTime).unwrap_or_default()
}

/// Whether `at` (an RFC 3339 time) is still more than a minute away.
fn ahead(at: Option<&str>) -> bool {
    at.and_then(|at| DateTime::from_str(at, Format::DateTime).ok()).is_some_and(|at| at.secs() > now() + EXPIRY_MARGIN)
}

/// `~/.aws/sso/cache/<sha1>.json`, the hash being of the session's name, or of the start URL for a
/// profile with no `sso-session`.
fn cache_path(sso: &SsoSettings) -> Option<PathBuf> {
    let key = sso.session.as_deref().unwrap_or(&sso.start_url);
    let hash: String = Sha1::digest(key.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Some(dirs::home_dir()?.join(".aws").join("sso").join("cache").join(format!("{hash}.json")))
}

fn read_cache(path: &Path) -> Option<CachedToken> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Writes the token through a temporary file, readable by its owner only (it holds credentials).
fn write_cache(path: &Path, token: &CachedToken) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(serde_json::to_string_pretty(token).map_err(std::io::Error::other)?.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

/// An access token for `sso`'s start URL: the cached one, a refreshed one, or a new one the
/// user grants in the browser, which then shows `page`. A new token goes to the cache.
pub(super) async fn access_token(sso: &SsoSettings, page: &SignInPage) -> Result<String, Error> {
    let path = cache_path(sso);
    let cached = path.as_deref().and_then(read_cache).filter(|c| c.start_url == sso.start_url && c.region == sso.region);
    if let Some(cached) = cached.as_ref().filter(|c| ahead(Some(&c.expires_at))) {
        return Ok(cached.access_token.clone());
    }

    let client = aws_sdk_ssooidc::Client::new(&anonymous_config(&sso.region).await);
    let token = match cached {
        Some(cached) => match refresh(&client, cached).await {
            Some(token) => token,
            None => browser_sign_in(&client, sso, page).await?,
        },
        None => browser_sign_in(&client, sso, page).await?,
    };
    if let Some(path) = &path
        && let Err(error) = write_cache(path, &token)
    {
        tracing::warn!("couldn't keep the AWS sign-in in {}: {error}", path.display());
    }
    Ok(token.access_token)
}

/// The cached token renewed with its refresh token, while its client registration lasts.
async fn refresh(client: &aws_sdk_ssooidc::Client, cached: CachedToken) -> Option<CachedToken> {
    let (Some(client_id), Some(client_secret), Some(refresh_token)) = (&cached.client_id, &cached.client_secret, &cached.refresh_token)
    else {
        return None;
    };
    if !ahead(cached.registration_expires_at.as_deref()) {
        return None;
    }
    let renewed = client
        .create_token()
        .client_id(client_id)
        .client_secret(client_secret)
        .grant_type("refresh_token")
        .refresh_token(refresh_token)
        .send()
        .await
        .inspect_err(|error| tracing::info!("the AWS sign-in couldn't be refreshed: {}", describe_text(error)))
        .ok()?;
    Some(CachedToken {
        access_token: renewed.access_token()?.to_string(),
        expires_at: timestamp(now() + i64::from(renewed.expires_in())),
        refresh_token: renewed.refresh_token().map(str::to_string).or(cached.refresh_token.clone()),
        ..cached
    })
}

fn describe_text<E: std::error::Error + aws_sdk_ssooidc::error::ProvideErrorMetadata>(error: &E) -> String {
    error.message().map_or_else(|| error.to_string(), str::to_string)
}

/// Signs in in the browser: registers this app as a client, opens the authorization page with
/// a PKCE challenge, waits for the browser to come back on a local port with the code, and
/// trades that code for a token.
async fn browser_sign_in(client: &aws_sdk_ssooidc::Client, sso: &SsoSettings, page: &SignInPage) -> Result<CachedToken, Error> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(AwsError::Callback)?;
    let port = listener.local_addr().map_err(AwsError::Callback)?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let scopes = if sso.scopes.is_empty() { vec![DEFAULT_SCOPE.to_string()] } else { sso.scopes.clone() };

    let registration = client
        .register_client()
        .client_name("cachemire")
        .client_type("public")
        .grant_types("authorization_code")
        .grant_types("refresh_token")
        .redirect_uris(format!("http://127.0.0.1{CALLBACK_PATH}"))
        .issuer_url(&sso.start_url)
        .set_scopes(Some(scopes.clone()))
        .send()
        .await
        .map_err(describe)?;
    let client_id = incomplete(registration.client_id())?.to_string();
    let client_secret = incomplete(registration.client_secret())?.to_string();

    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|e| AwsError::Callback(std::io::Error::other(e)))?;
    let verifier = URL_SAFE_NO_PAD.encode(random);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = uuid::Uuid::new_v4().to_string();
    let mut url = url::Url::parse(&format!("https://oidc.{}.amazonaws.com/authorize", sso.region)).map_err(|_| AwsError::Incomplete)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("state", &state)
        .append_pair("code_challenge_method", "S256")
        .append_pair("scopes", &scopes.join(" "))
        .append_pair("code_challenge", &challenge);
    open::that(url.as_str()).map_err(AwsError::Browser)?;

    let code =
        tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(&listener, &state, page)).await.map_err(|_| AwsError::SignInTimedOut)??;
    let token = client
        .create_token()
        .client_id(&client_id)
        .client_secret(&client_secret)
        .grant_type("authorization_code")
        .redirect_uri(&redirect_uri)
        .code_verifier(&verifier)
        .code(code)
        .send()
        .await
        .map_err(describe)?;
    Ok(CachedToken {
        start_url: sso.start_url.clone(),
        region: sso.region.clone(),
        access_token: incomplete(token.access_token())?.to_string(),
        expires_at: timestamp(now() + i64::from(token.expires_in())),
        client_id: Some(client_id),
        client_secret: Some(client_secret),
        registration_expires_at: Some(timestamp(registration.client_secret_expires_at())),
        refresh_token: token.refresh_token().map(str::to_string),
    })
}

/// Answers the browser until it comes back with the code for `state`, or with a refusal.
async fn wait_for_code(listener: &TcpListener, state: &str, page: &SignInPage) -> Result<String, Error> {
    loop {
        let (mut socket, _) = listener.accept().await.map_err(AwsError::Callback)?;
        // a connection that sends nothing (a browser's preconnect) mustn't hold up the next one
        let Ok(Some(params)) = tokio::time::timeout(Duration::from_secs(10), callback_params(&mut socket)).await else {
            respond(&mut socket, "404 Not Found", "").await;
            continue;
        };
        if let Some(error) = params.get("error") {
            respond(&mut socket, "200 OK", &html(&page.title, &page.refused)).await;
            let reason = params.get("error_description").unwrap_or(error);
            return Err(AwsError::SignInRefused(reason.clone()).into());
        }
        match params.get("code") {
            Some(code) if params.get("state").map(String::as_str) == Some(state) => {
                respond(&mut socket, "200 OK", &html(&page.title, &page.signed_in)).await;
                return Ok(code.clone());
            }
            _ => respond(&mut socket, "400 Bad Request", "").await,
        }
    }
}

/// The query of a request for the callback path; none for any other request.
async fn callback_params(socket: &mut TcpStream) -> Option<HashMap<String, String>> {
    let mut head = Vec::new();
    let mut buffer = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&buffer[..read]);
    }
    let head = String::from_utf8_lossy(&head);
    let target = head.lines().next()?.split(' ').nth(1)?;
    let url = url::Url::parse("http://127.0.0.1").ok()?.join(target).ok()?;
    (url.path() == CALLBACK_PATH).then(|| url.query_pairs().into_owned().collect())
}

async fn respond(socket: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

fn html(title: &str, message: &str) -> String {
    let escape = |text: &str| text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head>\
         <body style=\"font-family:sans-serif;display:grid;place-items:center;min-height:90vh\"><p>{}</p></body></html>",
        escape(title),
        escape(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block_on, temp_dir};

    fn sso(session: Option<&str>, start_url: &str) -> SsoSettings {
        SsoSettings {
            session: session.map(str::to_string),
            start_url: start_url.into(),
            region: "eu-west-1".into(),
            scopes: Vec::new(),
            account_id: "111".into(),
            role_name: "ReadOnly".into(),
        }
    }

    fn page() -> SignInPage {
        SignInPage { title: "Cachemire".into(), signed_in: "You're signed in.".into(), refused: "Sign-in refused.".into() }
    }

    /// Sends `request_line` as the browser would to `port`, and reads the whole answer.
    async fn browse(port: u16, request_line: &str) -> String {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        socket.write_all(format!("{request_line}\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).await.unwrap();
        let mut answer = String::new();
        socket.read_to_string(&mut answer).await.unwrap();
        answer
    }

    #[test]
    fn the_cache_file_is_the_one_the_aws_cli_shares_for_the_session_or_the_start_url() {
        let by_session = cache_path(&sso(Some("corp"), "https://corp.awsapps.com/start")).unwrap();
        assert!(by_session.ends_with(Path::new(".aws/sso/cache/ee0bfd2552fbd840c02cc48b6e823320543c450f.json")), "{by_session:?}");
        let legacy = cache_path(&sso(None, "https://old.awsapps.com/start")).unwrap();
        assert!(legacy.ends_with("33d5ca4bf3b1a62edb96b47f7c203034a43ac435.json"), "{legacy:?}");
    }

    #[test]
    fn a_token_written_by_the_aws_cli_is_read_and_one_kept_here_reads_back_the_same() {
        let dir = temp_dir("sso-cache");
        let path = dir.join("sso").join("cache").join("token.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"startUrl":"https://corp.awsapps.com/start","region":"eu-west-1","accessToken":"abc","expiresAt":"2030-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let from_cli = read_cache(&path).expect("the AWS CLI's own format");
        assert_eq!((from_cli.access_token.as_str(), from_cli.refresh_token), ("abc", None));

        let token = CachedToken { refresh_token: Some("again".into()), client_id: Some("me".into()), ..from_cli };
        write_cache(&path, &token).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("\"refreshToken\"") && !written.contains("clientSecret"), "camelCase, the absent left out: {written}");
        let back = read_cache(&path).unwrap();
        assert_eq!((back.refresh_token.as_deref(), back.client_id.as_deref()), (Some("again"), Some("me")));
        assert!(!path.with_extension("json.tmp").exists(), "the temporary file is renamed over the cache");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "credentials stay the owner's");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_damaged_or_missing_cache_is_no_token() {
        let dir = temp_dir("sso-cache-damaged");
        std::fs::write(dir.join("half.json"), r#"{"startUrl":"https://x","accessToken":"abc"}"#).unwrap();
        assert!(read_cache(&dir.join("half.json")).is_none());
        assert!(read_cache(&dir.join("absent.json")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_token_about_to_expire_counts_as_gone() {
        assert!(ahead(Some(&timestamp(now() + 3600))));
        assert!(!ahead(Some(&timestamp(now() + 30))), "within the last minute");
        assert!(!ahead(Some(&timestamp(now() - 3600))));
        assert!(!ahead(Some("not a time")) && !ahead(None), "an unknown end is no end to rely on");
    }

    #[test]
    fn the_browser_comes_back_with_the_code_once_its_state_matches() {
        block_on(async {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let page = page();
            let browser = async {
                let elsewhere = browse(port, "GET /favicon.ico HTTP/1.1").await;
                let forged = browse(port, "GET /oauth/callback?code=stolen&state=other HTTP/1.1").await;
                let ours = browse(port, "GET /oauth/callback?code=the-code&state=s1 HTTP/1.1").await;
                (elsewhere, forged, ours)
            };
            let (code, (elsewhere, forged, ours)) = tokio::join!(wait_for_code(&listener, "s1", &page), browser);
            assert_eq!(code.unwrap(), "the-code");
            assert!(elsewhere.starts_with("HTTP/1.1 404"), "{elsewhere}");
            assert!(forged.starts_with("HTTP/1.1 400"), "another sign-in's answer is turned down: {forged}");
            assert!(ours.starts_with("HTTP/1.1 200") && ours.contains("You're signed in."), "{ours}");
        });
    }

    #[test]
    fn a_refusal_in_the_browser_ends_the_sign_in_with_its_reason() {
        block_on(async {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let page = page();
            let browser = browse(port, "GET /oauth/callback?error=access_denied&error_description=User%20said%20no HTTP/1.1");
            let (result, answer) = tokio::join!(wait_for_code(&listener, "s1", &page), browser);
            assert!(matches!(result, Err(Error::Cloud(AwsError::SignInRefused(ref reason))) if reason == "User said no"), "{result:?}");
            assert!(answer.contains("Sign-in refused."), "{answer}");
        });
    }

    #[test]
    fn the_sign_in_page_shows_its_texts_as_text() {
        let page = html("<Cachemire>", "Tom & Jerry");
        assert!(page.contains("<title>&lt;Cachemire&gt;</title>") && page.contains("<p>Tom &amp; Jerry</p>"), "{page}");
    }
}
