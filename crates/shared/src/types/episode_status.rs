// Episode status SSoT
//
// The canonical definition of all episode statuses. Every read/write must use this
// enum, never raw string literals (except SQL literals inside raw queries, which
// must match by construction). Change only this enum and let the compiler find the rest.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Episode status — the SSoT for all status values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EpisodeStatus {
    /// The episode's release date has passed and no file exists.
    /// The system should search for a release automatically.
    #[serde(rename = "missing")]
    Missing,
    /// The episode's release date is in the future or unknown.
    #[serde(rename = "unreleased")]
    Unreleased,
    /// A file exists on disk (set by the download pipeline or manual import).
    #[serde(rename = "downloaded")]
    Downloaded,
    /// The file has been organized/moved to its final library path.
    #[serde(rename = "organized")]
    Organized,
    /// The episode has an active entry in the download queue.
    #[serde(rename = "in_queue")]
    InQueue,
    /// The episode falls outside the user's configured season range.
    #[serde(rename = "out_of_range")]
    OutOfRange,
}

impl EpisodeStatus {
    /// Return the canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unreleased => "unreleased",
            Self::Downloaded => "downloaded",
            Self::Organized => "organized",
            Self::InQueue => "in_queue",
            Self::OutOfRange => "out_of_range",
        }
    }

    /// The "persistent" statuses — those set by the organize / download pipeline
    /// and never overridden by reactive derivation.
    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::OutOfRange | Self::Organized | Self::Downloaded)
    }

    /// Human-readable label for UI display.
    ///
    /// This is the SSoT for how each status appears in the frontend.
    /// The canonical `as_str()` value is used for API/serialization;
    /// this method provides the display-friendly version.
    pub fn display_label(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unreleased => "unreleased",
            Self::Downloaded => "downloaded",
            Self::Organized => "organized",
            Self::InQueue => "queue",
            Self::OutOfRange => "out of range",
        }
    }
}

impl fmt::Display for EpisodeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for EpisodeStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "missing" => Ok(Self::Missing),
            "unreleased" => Ok(Self::Unreleased),
            "downloaded" => Ok(Self::Downloaded),
            "organized" => Ok(Self::Organized),
            "in_queue" => Ok(Self::InQueue),
            "out_of_range" => Ok(Self::OutOfRange),
            _ => Err(format!("unknown episode status: {s:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip() {
        for status in [
            EpisodeStatus::Missing,
            EpisodeStatus::Unreleased,
            EpisodeStatus::Downloaded,
            EpisodeStatus::Organized,
            EpisodeStatus::InQueue,
            EpisodeStatus::OutOfRange,
        ] {
            let s = status.as_str();
            assert_eq!(s.parse::<EpisodeStatus>().unwrap(), status);
            assert_eq!(status.to_string(), s);
        }
    }

    #[test]
    fn test_serde_round_trip() {
        for status in [
            EpisodeStatus::Missing,
            EpisodeStatus::Unreleased,
            EpisodeStatus::Downloaded,
            EpisodeStatus::Organized,
            EpisodeStatus::InQueue,
            EpisodeStatus::OutOfRange,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, format!("\"{}\"", status.as_str()));
            let deserialized: EpisodeStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, status);
        }
    }

    #[test]
    fn test_is_persistent() {
        assert!(EpisodeStatus::OutOfRange.is_persistent());
        assert!(EpisodeStatus::Organized.is_persistent());
        assert!(EpisodeStatus::Downloaded.is_persistent());
        assert!(!EpisodeStatus::Missing.is_persistent());
        assert!(!EpisodeStatus::Unreleased.is_persistent());
        assert!(!EpisodeStatus::InQueue.is_persistent());
    }

    #[test]
    fn test_unknown_status() {
        assert!("bogus".parse::<EpisodeStatus>().is_err());
    }
}
