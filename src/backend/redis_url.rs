use percent_encoding::percent_decode_str;

/// What a `redis://[user[:password]@]host[:port][/db]` URL spells out; the
/// scheme is optional (`host:port` alone works), `rediss://` means TLS. Fields
/// the URL doesn't say are left for the caller to default.
pub struct UrlParts {
    pub tls: bool,
    pub host: String,
    pub port: Option<u16>,
    pub db: Option<i64>,
    pub username: String,
    pub password: String,
}

pub fn parse(value: &str) -> Option<UrlParts> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let uri = if value.contains("://") {
        url::Url::parse(value)
    } else {
        url::Url::parse(&format!("redis://{value}"))
    }
    .ok()?;

    let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
    Some(UrlParts {
        tls: uri.scheme().eq_ignore_ascii_case("rediss"),
        host: uri.host_str().unwrap_or_default().to_string(),
        port: uri.port(),
        db: uri.path().trim_matches('/').parse().ok(),
        username: decode(uri.username()),
        password: uri.password().map(decode).unwrap_or_default(),
    })
}
