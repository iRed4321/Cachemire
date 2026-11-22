//! A password, passphrase or token: wiped from memory when dropped, and never
//! printed by `Debug`. Reading the text takes an explicit [`Secret::expose`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(text: impl Into<String>) -> Self {
        Self(Zeroizing::new(text.into()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Empties it, zeroing the memory it held.
    pub fn clear(&mut self) {
        self.0.zeroize();
    }

    /// The text, in clear.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_empty() { "\"\"" } else { "***" })
    }
}

impl From<String> for Secret {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for Secret {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}
