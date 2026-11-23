//! Typed identifiers.
//!
//! Using newtypes instead of raw `String`/`Uuid` prevents accidentally passing
//! an instance id where a server id is expected.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub fn parse(value: &str) -> Result<Self, uuid::Error> {
                Uuid::parse_str(value).map(Self)
            }

            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }

            pub fn to_string_lower(&self) -> String {
                self.0.hyphenated().to_string()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0.hyphenated())
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.to_string_lower()
            }
        }
    };
}

id_type!(InstanceId, "Stable instance identifier (UUID v4).");
id_type!(ServerId, "Stable local server identifier (UUID v4).");
id_type!(AccountId, "Stable account identifier (UUID v4).");
id_type!(SourceId, "Stable NEXUS source identifier (UUID v4).");
id_type!(ModId, "Stable instance mod identifier (UUID v4).");
id_type!(RuntimeId, "Stable Java runtime identifier (UUID v4).");
id_type!(TaskId, "Stable download task identifier (UUID v4).");
id_type!(FileId, "Stable download file identifier (UUID v4).");
id_type!(BackupId, "Stable backup identifier (UUID v4).");
id_type!(BuildId, "Stable installed build identifier (UUID v4).");

/// Deterministic offline Minecraft UUID.
///
/// The value is derived from the offline player name using UUID v3 with the
/// standard `OfflinePlayer:<name>` input. It is never presented as a verified
/// Microsoft UUID.
pub fn offline_player_uuid(name: &str) -> Uuid {
    Uuid::new_v3(
        &Uuid::NAMESPACE_OID,
        format!("OfflinePlayer:{name}").as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_serialize_as_plain_uuid_strings() {
        let id = InstanceId::parse("069a79f4-44e9-4726-a5be-fca90e38aaf5").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"069a79f4-44e9-4726-a5be-fca90e38aaf5\"");
        let back: InstanceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn generated_ids_are_unique() {
        assert_ne!(ServerId::new(), ServerId::new());
    }

    #[test]
    fn offline_uuid_is_deterministic_and_typed() {
        let first = offline_player_uuid("Player");
        let second = offline_player_uuid("Player");
        assert_eq!(first, second);
        assert_eq!(first.get_version_num(), 3);
        assert_ne!(first, offline_player_uuid("player"));
    }
}
