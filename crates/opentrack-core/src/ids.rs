//! Typed identifiers for tracks, events and media assets.
//!
//! IDs are `u64` counters owned by the [`Project`](crate::project::Project);
//! they serialize transparently and are stable across save/load.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            /// Wraps a raw counter value.
            pub const fn from_raw(v: u64) -> Self {
                Self(v)
            }

            /// The raw counter value.
            pub const fn as_u64(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "#{}"), self.0)
            }
        }
    };
}

id_type!(
    /// Identifies a track within a project.
    TrackId
);
id_type!(
    /// Identifies an event (a media instance on a track).
    EventId
);
id_type!(
    /// Identifies a media asset in the project media pool.
    MediaId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_transparently() {
        let json = serde_json::to_string(&TrackId(7)).unwrap();
        assert_eq!(json, "7");
        let back: TrackId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TrackId(7));
    }
}
