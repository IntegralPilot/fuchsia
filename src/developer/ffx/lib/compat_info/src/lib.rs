// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use std::{fmt, io};
#[derive(Clone, Deserialize, JsonSchema, Serialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CompatibilityState {
    /// An error was encountered determining the compatibility status.
    Error,
    /// The compatibility information is not present.
    Absent,
    ///  ABI revision was not recognized.
    Unknown,
    ///  ABI revision it presented is not supported.
    Unsupported,
    /// ABI revision is supported
    #[serde(alias = "OK")]
    Supported,
}
impl fmt::Display for CompatibilityState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let trim: &[char] = &['"'];
        write!(f, "{}", serde_json::to_value(self).unwrap().to_string().trim_matches(trim))
    }
}
impl std::str::FromStr for CompatibilityState {
    type Err = io::Error;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(&format!("\"{}\"", text.to_lowercase())).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("could not convert {text} into CompatibilityState: {e}"),
            )
        })
    }
}
impl From<fidl_fuchsia_developer_remotecontrol::CompatibilityState> for CompatibilityState {
    fn from(value: fidl_fuchsia_developer_remotecontrol::CompatibilityState) -> Self {
        match value {
            fidl_fuchsia_developer_remotecontrol::CompatibilityState::Error => Self::Error,
            fidl_fuchsia_developer_remotecontrol::CompatibilityState::Absent => Self::Absent,
            fidl_fuchsia_developer_remotecontrol::CompatibilityState::Unknown => Self::Unknown,
            fidl_fuchsia_developer_remotecontrol::CompatibilityState::Unsupported => {
                Self::Unsupported
            }
            fidl_fuchsia_developer_remotecontrol::CompatibilityState::Supported => Self::Supported,
            _ => CompatibilityState::Error,
        }
    }
}
impl Into<fidl_fuchsia_developer_remotecontrol::CompatibilityState> for CompatibilityState {
    fn into(self) -> fidl_fuchsia_developer_remotecontrol::CompatibilityState {
        match self {
            Self::Error => fidl_fuchsia_developer_remotecontrol::CompatibilityState::Error,
            Self::Absent => fidl_fuchsia_developer_remotecontrol::CompatibilityState::Absent,
            Self::Unknown => fidl_fuchsia_developer_remotecontrol::CompatibilityState::Unknown,
            Self::Unsupported => {
                fidl_fuchsia_developer_remotecontrol::CompatibilityState::Unsupported
            }
            Self::Supported => fidl_fuchsia_developer_remotecontrol::CompatibilityState::Supported,
        }
    }
}
impl From<version_history::AbiRevisionError> for CompatibilityState {
    fn from(value: version_history::AbiRevisionError) -> Self {
        match value {
            version_history::AbiRevisionError::Malformed { .. }
            | version_history::AbiRevisionError::PlatformMismatch { .. }
            | version_history::AbiRevisionError::UnstableMismatch { .. }
            | version_history::AbiRevisionError::TooNew { .. } => Self::Unknown,
            version_history::AbiRevisionError::Retired { .. }
            | version_history::AbiRevisionError::Invalid => Self::Unsupported,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompatibilityInfo {
    pub status: CompatibilityState,
    pub platform_abi: u64,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ConnectionInfo {
    pub ssh_connection: String,
    // Use the old "compatibility" name for backwards-compatibility
    #[serde(rename = "compatibility")]
    pub connect_info: DeviceConnectionInfo,
}

impl From<fidl_fuchsia_developer_remotecontrol::CompatibilityInfo> for CompatibilityInfo {
    fn from(value: fidl_fuchsia_developer_remotecontrol::CompatibilityInfo) -> Self {
        CompatibilityInfo {
            status: value.state.into(),
            platform_abi: value.platform_abi,
            message: value.message,
        }
    }
}
impl Into<fidl_fuchsia_developer_remotecontrol::CompatibilityInfo> for CompatibilityInfo {
    fn into(self) -> fidl_fuchsia_developer_remotecontrol::CompatibilityInfo {
        fidl_fuchsia_developer_remotecontrol::CompatibilityInfo {
            state: self.status.into(),
            platform_abi: self.platform_abi,
            message: self.message,
        }
    }
}

// This struct defines the protocol used by the ssh host-pipe. Most of the fields
// get placed in the CompatibilityInfo object stored in the Target, but the
// overnet_id gets kept in the HostPipeChild.
// We put the overnet_id in here because this allows backwards-compatibility
// with devices that don't provide the overnet_id.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct DeviceConnectionInfo {
    pub status: CompatibilityState,
    // This could be serialized as a string in some cases, so handle that case
    #[serde(deserialize_with = "parse_string_or_u64")]
    pub platform_abi: u64,
    pub message: String,
    pub overnet_id: Option<u64>,
}

fn parse_string_or_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    // use a untagged enum to handle both formats
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringNum {
        String(String),
        Num(u64),
    }
    match StringNum::deserialize(deserializer)? {
        StringNum::String(s) => s.parse::<u64>().map_err(serde::de::Error::custom),
        StringNum::Num(n) => Ok(n),
    }
}

impl Into<CompatibilityInfo> for DeviceConnectionInfo {
    fn into(self) -> CompatibilityInfo {
        CompatibilityInfo {
            status: self.status,
            platform_abi: self.platform_abi,
            message: self.message,
        }
    }
}
