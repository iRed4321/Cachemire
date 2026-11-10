//! End-to-end tests of the core against a real Redis server, each on an emptied database of
//! its own (see [`TestRedis`]). A plain `cargo test` skips them: `cargo xtask e2e` runs them,
//! by area: [`connection`], [`browse`], [`read`], [`edit`], [`search`] and [`redisql`].

mod browse;
mod connection;
mod edit;
mod read;
mod redisql;
mod search;

use crate::key_name::KeyName;
use crate::test_support::TestRedis;

thread_local! {
    static RUNTIME: tokio::runtime::Runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("test runtime");
}

/// Runs `future` on the test thread's runtime, which outlives each call: the pool a state
/// opens keeps its connections' tasks, so the next call can use it.
pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.with(|runtime| runtime.block_on(future))
}

pub(crate) fn key(id: &str) -> KeyName {
    KeyName::from_id(id)
}

/// A URL logging in to the test's database as an ACL user of its own, allowed what `rules`
/// say (`["~*", "+@all", "-@scripting"]`).
pub(crate) fn acl_user(redis: &mut TestRedis, name: &str, rules: &[&str]) -> String {
    let database = redis.url.rsplit('/').next().unwrap_or_default().to_string();
    let username = format!("e2e-{database}-{name}");
    let password = "e2e-secret";
    let login = format!(">{password}");
    let mut args = vec!["ACL", "SETUSER", username.as_str(), "reset", "on", login.as_str()];
    args.extend_from_slice(rules);
    redis.run(&args);
    redis.url_as(&username, password)
}

/// `count` arguments made by `make`, for one command that writes many entries at once.
pub(crate) fn many(count: usize, make: impl Fn(usize) -> Vec<String>) -> Vec<String> {
    (0..count).flat_map(make).collect()
}

/// `command` followed by `args`, as [`TestRedis::run`] takes it.
pub(crate) fn with_args<'a>(command: &[&'a str], args: &'a [String]) -> Vec<&'a str> {
    command.iter().copied().chain(args.iter().map(String::as_str)).collect()
}
