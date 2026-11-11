//! A key's name as the raw bytes Redis holds, which need not be UTF-8. The UI deals
//! in strings, so each name has an id: its text, with `\` written `\\` and each byte
//! that isn't UTF-8 written `\xNN`, so it reads back to the exact bytes.

use std::borrow::Cow;

/// `bytes` as text, each byte that isn't UTF-8 written `\xNN`; `double` says whether a
/// backslash (given the next character, if any, and whether bytes that aren't UTF-8
/// follow in this run) is written `\\`.
fn escape_bytes(bytes: &[u8], double: impl Fn(Option<char>, bool) -> bool) -> String {
    let mut text = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        let mut chars = chunk.valid().chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' && double(chars.peek().copied(), !chunk.invalid().is_empty()) {
                text.push_str("\\\\");
            } else {
                text.push(c);
            }
        }
        for byte in chunk.invalid() {
            text.push_str(&format!("\\x{byte:02x}"));
        }
    }
    text
}

/// A value as text for the screen: valid UTF-8 as it is, each byte that isn't written
/// `\xNN`; and whether any byte wasn't UTF-8.
pub fn display_bytes(bytes: &[u8]) -> (String, bool) {
    (escape_bytes(bytes, |_, _| false), std::str::from_utf8(bytes).is_err())
}

/// A hash field's name as text that reads back to its bytes ([`field_bytes`]): bytes that
/// aren't UTF-8 are written `\xNN`, and a backslash `\\` where it could be taken for one.
pub fn field_name(bytes: &[u8]) -> String {
    escape_bytes(bytes, |next, invalid_follows| matches!(next, Some('x' | '\\')) || (next.is_none() && invalid_follows))
}

/// The bytes a field name from [`field_name`] stands for.
pub fn field_bytes(name: &str) -> Cow<'_, [u8]> {
    if !name.contains('\\') {
        return Cow::Borrowed(name.as_bytes());
    }
    let mut bytes = Vec::with_capacity(name.len());
    let mut rest = name;
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
            bytes.push(b'\\');
            rest = escape;
        }
    }
    bytes.extend_from_slice(rest.as_bytes());
    Cow::Owned(bytes)
}

/// Whether the field name stands for bytes that are UTF-8 text.
pub fn is_text_field(name: &str) -> bool {
    std::str::from_utf8(&field_bytes(name)).is_ok()
}

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
            } else if let Some(byte) =
                escape.strip_prefix('x').and_then(|hex| hex.get(..2)).and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
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
        escape_bytes(&self.0, |_, _| true)
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

#[cfg(test)]
mod tests;
