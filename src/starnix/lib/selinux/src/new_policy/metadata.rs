// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use selinux_policy_derive::{Parse, Serialize, Validate};

use super::error::{ParseError, SerializeError, ValidateError};
use super::parser::{ByteArray, PolicyCursor, PolicyWriter};
use super::traits::{Parse, Serialize, Validate};

/// Magic number identifying a SELinux policy file.
pub(super) const SELINUX_MAGIC: u32 = 0xf97cff8c;

/// Maximum allowed length for a signature in the policy database.
pub(super) const POLICYDB_STRING_MAX_LENGTH: u32 = 32;

/// Expected signature prefix for a valid SELinux policy.
pub(super) const POLICYDB_SIGNATURE: &[u8] = b"SE Linux";

/// Minimum supported SELinux policy database version.
pub(super) const POLICYDB_VERSION_MIN: u32 = 30;

/// Maximum supported SELinux policy database version.
pub const POLICYDB_VERSION_MAX: u32 = 33;

/// Config flag indicating that MLS is enabled.
pub(super) const CONFIG_MLS_FLAG: u32 = 1;

/// Config flag indicating that unknown permissions should be rejected.
pub(super) const CONFIG_HANDLE_UNKNOWN_REJECT_FLAG: u32 = 1 << 1;

/// Config flag indicating that unknown permissions should be allowed.
pub(super) const CONFIG_HANDLE_UNKNOWN_ALLOW_FLAG: u32 = 1 << 2;

/// Mask for the handle-unknown configuration bits.
pub(super) const CONFIG_HANDLE_UNKNOWN_MASK: u32 =
    CONFIG_HANDLE_UNKNOWN_REJECT_FLAG | CONFIG_HANDLE_UNKNOWN_ALLOW_FLAG;

/// Controls how "unknown" policy decisions are handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleUnknown {
    Deny,
    Reject,
    Allow,
}

/// Magic number at the start of a SELinux policy binary.
#[derive(Debug, Parse, Serialize)]
pub(super) struct Magic {
    value: u32,
}

impl Validate for Magic {
    fn validate(&self, _policy: &super::NewPolicy) -> Result<(), ValidateError> {
        if self.value != SELINUX_MAGIC {
            return Err(ValidateError::InvalidMagic { found_magic: self.value });
        }
        Ok(())
    }
}

/// Signature string that identifies the policy database.
#[derive(Debug, Parse, Serialize)]
pub(super) struct Signature {
    value: ByteArray,
}

impl Validate for Signature {
    fn validate(&self, _policy: &super::NewPolicy) -> Result<(), ValidateError> {
        let len = self.value.len() as u32;
        if len > POLICYDB_STRING_MAX_LENGTH {
            return Err(ValidateError::InvalidSignatureLength { found_length: len });
        }
        if self.value.as_ref() != POLICYDB_SIGNATURE {
            return Err(ValidateError::InvalidSignature {
                found_signature: self.value.as_ref().into(),
            });
        }
        Ok(())
    }
}

/// Version of the SELinux policy database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct PolicyVersion {
    value: u32,
}

impl Parse for PolicyVersion {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let value = u32::parse(cursor)?;
        let version = Self { value };
        cursor.set_policy_version(version);
        Ok(version)
    }
}

impl PolicyVersion {
    pub const V30: Self = Self { value: 30 };
    pub const V31: Self = Self { value: 31 };
    pub const V33: Self = Self { value: 33 };

    /// Minimum policy version supporting InfiniBand partition keys and end ports.
    pub const MIN_INFINIBAND: Self = Self { value: 31 };

    /// Returns the raw policy version value.
    pub fn get(&self) -> u32 {
        self.value
    }
}

impl Validate for PolicyVersion {
    fn validate(&self, _policy: &super::NewPolicy) -> Result<(), ValidateError> {
        let version = self.value;
        if version < POLICYDB_VERSION_MIN || version > POLICYDB_VERSION_MAX {
            return Err(ValidateError::InvalidPolicyVersion { found_policy_version: version });
        }
        Ok(())
    }
}

/// Configuration flags of the SELinux policy.
#[derive(Debug)]
pub(super) struct Config {
    handle_unknown: HandleUnknown,
    raw_flags: u32,
}

impl Config {
    /// Returns the [`HandleUnknown`] configuration.
    pub(super) fn handle_unknown(&self) -> HandleUnknown {
        self.handle_unknown
    }
}

impl Parse for Config {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let flags = u32::parse(cursor)?;

        // Reject if MLS is not enabled.
        let mls_enabled = (flags & CONFIG_MLS_FLAG) != 0;
        if !mls_enabled {
            return Err(ParseError::ConfigMissingMlsFlag { found_config: flags });
        }

        // Reject if invalid combination of handle_unknown bits (both Reject and Allow set).
        let masked_bits = flags & CONFIG_HANDLE_UNKNOWN_MASK;
        let handle_unknown = match masked_bits {
            CONFIG_HANDLE_UNKNOWN_REJECT_FLAG => HandleUnknown::Reject,
            CONFIG_HANDLE_UNKNOWN_ALLOW_FLAG => HandleUnknown::Allow,
            0 => HandleUnknown::Deny,
            _ => return Err(ParseError::InvalidConfigFlags { flags }),
        };

        // Store the rest of the flags.
        let raw_flags = flags & !CONFIG_HANDLE_UNKNOWN_MASK;

        Ok(Self { handle_unknown, raw_flags })
    }
}

impl Serialize for Config {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let mut flags = self.raw_flags;
        match self.handle_unknown {
            HandleUnknown::Reject => flags |= CONFIG_HANDLE_UNKNOWN_REJECT_FLAG,
            HandleUnknown::Allow => flags |= CONFIG_HANDLE_UNKNOWN_ALLOW_FLAG,
            HandleUnknown::Deny => {}
        }
        flags.serialize(writer)
    }
}

impl Validate for Config {
    fn validate(&self, _policy: &super::NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

/// Contains various count fields representing the size of different registries
/// and tables in the policy.
#[derive(Debug, Parse, Serialize, Validate)]
pub(super) struct Counts {
    /// Number of symbols in the symbol table.
    symbols_count: u32,
    /// Number of object contexts in the policy.
    object_context_count: u32,
}
