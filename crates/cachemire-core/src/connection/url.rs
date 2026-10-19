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
    let uri = if value.contains("://") { url::Url::parse(value) } else { url::Url::parse(&format!("redis://{value}")) }.ok()?;

    let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
    Some(UrlParts {
        tls: uri.scheme().eq_ignore_ascii_case("rediss"),
        // an IPv6 address without the brackets the URL writes it in
        host: match uri.host() {
            Some(url::Host::Ipv6(address)) => address.to_string(),
            _ => uri.host_str().unwrap_or_default().to_string(),
        },
        port: uri.port(),
        db: uri.path().trim_matches('/').parse().ok(),
        username: decode(uri.username()),
        password: uri.password().map(decode).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(url: &str) -> (bool, String, Option<u16>, Option<i64>, String, String) {
        let p = parse(url).unwrap_or_else(|| panic!("{url} wasn't understood"));
        (p.tls, p.host, p.port, p.db, p.username, p.password)
    }

    #[test]
    fn every_part_of_a_full_url_is_read() {
        assert_eq!(
            parts("rediss://app:s3cret@cache.example:6380/4"),
            (true, "cache.example".into(), Some(6380), Some(4), "app".into(), "s3cret".into())
        );
        assert_eq!(parts("  redis://cache.example  "), (false, "cache.example".into(), None, None, String::new(), String::new()));
    }

    #[test]
    fn the_scheme_may_be_left_out() {
        assert_eq!(parts("cache.example:6390/2"), (false, "cache.example".into(), Some(6390), Some(2), String::new(), String::new()));
        assert_eq!(parts("10.0.0.7").1, "10.0.0.7");
    }

    #[test]
    fn an_ipv6_host_comes_without_its_brackets() {
        assert_eq!(parts("redis://[::1]:6379").1, "::1");
        assert_eq!(parts("rediss://[2001:db8::7]/1").1, "2001:db8::7");
    }

    #[test]
    fn credentials_are_percent_decoded() {
        let (.., username, password) = parts("redis://us%40er:p%40ss%3Aw0rd@h");
        assert_eq!((username.as_str(), password.as_str()), ("us@er", "p@ss:w0rd"));
        assert_eq!(parts("redis://:only-a-password@h").5, "only-a-password");
        assert_eq!(parts("redis://:p@ss@h").5, "p@ss", "an @ in the password needn't be encoded");
    }

    #[test]
    fn the_scheme_is_read_whatever_its_case() {
        assert!(parts("REDISS://h").0);
        assert!(!parts("Redis://h").0);
    }

    #[test]
    fn a_database_that_isnt_a_number_is_left_unsaid() {
        assert_eq!(parts("redis://h/").3, None);
        assert_eq!(parts("redis://h/abc").3, None);
        assert_eq!(parts("redis://h/7/").3, Some(7));
    }

    #[test]
    fn what_cant_be_a_url_is_refused() {
        for url in ["", "   ", "redis://h:99999", "redis://h:port"] {
            assert!(parse(url).is_none(), "{url:?} was understood");
        }
    }
}
