// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, bail};
use chrono::NaiveDate;
use itertools::Itertools;
use log::error;
use serde::de::{Error, Unexpected};
use serde::{Deserialize, Deserializer, Serialize};
use std::array::TryFromSliceError;
use std::collections::BTreeMap;
use std::fmt;

const VERSION_HISTORY_SCHEMA_ID: &str = "https://fuchsia.dev/schema/version_history.json";
const VERSION_HISTORY_NAME: &str = "Platform version map";
const VERSION_HISTORY_TYPE: &str = "version_history";

/// An `ApiLevel` represents an API level of the Fuchsia platform.
#[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Copy, Clone)]
pub struct ApiLevel(u32);

impl ApiLevel {
    /// The `NEXT` API level, representing an unstable draft of the next
    /// numbered stable API level.
    pub const NEXT: ApiLevel = ApiLevel(4291821568);

    /// The `HEAD` API level, representing the bleeding edge of development.
    pub const HEAD: ApiLevel = ApiLevel(4292870144);

    /// The `PLATFORM` pseudo-API level, which is used in platform builds.
    pub const PLATFORM: ApiLevel = ApiLevel(4293918720);

    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    pub fn as_u32(&self) -> u32 {
        self.0
    }

    #[deprecated = "API levels are 32-bits as of RFC-0246"]
    pub fn as_u64(&self) -> u64 {
        self.0 as u64
    }
}

impl fmt::Debug for ApiLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ApiLevel::NEXT => write!(f, "ApiLevel::NEXT"),
            ApiLevel::HEAD => write!(f, "ApiLevel::HEAD"),
            ApiLevel::PLATFORM => write!(f, "ApiLevel::PLATFORM"),
            ApiLevel(l) => f.debug_tuple("ApiLevel").field(&l).finish(),
        }
    }
}

impl fmt::Display for ApiLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ApiLevel::NEXT => write!(f, "NEXT"),
            ApiLevel::HEAD => write!(f, "HEAD"),
            ApiLevel::PLATFORM => write!(f, "PLATFORM"),
            ApiLevel(l) => write!(f, "{}", l),
        }
    }
}

impl std::str::FromStr for ApiLevel {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "NEXT" => Ok(ApiLevel::NEXT),
            "HEAD" => Ok(ApiLevel::HEAD),
            "PLATFORM" => Ok(ApiLevel::PLATFORM),
            s => Ok(ApiLevel::from_u32(s.parse()?)),
        }
    }
}

impl From<u32> for ApiLevel {
    fn from(api_level: u32) -> ApiLevel {
        ApiLevel(api_level)
    }
}

impl From<&u32> for ApiLevel {
    fn from(api_level: &u32) -> ApiLevel {
        ApiLevel(*api_level)
    }
}

impl From<ApiLevel> for u32 {
    fn from(api_level: ApiLevel) -> u32 {
        api_level.0
    }
}

/// An `AbiRevision` is the 64-bit stamp representing an ABI revision of the
/// Fuchsia platform.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Copy, Clone)]
pub struct AbiRevision(u64);

/// An AbiRevisionExplanation represents the information that can be extracted
/// from a raw ABI revision just based on the 64-bit number itself.
///
/// See //build/sdk/generate_version_history for the code that generates special
/// ABI revisions, and the precise definitions of the fields below.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum AbiRevisionExplanation {
    /// This is a normal ABI revision, selected randomly. It should correspond
    /// to a normal API level.
    Normal,

    /// This is an unstable ABI revision targeted by platform components.
    Platform {
        /// Approximate date of the `integration.git` revision at which the
        /// package was built.
        date: chrono::NaiveDate,
        /// Prefix of an `integration.git` revision from around the same time
        /// that the package was built.
        git_revision: u16,
    },

    /// This is an unstable ABI revision targeted by components built with the
    /// SDK. This corresponds to API levels like `NEXT` and `HEAD`.
    Unstable {
        /// Approximate date of the `integration.git` revision from which the
        /// SDK that built the package was built.
        date: chrono::NaiveDate,
        /// Prefix of an `integration.git` revision from around the same time
        /// that the SDK that built the package was built.
        git_revision: u16,
    },

    /// This ABI revision is exactly AbiRevision::INVALID, which is 2^64-1.
    Invalid,

    /// This is a special ABI revision with an unknown meaning. Presumably it
    /// was introduced sometime after this code was compiled.
    Malformed,
}

impl AbiRevision {
    /// An ABI revision that is never supported by the platform. To be used when
    /// an ABI revision is necessary, but none makes sense.
    pub const INVALID: AbiRevision = AbiRevision(0xFFFF_FFFF_FFFF_FFFF);

    pub const PATH: &'static str = "meta/fuchsia.abi/abi-revision";

    /// Parse the ABI revision from little-endian bytes.
    pub fn from_bytes(b: [u8; 8]) -> Self {
        AbiRevision(u64::from_le_bytes(b))
    }

    /// Encode the ABI revision into little-endian bytes.
    pub fn as_bytes(&self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    pub const fn from_u64(u: u64) -> AbiRevision {
        AbiRevision(u)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn explanation(self) -> AbiRevisionExplanation {
        // ABI revisions beginning with 0xFF are "special".
        const SPECIAL_ABI_REVISION_PREFIX: u64 = 0xFF;
        const UNSTABLE_ABI_REVISION_PREFIX: u64 = 0xFF00;
        const PLATFORM_ABI_REVISION_PREFIX: u64 = 0xFF01;

        // Extract the date from an unstable or platform ABI revision.
        let get_date = || {
            // The lower 32 bits of an unstable or platform ABI revision encode
            // the date.
            const DATE_MASK: u64 = 0x0000_0000_FFFF_FFFF;
            let date_ordinal = (self.as_u64() & DATE_MASK) as i32;
            chrono::NaiveDate::from_num_days_from_ce_opt(date_ordinal).unwrap_or_default()
        };

        // Extract the Git revision prefix from an unstable or platform ABI
        // revision.
        let get_git_revision = || {
            // The second most significant 16 bits are a prefix of the
            // `integration.git` hash.
            const GIT_HASH_MASK: u64 = 0x0000_FFFF_0000_0000;
            ((self.as_u64() & GIT_HASH_MASK) >> 32) as u16
        };

        if self.as_u64() >> 56 != SPECIAL_ABI_REVISION_PREFIX {
            AbiRevisionExplanation::Normal
        } else if self.as_u64() >> 48 == UNSTABLE_ABI_REVISION_PREFIX {
            AbiRevisionExplanation::Unstable { date: get_date(), git_revision: get_git_revision() }
        } else if self.as_u64() >> 48 == PLATFORM_ABI_REVISION_PREFIX {
            AbiRevisionExplanation::Platform { date: get_date(), git_revision: get_git_revision() }
        } else if self == AbiRevision::INVALID {
            AbiRevisionExplanation::Invalid
        } else {
            AbiRevisionExplanation::Malformed
        }
    }
}

impl fmt::Display for AbiRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO(https://fxbug.dev/370727480): Pad to 16 digits and add 0x prefix.
        write!(f, "{:x}", self.0)
    }
}

impl std::str::FromStr for AbiRevision {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed = if let Some(s) = s.strip_prefix("0x") {
            u64::from_str_radix(&s, 16)?
        } else {
            u64::from_str_radix(&s, 10)?
        };
        Ok(AbiRevision::from_u64(parsed))
    }
}

impl<'de> Deserialize<'de> for AbiRevision {
    fn deserialize<D>(deserializer: D) -> Result<AbiRevision, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(|_| D::Error::invalid_value(Unexpected::Str(&s), &"an unsigned integer"))
    }
}

impl Serialize for AbiRevision {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let string = format!("0x{:016x}", self.0);
        serializer.serialize_str(&string)
    }
}

impl From<u64> for AbiRevision {
    fn from(abi_revision: u64) -> AbiRevision {
        AbiRevision(abi_revision)
    }
}

impl From<&u64> for AbiRevision {
    fn from(abi_revision: &u64) -> AbiRevision {
        AbiRevision(*abi_revision)
    }
}

impl From<AbiRevision> for u64 {
    fn from(abi_revision: AbiRevision) -> u64 {
        abi_revision.0
    }
}

impl From<[u8; 8]> for AbiRevision {
    fn from(abi_revision: [u8; 8]) -> AbiRevision {
        AbiRevision::from_bytes(abi_revision)
    }
}

impl TryFrom<&[u8]> for AbiRevision {
    type Error = TryFromSliceError;

    fn try_from(abi_revision: &[u8]) -> Result<AbiRevision, Self::Error> {
        let abi_revision: [u8; 8] = abi_revision.try_into()?;
        Ok(AbiRevision::from_bytes(abi_revision))
    }
}

/// Version represents an API level of the Fuchsia platform API.
///
/// See
/// https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0239_platform_versioning_in_practice
/// for more details.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub struct Version {
    /// The number associated with this API level.
    pub api_level: ApiLevel,

    /// The ABI revision associated with this API level.
    pub abi_revision: AbiRevision,

    /// The Status denotes the current status of the API level.
    pub status: Status,
}

impl Version {
    /// Returns true if this version is runnable - that is, whether components
    /// targeting this version will be able to run on this device.
    fn is_runnable(&self) -> bool {
        match self.status {
            Status::InDevelopment | Status::Supported => true,
            Status::Unsupported => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord, Hash)]
pub enum Status {
    #[serde(rename = "in-development")]
    InDevelopment,
    #[serde(rename = "supported")]
    Supported,
    #[serde(rename = "unsupported")]
    Unsupported,
}

/// VersionHistory stores the history of Fuchsia API levels, and lets callers
/// query the support status of API levels and ABI revisions.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VersionHistory {
    versions: &'static [Version],
}

impl VersionHistory {
    /// Outside of tests, callers should use the static [HISTORY]
    /// instance. However, for testing purposes, you may want to build your own
    /// hermetic "alternate history" that doesn't change over time.
    ///
    /// If you're not testing versioning in particular, and you just want an API
    /// level/ABI revision that works, see
    /// [get_example_supported_version_for_tests].
    pub const fn new(versions: &'static [Version]) -> Self {
        VersionHistory { versions }
    }

    /// The ABI revision for components and packages that are "part of the
    /// platform" and never "move between releases". For example:
    ///
    /// - Packages produced by the platform build have this ABI revision.
    /// - Components which are not packaged but are "part of the platform"
    ///   nonetheless (e.g. components loaded from bootfs) have this ABI
    ///   revision.
    /// - Most packages produced by assembly tools have this ABI revision.
    ///   - The `update` package is a noteworthy exception, since it "moves
    ///     between releases", in that it is produced by assembly tools from one
    ///     Fuchsia release, and then later read by the OS from a previous
    ///     release (that is, the one performing the update).
    pub fn get_abi_revision_for_platform_components(&self) -> AbiRevision {
        self.version_from_api_level(ApiLevel::PLATFORM)
            .expect("API Level PLATFORM not found!")
            .abi_revision
    }

    /// ffx currently presents information suggesting that the platform supports
    /// a _single_ API level and ABI revision. This is misleading and should be
    /// fixed. Until we do, we have to choose a particular Version for ffx to
    /// present.
    ///
    /// TODO: https://fxbug.dev/326096999 - Remove this, or turn it into
    /// something that makes more sense.
    pub fn get_misleading_version_for_ffx(&self) -> Version {
        self.runnable_versions()
            .filter(|v| {
                v.api_level != ApiLevel::NEXT
                    && v.api_level != ApiLevel::HEAD
                    && v.api_level != ApiLevel::PLATFORM
            })
            .last()
            .unwrap()
    }

    /// API level to be used in tests that create components on the fly and need
    /// to specify a supported API level or ABI revision, but don't particularly
    /// care which. The returned [Version] will be consistent within a given
    /// build, but may change from build to build.
    pub fn get_example_supported_version_for_tests(&self) -> Version {
        self.runnable_versions()
            .filter(|v| {
                v.api_level != ApiLevel::NEXT
                    && v.api_level != ApiLevel::HEAD
                    && v.api_level != ApiLevel::PLATFORM
            })
            .last()
            .unwrap()
    }

    /// Check whether the platform supports building components that target the
    /// given API level, and if so, returns the ABI revision associated with
    /// that API level.
    pub fn check_api_level_for_build(
        &self,
        api_level: ApiLevel,
    ) -> Result<AbiRevision, ApiLevelError> {
        let Some(version) = self.version_from_api_level(api_level) else {
            return Err(ApiLevelError::Unknown {
                api_level,
                supported: self.runnable_versions().map(|v| v.api_level).collect(),
            });
        };

        if version.is_runnable() {
            Ok(version.abi_revision)
        } else {
            Err(ApiLevelError::Unsupported {
                version,
                supported: self.runnable_versions().map(|v| v.api_level).collect(),
            })
        }
    }

    /// Check whether the operating system supports running components that
    /// target the given ABI revision.
    pub fn check_abi_revision_for_runtime(
        &self,
        abi_revision: AbiRevision,
    ) -> Result<(), AbiRevisionError> {
        if let Some(version) = self.version_from_abi_revision(abi_revision) {
            if version.is_runnable() {
                Ok(())
            } else {
                Err(AbiRevisionError::Retired {
                    version,
                    supported_versions: self.supported_versions(),
                })
            }
        } else {
            // We don't recognize this ABI revision... Look at its structure to
            // understand what's going on.
            match abi_revision.explanation() {
                AbiRevisionExplanation::Platform { date, git_revision } => {
                    let (platform_date, platform_commit_hash) = self.platform_abi_info();
                    Err(AbiRevisionError::PlatformMismatch {
                        abi_revision,
                        package_date: date,
                        package_commit_hash: git_revision,
                        platform_date,
                        platform_commit_hash,
                    })
                }
                AbiRevisionExplanation::Unstable { date, git_revision } => {
                    let (platform_date, platform_commit_hash) = self.platform_abi_info();
                    Err(AbiRevisionError::UnstableMismatch {
                        abi_revision,
                        package_sdk_date: date,
                        package_sdk_commit_hash: git_revision,
                        platform_date,
                        platform_commit_hash,
                        supported_versions: self.supported_versions(),
                    })
                }
                AbiRevisionExplanation::Malformed => {
                    Err(AbiRevisionError::Malformed { abi_revision })
                }
                AbiRevisionExplanation::Invalid => Err(AbiRevisionError::Invalid),
                AbiRevisionExplanation::Normal => Err(AbiRevisionError::TooNew {
                    abi_revision,
                    supported_versions: self.supported_versions(),
                }),
            }
        }
    }

    fn runnable_versions(&self) -> impl Iterator<Item = Version> + '_ {
        self.versions.iter().filter(|v| v.is_runnable()).cloned()
    }

    fn supported_versions(&self) -> VersionVec {
        VersionVec(
            self.versions
                .iter()
                .filter(|v| match v.status {
                    Status::InDevelopment => false,
                    Status::Supported => true,
                    Status::Unsupported => false,
                })
                .cloned()
                .collect(),
        )
    }

    fn version_from_abi_revision(&self, abi_revision: AbiRevision) -> Option<Version> {
        self.versions.iter().find(|v| v.abi_revision == abi_revision).cloned()
    }

    pub fn version_from_api_level(&self, api_level: ApiLevel) -> Option<Version> {
        self.versions.iter().find(|v| v.api_level == api_level).cloned()
    }

    fn platform_abi_info(&self) -> (NaiveDate, u16) {
        if let Some(platform_version) = self.version_from_api_level(ApiLevel::PLATFORM) {
            if let AbiRevisionExplanation::Platform { date, git_revision } =
                platform_version.abi_revision.explanation()
            {
                return (date, git_revision);
            }
        }
        error!(
            "No PLATFORM API level found. This should never happen.
Returning bogus data instead of panicking."
        );
        (NaiveDate::default(), 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AbiRevisionError {
    #[error(
        "Unknown platform ABI revision: 0x{abi_revision}.
Found platform component from {package_date} (Git hash {package_commit_hash:x}).
Platform components must be from {platform_date} (Git hash {platform_commit_hash:x})"
    )]
    PlatformMismatch {
        abi_revision: AbiRevision,
        package_date: NaiveDate,
        package_commit_hash: u16,
        platform_date: NaiveDate,
        platform_commit_hash: u16,
    },
    #[error(
        "Unknown NEXT or HEAD ABI revision: 0x{abi_revision}.
SDK is from {package_sdk_date} (Git hash {package_sdk_commit_hash:x}).
Expected {platform_date} (Git hash {platform_commit_hash:x})
The following API levels are stable and supported:{supported_versions}"
    )]
    UnstableMismatch {
        abi_revision: AbiRevision,
        package_sdk_date: NaiveDate,
        package_sdk_commit_hash: u16,
        platform_date: NaiveDate,
        platform_commit_hash: u16,
        supported_versions: VersionVec,
    },

    #[error(
        "Unknown ABI revision 0x{abi_revision}. It was probably created after
this platform or tool was built. The following API levels are stable and
supported:{supported_versions}"
    )]
    TooNew { abi_revision: AbiRevision, supported_versions: VersionVec },

    #[error("Retired API level {} (0x{}) cannot run.
The following API levels are stable and supported:{supported_versions}",
        .version.api_level, .version.abi_revision)]
    Retired { version: Version, supported_versions: VersionVec },

    #[error("Invalid ABI revision 0x{}.", AbiRevision::INVALID)]
    Invalid,

    #[error("ABI revision 0x{abi_revision} has an unrecognized format.")]
    Malformed { abi_revision: AbiRevision },
}

/// Wrapper for a vector of Versions with a nice implementation of Display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionVec(pub Vec<Version>);

impl std::fmt::Display for VersionVec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for version in self.0.iter() {
            write!(f, "\n└── {} (0x{})", version.api_level, version.abi_revision)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiLevelError {
    /// The user attempted to build a component targeting an API level that was
    /// not recognized.
    Unknown { api_level: ApiLevel, supported: Vec<ApiLevel> },

    /// The user attempted to build a component targeting an API level that was
    /// recognized, but is not supported for building.
    Unsupported { version: Version, supported: Vec<ApiLevel> },
}

impl std::error::Error for ApiLevelError {}

impl std::fmt::Display for ApiLevelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiLevelError::Unknown { api_level, supported } => {
                write!(
                    f,
                    "Unknown target API level: {}. Is the SDK too old to support it?
The following API levels are supported: {}",
                    api_level,
                    supported.iter().join(", ")
                )
            }
            ApiLevelError::Unsupported { version, supported } => {
                write!(
                    f,
                    "The SDK no longer supports API level {}.
The following API levels are supported: {}",
                    version.api_level,
                    supported.iter().join(", ")
                )
            }
        }
    }
}

#[derive(Deserialize)]
struct VersionHistoryDataJson {
    name: String,
    #[serde(rename = "type")]
    element_type: String,
    api_levels: BTreeMap<String, ApiLevelJson>,
    special_api_levels: BTreeMap<String, SpecialApiLevelJson>,
}

#[derive(Deserialize)]
struct VersionHistoryJson {
    schema_id: String,
    data: VersionHistoryDataJson,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Deserialize)]
struct ApiLevelJson {
    pub abi_revision: AbiRevision,
    pub status: Status,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Deserialize)]
struct SpecialApiLevelJson {
    pub as_u32: u32,
    pub abi_revision: AbiRevision,
    pub status: Status,
}

pub fn parse_version_history(bytes: &[u8]) -> anyhow::Result<Vec<Version>> {
    let v: VersionHistoryJson = serde_json::from_slice(bytes)?;

    if v.schema_id != VERSION_HISTORY_SCHEMA_ID {
        bail!("expected schema_id = {:?}; got {:?}", VERSION_HISTORY_SCHEMA_ID, v.schema_id)
    }
    if v.data.name != VERSION_HISTORY_NAME {
        bail!("expected data.name = {:?}; got {:?}", VERSION_HISTORY_NAME, v.data.name)
    }
    if v.data.element_type != VERSION_HISTORY_TYPE {
        bail!("expected data.type = {:?}; got {:?}", VERSION_HISTORY_TYPE, v.data.element_type,)
    }

    let mut versions = Vec::new();

    for (key, value) in v.data.api_levels {
        versions.push(Version {
            api_level: key.parse()?,
            abi_revision: value.abi_revision,
            status: value.status,
        });
    }

    for (key, value) in v.data.special_api_levels {
        let api_level: ApiLevel =
            key.parse().with_context(|| format!("Unknown special API level: {}", key))?;
        if api_level.as_u32() != value.as_u32 {
            bail!(
                "Special API level {} had unexpected numerical value {} (Expected {})",
                api_level,
                value.as_u32,
                api_level.as_u32()
            )
        }
        versions.push(Version {
            api_level,
            abi_revision: value.abi_revision,
            status: value.status,
        });
    }

    versions.sort_by_key(|s| s.api_level);

    let Some(latest_api_version) = versions.last() else {
        bail!("there must be at least one API level")
    };

    if latest_api_version.status == Status::Unsupported {
        bail!("most recent API level must not be 'unsupported'")
    }

    Ok(versions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_history_works() {
        let expected_bytes = br#"{
            "data": {
                "name": "Platform version map",
                "type": "version_history",
                "api_levels": {
                    "1":{
                        "abi_revision":"0xBEBE3F5CAAA046D2",
                        "status":"supported"
                    },
                    "2":{
                        "abi_revision":"0x50cbc6e8a39e1e2c",
                        "status":"in-development"
                    }
                },
                "special_api_levels": {
                    "NEXT": {
                        "as_u32": 4291821568,
                        "abi_revision": "0xFF0038FA031D0347",
                        "status": "in-development"
                    },
                    "HEAD": {
                        "as_u32": 4292870144,
                        "abi_revision": "0xFF0038FA031D0348",
                        "status": "in-development"
                    },
                    "PLATFORM": {
                        "as_u32": 4293918720,
                        "abi_revision": "0xFF01A9328DA0C138",
                        "status": "in-development"
                    }
                }
            },
            "schema_id": "https://fuchsia.dev/schema/version_history.json"
        }"#;

        assert_eq!(
            parse_version_history(&expected_bytes[..]).unwrap(),
            vec![
                Version {
                    api_level: 1.into(),
                    abi_revision: 0xBEBE3F5CAAA046D2.into(),
                    status: Status::Supported
                },
                Version {
                    api_level: 2.into(),
                    abi_revision: 0x50CBC6E8A39E1E2C.into(),
                    status: Status::InDevelopment
                },
                Version {
                    api_level: ApiLevel::NEXT,
                    abi_revision: 0xFF0038FA031D0347.into(),
                    status: Status::InDevelopment
                },
                Version {
                    api_level: ApiLevel::HEAD,
                    abi_revision: 0xFF0038FA031D0348.into(),
                    status: Status::InDevelopment
                },
                Version {
                    api_level: ApiLevel::PLATFORM,
                    abi_revision: 0xFF01A9328DA0C138.into(),
                    status: Status::InDevelopment
                },
            ],
        );
    }

    #[test]
    fn test_parse_history_rejects_invalid_schema() {
        let expected_bytes = br#"{
            "data": {
                "name": "Platform version map",
                "type": "version_history",
                "api_levels": {},
                "special_api_levels": {}
            },
            "schema_id": "some-schema"
        }"#;

        assert_eq!(
            &parse_version_history(&expected_bytes[..]).unwrap_err().to_string(),
            r#"expected schema_id = "https://fuchsia.dev/schema/version_history.json"; got "some-schema""#
        );
    }

    #[test]
    fn test_parse_history_rejects_invalid_name() {
        let expected_bytes = br#"{
            "data": {
                "name": "some-name",
                "type": "version_history",
                "api_levels": {},
                "special_api_levels": {}
            },
            "schema_id": "https://fuchsia.dev/schema/version_history.json"
        }"#;

        assert_eq!(
            &parse_version_history(&expected_bytes[..]).unwrap_err().to_string(),
            r#"expected data.name = "Platform version map"; got "some-name""#
        );
    }

    #[test]
    fn test_parse_history_rejects_invalid_type() {
        let expected_bytes = br#"{
            "data": {
                "name": "Platform version map",
                "type": "some-type",
                "api_levels": {},
                "special_api_levels": {}
            },
            "schema_id": "https://fuchsia.dev/schema/version_history.json"
        }"#;

        assert_eq!(
            &parse_version_history(&expected_bytes[..]).unwrap_err().to_string(),
            r#"expected data.type = "version_history"; got "some-type""#
        );
    }

    #[test]
    fn test_parse_history_rejects_invalid_versions() {
        for (api_level, abi_revision, err) in [
            ("some-version", "1", "invalid digit found in string"),
            ("-1", "1", "invalid digit found in string"),
            (
                "1",
                "some-revision",
                "invalid value: string \"some-revision\", expected an unsigned integer at line 1 column 113",
            ),
            (
                "1",
                "-1",
                "invalid value: string \"-1\", expected an unsigned integer at line 1 column 102",
            ),
        ] {
            let expected_bytes = serde_json::to_vec(&serde_json::json!({
                "data": {
                    "name": VERSION_HISTORY_NAME,
                    "type": VERSION_HISTORY_TYPE,
                    "api_levels": {
                        api_level:{
                            "abi_revision": abi_revision,
                            "status": Status::InDevelopment,
                        }
                    },
                    "special_api_levels": {},
                },
                "schema_id": VERSION_HISTORY_SCHEMA_ID,
            }))
            .unwrap();

            assert_eq!(parse_version_history(&expected_bytes[..]).unwrap_err().to_string(), err);
        }
    }

    #[test]
    fn test_parse_history_rejects_bogus_special_levels() {
        let input = br#"{
            "data": {
                "name": "Platform version map",
                "type": "version_history",
                "api_levels": {},
                "special_api_levels": {
                    "WACKY_WAVING_INFLATABLE_ARM_FLAILING_TUBE_MAN": {
                        "as_u32": 1234,
                        "abi_revision": "0x50cbc6e8a39e1e2c",
                        "status": "in-development"
                    }
                }
            },
            "schema_id": "https://fuchsia.dev/schema/version_history.json"
        }"#;

        assert_eq!(
            parse_version_history(&input[..]).unwrap_err().to_string(),
            "Unknown special API level: WACKY_WAVING_INFLATABLE_ARM_FLAILING_TUBE_MAN"
        );
    }

    #[test]
    fn test_parse_history_rejects_misnumbered_special_levels() {
        let input = br#"{
            "data": {
                "name": "Platform version map",
                "type": "version_history",
                "api_levels": {},
                "special_api_levels": {
                    "HEAD": {
                        "as_u32": 1234,
                        "abi_revision": "0x50cbc6e8a39e1e2c",
                        "status": "in-development"
                    }
                }
            },
            "schema_id": "https://fuchsia.dev/schema/version_history.json"
        }"#;

        assert_eq!(
            parse_version_history(&input[..]).unwrap_err().to_string(),
            "Special API level HEAD had unexpected numerical value 1234 (Expected 4292870144)"
        );
    }

    #[test]
    fn test_serialize_abi_revision() {
        {
            let abi = AbiRevision::from_u64(16);
            let str = "\"0x0000000000000010\"";
            assert_eq!(serde_json::to_string(&abi).unwrap(), str);
            assert_eq!(serde_json::from_str::<AbiRevision>(str).unwrap(), abi);
        }
        {
            let abi = AbiRevision::from_u64(0x58ea445e942a0004);
            let str = "\"0x58ea445e942a0004\"";
            assert_eq!(serde_json::to_string(&abi).unwrap(), str);
            assert_eq!(serde_json::from_str::<AbiRevision>(str).unwrap(), abi);
        }
    }

    pub const FAKE_VERSION_HISTORY: VersionHistory = VersionHistory {
        versions: &[
            Version {
                api_level: ApiLevel::from_u32(4),
                abi_revision: AbiRevision::from_u64(0x58ea445e942a0004),
                status: Status::Unsupported,
            },
            Version {
                api_level: ApiLevel::from_u32(5),
                abi_revision: AbiRevision::from_u64(0x58ea445e942a0005),
                status: Status::Supported,
            },
            Version {
                api_level: ApiLevel::from_u32(6),
                abi_revision: AbiRevision::from_u64(0x58ea445e942a0006),
                status: Status::Supported,
            },
            Version {
                api_level: ApiLevel::from_u32(7),
                abi_revision: AbiRevision::from_u64(0x58ea445e942a0007),
                status: Status::Supported,
            },
            Version {
                api_level: ApiLevel::NEXT,
                abi_revision: AbiRevision::from_u64(0xFF00_8C4D_000B_4751),
                status: Status::InDevelopment,
            },
            Version {
                api_level: ApiLevel::HEAD,
                abi_revision: AbiRevision::from_u64(0xFF00_8C4D_000B_4751),
                status: Status::InDevelopment,
            },
            Version {
                api_level: ApiLevel::PLATFORM,
                abi_revision: AbiRevision::from_u64(0xFF01_8C4D_000B_4751),
                status: Status::InDevelopment,
            },
        ],
    };

    #[test]
    fn test_check_abi_revision() {
        let supported_versions =
            VersionVec(FAKE_VERSION_HISTORY.versions[1..4].iter().cloned().collect());

        assert_eq!(
            FAKE_VERSION_HISTORY.check_abi_revision_for_runtime(0xFF01_ABCD_000B_2224.into()),
            Err(AbiRevisionError::PlatformMismatch {
                abi_revision: 0xFF01_ABCD_000B_2224.into(),
                package_date: NaiveDate::from_ymd_opt(1998, 9, 4).unwrap(),
                package_commit_hash: 0xABCD,
                platform_date: NaiveDate::from_ymd_opt(2024, 9, 24).unwrap(),
                platform_commit_hash: 0x8C4D,
            })
        );

        assert_eq!(
            FAKE_VERSION_HISTORY.check_abi_revision_for_runtime(0xFF00_ABCD_000B_2224.into()),
            Err(AbiRevisionError::UnstableMismatch {
                abi_revision: 0xFF00_ABCD_000B_2224.into(),
                package_sdk_date: NaiveDate::from_ymd_opt(1998, 9, 4).unwrap(),
                package_sdk_commit_hash: 0xABCD,
                platform_date: NaiveDate::from_ymd_opt(2024, 9, 24).unwrap(),
                platform_commit_hash: 0x8C4D,
                supported_versions: supported_versions.clone()
            })
        );

        assert_eq!(
            FAKE_VERSION_HISTORY.check_abi_revision_for_runtime(0x1234.into()),
            Err(AbiRevisionError::TooNew {
                abi_revision: 0x1234.into(),
                supported_versions: supported_versions.clone()
            })
        );

        assert_eq!(
            FAKE_VERSION_HISTORY.check_abi_revision_for_runtime(0x58ea445e942a0004.into()),
            Err(AbiRevisionError::Retired {
                version: FAKE_VERSION_HISTORY.versions[0].clone(),

                supported_versions: supported_versions.clone()
            })
        );

        assert_eq!(
            FAKE_VERSION_HISTORY.check_abi_revision_for_runtime(AbiRevision::INVALID),
            Err(AbiRevisionError::Invalid)
        );

        assert_eq!(
            FAKE_VERSION_HISTORY.check_abi_revision_for_runtime(0xFF02_0000_0000_0000.into()),
            Err(AbiRevisionError::Malformed { abi_revision: 0xFF02_0000_0000_0000.into() })
        );

        FAKE_VERSION_HISTORY
            .check_abi_revision_for_runtime(0x58ea445e942a0005.into())
            .expect("level 5 should be supported");
        FAKE_VERSION_HISTORY
            .check_abi_revision_for_runtime(0x58ea445e942a0007.into())
            .expect("level 7 should be supported");
    }

    #[test]
    fn test_pretty_print_abi_error() {
        let supported_versions =
            VersionVec(FAKE_VERSION_HISTORY.versions[1..4].iter().cloned().collect());

        assert_eq!(
            AbiRevisionError::PlatformMismatch {
                abi_revision: 0xFF01_ABCD_000B_2224.into(),
                package_date: NaiveDate::from_ymd_opt(1998, 9, 4).unwrap(),
                package_commit_hash: 0xABCD,
                platform_date: NaiveDate::from_ymd_opt(2024, 9, 24).unwrap(),
                platform_commit_hash: 0x8C4D,
            }
            .to_string(),
            "Unknown platform ABI revision: 0xff01abcd000b2224.
Found platform component from 1998-09-04 (Git hash abcd).
Platform components must be from 2024-09-24 (Git hash 8c4d)"
        );

        assert_eq!(
            AbiRevisionError::UnstableMismatch {
                abi_revision: 0xFF00_ABCD_000B_2224.into(),
                package_sdk_date: NaiveDate::from_ymd_opt(1998, 9, 4).unwrap(),
                package_sdk_commit_hash: 0xABCD,
                platform_date: NaiveDate::from_ymd_opt(2024, 9, 24).unwrap(),
                platform_commit_hash: 0x8C4D,
                supported_versions: supported_versions.clone()
            }
            .to_string(),
            "Unknown NEXT or HEAD ABI revision: 0xff00abcd000b2224.
SDK is from 1998-09-04 (Git hash abcd).
Expected 2024-09-24 (Git hash 8c4d)
The following API levels are stable and supported:
└── 5 (0x58ea445e942a0005)
└── 6 (0x58ea445e942a0006)
└── 7 (0x58ea445e942a0007)"
        );

        assert_eq!(
            AbiRevisionError::TooNew {
                abi_revision: 0x1234.into(),
                supported_versions: supported_versions.clone()
            }
            .to_string(),
            "Unknown ABI revision 0x1234. It was probably created after
this platform or tool was built. The following API levels are stable and
supported:
└── 5 (0x58ea445e942a0005)
└── 6 (0x58ea445e942a0006)
└── 7 (0x58ea445e942a0007)"
        );
        assert_eq!(
            AbiRevisionError::Retired {
                version: FAKE_VERSION_HISTORY.versions[0].clone(),
                supported_versions: supported_versions.clone()
            }
            .to_string(),
            "Retired API level 4 (0x58ea445e942a0004) cannot run.
The following API levels are stable and supported:
└── 5 (0x58ea445e942a0005)
└── 6 (0x58ea445e942a0006)
└── 7 (0x58ea445e942a0007)"
        );

        assert_eq!(
            AbiRevisionError::Invalid.to_string(),
            "Invalid ABI revision 0xffffffffffffffff."
        );

        assert_eq!(
            AbiRevisionError::Malformed { abi_revision: 0xFF02_0000_0000_0000.into() }.to_string(),
            "ABI revision 0xff02000000000000 has an unrecognized format."
        );
    }

    #[test]
    fn test_pretty_print_api_error() {
        let supported: Vec<ApiLevel> =
            vec![5.into(), 6.into(), 7.into(), ApiLevel::NEXT, ApiLevel::HEAD, ApiLevel::PLATFORM];

        assert_eq!(
            ApiLevelError::Unknown { api_level: 42.into(), supported: supported.clone() }
                .to_string(),
            "Unknown target API level: 42. Is the SDK too old to support it?
The following API levels are supported: 5, 6, 7, NEXT, HEAD, PLATFORM",
        );
        assert_eq!(
            ApiLevelError::Unsupported {
                version: FAKE_VERSION_HISTORY.versions[0].clone(),
                supported: supported.clone()
            }
            .to_string(),
            "The SDK no longer supports API level 4.
The following API levels are supported: 5, 6, 7, NEXT, HEAD, PLATFORM"
        );
    }

    #[test]
    fn test_check_api_level() {
        let supported: Vec<ApiLevel> =
            vec![5.into(), 6.into(), 7.into(), ApiLevel::NEXT, ApiLevel::HEAD, ApiLevel::PLATFORM];

        assert_eq!(
            FAKE_VERSION_HISTORY.check_api_level_for_build(42.into()),
            Err(ApiLevelError::Unknown { api_level: 42.into(), supported: supported.clone() })
        );

        assert_eq!(
            FAKE_VERSION_HISTORY.check_api_level_for_build(4.into()),
            Err(ApiLevelError::Unsupported {
                version: FAKE_VERSION_HISTORY.versions[0].clone(),
                supported: supported.clone()
            })
        );
        assert_eq!(
            FAKE_VERSION_HISTORY.check_api_level_for_build(6.into()),
            Ok(0x58ea445e942a0006.into())
        );
        assert_eq!(
            FAKE_VERSION_HISTORY.check_api_level_for_build(7.into()),
            Ok(0x58ea445e942a0007.into())
        );
        assert_eq!(
            FAKE_VERSION_HISTORY.check_api_level_for_build(ApiLevel::NEXT),
            Ok(0xFF00_8C4D_000B_4751.into())
        );
        assert_eq!(
            FAKE_VERSION_HISTORY.check_api_level_for_build(ApiLevel::HEAD),
            Ok(0xFF00_8C4D_000B_4751.into())
        );
        assert_eq!(
            FAKE_VERSION_HISTORY.check_api_level_for_build(ApiLevel::PLATFORM),
            Ok(0xFF01_8C4D_000B_4751.into())
        );
    }

    #[test]
    fn test_various_getters() {
        assert_eq!(
            FAKE_VERSION_HISTORY.get_abi_revision_for_platform_components(),
            0xFF01_8C4D_000B_4751.into()
        );
        assert_eq!(
            FAKE_VERSION_HISTORY.get_misleading_version_for_ffx(),
            FAKE_VERSION_HISTORY.versions[3].clone()
        );
        assert_eq!(FAKE_VERSION_HISTORY.get_misleading_version_for_ffx().api_level, 7.into());
        assert_eq!(
            FAKE_VERSION_HISTORY.get_example_supported_version_for_tests(),
            FAKE_VERSION_HISTORY.versions[FAKE_VERSION_HISTORY.versions.len() - 4].clone()
        );
        assert_eq!(
            FAKE_VERSION_HISTORY.get_example_supported_version_for_tests().api_level,
            7.into()
        );
    }

    #[test]
    fn test_explanations() {
        let exp = |abi_revision| AbiRevision::from_u64(abi_revision).explanation();

        assert_eq!(exp(0x1234_5678_9abc_deff), AbiRevisionExplanation::Normal);
        assert_eq!(
            exp(0xFF01_abcd_00ab_eeee),
            AbiRevisionExplanation::Platform {
                date: chrono::NaiveDate::from_num_days_from_ce_opt(0xab_eeee).unwrap(),
                git_revision: 0xabcd
            }
        );
        assert_eq!(
            exp(0xFF00_1234_0012_3456),
            AbiRevisionExplanation::Unstable {
                date: chrono::NaiveDate::from_num_days_from_ce_opt(0x12_3456).unwrap(),
                git_revision: 0x1234
            }
        );
        // 0xabcd_9876 is too large a date for chrono. Make sure we return a
        // null date, rather than crashing.
        assert_eq!(
            exp(0xFF00_1234_abcd_9876),
            AbiRevisionExplanation::Unstable {
                date: chrono::NaiveDate::default(),
                git_revision: 0x1234
            }
        );
        assert_eq!(
            exp(0xFF01_1234_abcd_9876),
            AbiRevisionExplanation::Platform {
                date: chrono::NaiveDate::default(),
                git_revision: 0x1234
            }
        );
        assert_eq!(exp(0xFFFF_FFFF_FFFF_FFFF), AbiRevisionExplanation::Invalid);
        assert_eq!(exp(0xFFFF_FFFF_FFFF_FFFE), AbiRevisionExplanation::Malformed);
    }
}
