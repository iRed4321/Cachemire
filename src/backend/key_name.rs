//! A key's name as the raw bytes Redis holds, which need not be UTF-8. The UI deals
//! in strings, so each name has an id: its text, with `\` written `\\` and each byte
//! that isn't UTF-8 written `\xNN`, so it reads back to the exact bytes.

use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct KeyName(Vec<u8>);

impl KeyName {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The name an id (see [`KeyName::id`]) stands for.
    pub fn from_id(id: &str) -> Self {
        let mut bytes = Vec::with_capacity(id.len());
        let mut rest = id;
        while let Some(at) = rest.find('\\') {
            bytes.extend_from_slice(&rest.as_bytes()[..at]);
            let escape = &rest[at + 1..];
            if let Some(after) = escape.strip_prefix('\\') {
                bytes.push(b'\\');
                rest = after;
            } else if let Some(byte) = escape.strip_prefix('x').and_then(|hex| hex.get(..2)).and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                bytes.push(byte);
                rest = &escape[3..];
            } else {
                // not an escape this writes: kept as it is
                bytes.push(b'\\');
                rest = escape;
            }
        }
        bytes.extend_from_slice(rest.as_bytes());
        Self(bytes)
    }

    /// The name as a string the UI can hold and give back: the text itself for a
    /// UTF-8 name with no backslash, which is nearly every name.
    pub fn id(&self) -> String {
        let mut id = String::with_capacity(self.0.len());
        for chunk in self.0.utf8_chunks() {
            for c in chunk.valid().chars() {
                if c == '\\' {
                    id.push_str("\\\\");
                } else {
                    id.push(c);
                }
            }
            for byte in chunk.invalid() {
                id.push_str(&format!("\\x{byte:02x}"));
            }
        }
        id
    }

    /// How an id is shown: as the name reads, its bytes that aren't UTF-8 as `\xNN`.
    pub fn display_id(id: &str) -> Cow<'_, str> {
        if id.contains("\\\\") { Cow::Owned(id.replace("\\\\", "\\")) } else { Cow::Borrowed(id) }
    }

    /// The name as text, any bytes that aren't UTF-8 replaced.
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    /// The part up to byte `end`; the caller cuts at an ASCII delimiter.
    pub fn prefix(&self, end: usize) -> Self {
        Self(self.0[..end].to_vec())
    }
}

impl From<&str> for KeyName {
    fn from(text: &str) -> Self {
        Self(text.as_bytes().to_vec())
    }
}

impl From<Vec<u8>> for KeyName {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl redis::ToRedisArgs for KeyName {
    fn write_redis_args<W: ?Sized + redis::RedisWrite>(&self, out: &mut W) {
        out.write_arg(&self.0);
    }
}

impl redis::FromRedisValue for KeyName {
    fn from_redis_value(value: redis::Value) -> Result<Self, redis::ParsingError> {
        Ok(Self(Vec::<u8>::from_redis_value(value)?))
    }
}

