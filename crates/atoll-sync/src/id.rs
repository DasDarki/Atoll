use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::SyncError;

/// The id of a workspace, island or tab that is the same on every device, written as a UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyncId([u8; 16]);

impl SyncId {
    /// A new random id (UUID version 4).
    pub fn random() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the system has no source of randomness");
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for SyncId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                formatter.write_str("-")?;
            }
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for SyncId {
    type Err = SyncError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let hex: String = text.chars().filter(|character| *character != '-').collect();
        if hex.len() != 32 || text.len() != 36 {
            return Err(SyncError::InvalidId(text.to_owned()));
        }
        let mut bytes = [0u8; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
                .map_err(|_| SyncError::InvalidId(text.to_owned()))?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for SyncId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SyncId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_written_as_uuids_and_read_back() {
        let id = SyncId::random();
        let text = id.to_string();
        assert_eq!(text.len(), 36);
        assert_eq!(&text[14..15], "4", "version 4");
        assert_eq!(text.parse::<SyncId>().unwrap(), id);
        assert_ne!(SyncId::random(), id);
    }

    #[test]
    fn what_is_no_uuid_is_refused() {
        assert!("".parse::<SyncId>().is_err());
        assert!("not-an-id".parse::<SyncId>().is_err());
        assert!(
            "0123456789abcdef0123456789abcdef"
                .parse::<SyncId>()
                .is_err(),
            "without dashes"
        );
        assert!(
            "0123456z-89ab-cdef-0123-456789abcdef"
                .parse::<SyncId>()
                .is_err()
        );
    }
}
