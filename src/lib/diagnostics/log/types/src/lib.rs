// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

//! This crate provides the basic types that we rely on for logs.

#![warn(missing_docs)]

use std::str::FromStr;
use std::{cmp, fmt};

#[cfg(fuchsia_api_level_less_than = "27")]
use fidl_fuchsia_diagnostics as fdiagnostics;
#[cfg(fuchsia_api_level_at_least = "27")]
use fidl_fuchsia_diagnostics_types as fdiagnostics;

#[cfg(feature = "serde")]
#[doc(hidden)]
pub mod serde_ext;

/// Severities a log message can have, often called the log's "level".
#[cfg_attr(feature = "serde", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "serde", serde(rename_all = "UPPERCASE"))]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Severity {
    /// Trace severity level
    Trace = 0x10,
    /// Debug severity level
    Debug = 0x20,
    /// Info severity level
    Info = 0x30,
    /// Warn severity level
    Warn = 0x40,
    /// Error severity level
    Error = 0x50,
    /// Fatal severity level
    Fatal = 0x60,
}

#[cfg(feature = "serde")]
impl serde::Serialize for Severity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serde_ext::severity::serialize(self, serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Severity {
    fn deserialize<D>(deserializer: D) -> Result<Severity, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde_ext::severity::deserialize(deserializer)
    }
}

impl Severity {
    /// Returns a severity and also the raw severity if it's  not an exact match of a severity value.
    pub fn parse_exact(raw_severity: u8) -> (Option<u8>, Severity) {
        if raw_severity == Severity::Trace as u8 {
            (None, Severity::Trace)
        } else if raw_severity == Severity::Debug as u8 {
            (None, Severity::Debug)
        } else if raw_severity == Severity::Info as u8 {
            (None, Severity::Info)
        } else if raw_severity == Severity::Warn as u8 {
            (None, Severity::Warn)
        } else if raw_severity == Severity::Error as u8 {
            (None, Severity::Error)
        } else if raw_severity == Severity::Fatal as u8 {
            (None, Severity::Fatal)
        } else {
            (Some(raw_severity), Severity::from(raw_severity))
        }
    }

    /// Returns the string representation of a severity.
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Trace => "TRACE",
            Severity::Debug => "DEBUG",
            Severity::Info => "INFO",
            Severity::Warn => "WARN",
            Severity::Error => "ERROR",
            Severity::Fatal => "FATAL",
        }
    }
}

macro_rules! impl_from_signed {
    ($($type:ty),*) => {
        $(
            impl From<$type> for Severity {
                fn from(value: $type) -> Severity {
                    match value {
                        ..0x00 => Severity::Trace,
                        0x00..=0x10 => Severity::Trace,
                        0x11..=0x20 => Severity::Debug,
                        0x21..=0x30 => Severity::Info,
                        0x31..=0x40 => Severity::Warn,
                        0x41..=0x50 => Severity::Error,
                        0x51.. => Severity::Fatal,
                    }
                }
            }
        )*
    }
}

macro_rules! impl_from_unsigned {
    ($($type:ty),*) => {
        $(
            impl From<$type> for Severity {
                fn from(value: $type) -> Severity {
                    match value {
                        0x00..=0x10 => Severity::Trace,
                        0x11..=0x20 => Severity::Debug,
                        0x21..=0x30 => Severity::Info,
                        0x31..=0x40 => Severity::Warn,
                        0x41..=0x50 => Severity::Error,
                        0x51.. => Severity::Fatal,
                    }
                }
            }
        )*
    }
}

impl_from_signed!(i8, i16, i32, i64, i128);
impl_from_unsigned!(u8, u16, u32, u64, u128);

impl From<log::Level> for Severity {
    fn from(level: log::Level) -> Severity {
        match level {
            log::Level::Trace => Severity::Trace,
            log::Level::Debug => Severity::Debug,
            log::Level::Info => Severity::Info,
            log::Level::Warn => Severity::Warn,
            log::Level::Error => Severity::Error,
        }
    }
}

impl TryFrom<log::LevelFilter> for Severity {
    type Error = ();
    fn try_from(s: log::LevelFilter) -> Result<Severity, ()> {
        match s {
            log::LevelFilter::Off => Err(()),
            log::LevelFilter::Trace => Ok(Severity::Trace),
            log::LevelFilter::Debug => Ok(Severity::Debug),
            log::LevelFilter::Info => Ok(Severity::Info),
            log::LevelFilter::Warn => Ok(Severity::Warn),
            log::LevelFilter::Error => Ok(Severity::Error),
        }
    }
}

impl From<Severity> for log::LevelFilter {
    fn from(s: Severity) -> log::LevelFilter {
        match s {
            Severity::Trace => log::LevelFilter::Trace,
            Severity::Debug => log::LevelFilter::Debug,
            Severity::Info => log::LevelFilter::Info,
            Severity::Warn => log::LevelFilter::Warn,
            Severity::Fatal | Severity::Error => log::LevelFilter::Error,
        }
    }
}

impl From<Severity> for fdiagnostics::Severity {
    fn from(s: Severity) -> fdiagnostics::Severity {
        match s {
            Severity::Trace => fdiagnostics::Severity::Trace,
            Severity::Debug => fdiagnostics::Severity::Debug,
            Severity::Info => fdiagnostics::Severity::Info,
            Severity::Warn => fdiagnostics::Severity::Warn,
            Severity::Error => fdiagnostics::Severity::Error,
            Severity::Fatal => fdiagnostics::Severity::Fatal,
        }
    }
}

impl From<fdiagnostics::Severity> for Severity {
    fn from(s: fdiagnostics::Severity) -> Severity {
        match s {
            fdiagnostics::Severity::Trace => Severity::Trace,
            fdiagnostics::Severity::Debug => Severity::Debug,
            fdiagnostics::Severity::Info => Severity::Info,
            fdiagnostics::Severity::Warn => Severity::Warn,
            fdiagnostics::Severity::Error => Severity::Error,
            fdiagnostics::Severity::Fatal => Severity::Fatal,
            #[cfg(fuchsia_api_level_at_least = "27")]
            fdiagnostics::Severity::__SourceBreaking { .. } => Severity::Info,
        }
    }
}

impl AsRef<str> for Severity {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Parsing error for severities.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Attempted to parse a string that didn't map to a valid severity.
    #[error("invalid severity: {0}")]
    Invalid(String),
}

impl FromStr for Severity {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        match s.as_str() {
            "trace" => Ok(Severity::Trace),
            "debug" => Ok(Severity::Debug),
            "info" => Ok(Severity::Info),
            "warn" | "warning" => Ok(Severity::Warn),
            "error" => Ok(Severity::Error),
            "fatal" => Ok(Severity::Fatal),
            other => Err(Error::Invalid(other.to_string())),
        }
    }
}

impl PartialEq<fdiagnostics::Severity> for Severity {
    fn eq(&self, other: &fdiagnostics::Severity) -> bool {
        matches!(
            (self, other),
            (Severity::Trace, fdiagnostics::Severity::Trace)
                | (Severity::Debug, fdiagnostics::Severity::Debug)
                | (Severity::Info, fdiagnostics::Severity::Info)
                | (Severity::Warn, fdiagnostics::Severity::Warn)
                | (Severity::Error, fdiagnostics::Severity::Error)
                | (Severity::Fatal, fdiagnostics::Severity::Fatal)
        )
    }
}

impl PartialOrd<fdiagnostics::Severity> for Severity {
    fn partial_cmp(&self, other: &fdiagnostics::Severity) -> Option<cmp::Ordering> {
        let other = match other {
            fdiagnostics::Severity::Trace => Severity::Trace,
            fdiagnostics::Severity::Debug => Severity::Debug,
            fdiagnostics::Severity::Info => Severity::Info,
            fdiagnostics::Severity::Warn => Severity::Warn,
            fdiagnostics::Severity::Error => Severity::Error,
            fdiagnostics::Severity::Fatal => Severity::Fatal,
            #[cfg(fuchsia_api_level_at_least = "27")]
            fdiagnostics::Severity::__SourceBreaking { .. } => return None,
        };
        self.partial_cmp(&other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_fdiagnostics_severity() {
        assert_eq!(Severity::from(fdiagnostics::Severity::Trace), Severity::Trace);
        assert_eq!(Severity::from(fdiagnostics::Severity::Debug), Severity::Debug);
        assert_eq!(Severity::from(fdiagnostics::Severity::Info), Severity::Info);
        assert_eq!(Severity::from(fdiagnostics::Severity::Warn), Severity::Warn);
        assert_eq!(Severity::from(fdiagnostics::Severity::Error), Severity::Error);
        assert_eq!(Severity::from(fdiagnostics::Severity::Fatal), Severity::Fatal);
        #[cfg(fuchsia_api_level_at_least = "27")]
        assert_eq!(
            Severity::from(fdiagnostics::Severity::__SourceBreaking { unknown_ordinal: 123 }),
            Severity::Info
        );
    }

    #[test]
    fn partial_cmp_fdiagnostics_severity() {
        assert_eq!(
            Severity::Info.partial_cmp(&fdiagnostics::Severity::Warn),
            Some(cmp::Ordering::Less)
        );
        assert_eq!(
            Severity::Warn.partial_cmp(&fdiagnostics::Severity::Info),
            Some(cmp::Ordering::Greater)
        );
        assert_eq!(
            Severity::Info.partial_cmp(&fdiagnostics::Severity::Info),
            Some(cmp::Ordering::Equal)
        );
        #[cfg(fuchsia_api_level_at_least = "27")]
        assert_eq!(
            Severity::Info
                .partial_cmp(&fdiagnostics::Severity::__SourceBreaking { unknown_ordinal: 123 }),
            None
        );
    }
}
