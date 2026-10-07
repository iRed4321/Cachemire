use std::fmt;

/// Redis's own `TYPE` command reply: six variants cover every built-in type;
/// `Missing` is `none` (no such key), `Other` an escape hatch for
/// module-defined types (RedisJSON's `ReJSON-RL`, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RedisValueType {
    String,
    Hash,
    List,
    Set,
    Zset,
    Stream,
    Missing,
    Other(String),
}

impl RedisValueType {
    pub fn as_str(&self) -> &str {
        match self {
            RedisValueType::String => "string",
            RedisValueType::Hash => "hash",
            RedisValueType::List => "list",
            RedisValueType::Set => "set",
            RedisValueType::Zset => "zset",
            RedisValueType::Stream => "stream",
            RedisValueType::Missing => "none",
            RedisValueType::Other(s) => s,
        }
    }
}

impl From<String> for RedisValueType {
    fn from(value: String) -> Self {
        match value.as_str() {
            "string" => RedisValueType::String,
            "hash" => RedisValueType::Hash,
            "list" => RedisValueType::List,
            "set" => RedisValueType::Set,
            "zset" => RedisValueType::Zset,
            "stream" => RedisValueType::Stream,
            "none" => RedisValueType::Missing,
            _ => RedisValueType::Other(value),
        }
    }
}

impl From<&str> for RedisValueType {
    fn from(value: &str) -> Self {
        RedisValueType::from(value.to_string())
    }
}

impl fmt::Display for RedisValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
