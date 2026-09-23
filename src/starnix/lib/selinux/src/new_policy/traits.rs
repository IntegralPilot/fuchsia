// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Debug;
use std::hash::Hash;

use super::NewPolicy;
use super::error::{ParseError, SerializeError, ValidateError};
use super::parser::{PolicyCursor, PolicyWriter};

/// Trait for types that can be parsed from a [`PolicyCursor`].
pub trait Parse: Sized {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError>;
}

/// Trait for types that can be serialized into a byte vector.
pub trait Serialize {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError>;
}

/// Trait for types that can be validated against the parsed policy.
pub trait Validate {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError>;
}

/// Trait for strongly-typed policy identifiers.
///
/// Types implementing [`PolicyId`] can be parsed from and serialized to `u32` values
/// in the binary policy database, but are represented as strongly-typed integers
/// (often wrapping `NonZeroU16` or `NonZeroU32`) in the logical domain model.
pub trait PolicyId: Copy + Clone + Debug + Eq + Hash + PartialEq {
    /// Returns the raw `u32` value of the ID.
    fn as_u32(&self) -> u32;

    /// Constructs an instance of [`Self`] from a raw `u32` value, returning [`None`]
    /// if the value is invalid (e.g. zero for a non-optional ID, or out of range).
    fn from_u32(value: u32) -> Option<Self>;
}

// Blanket implementations for all strongly-typed IDs
impl<T: PolicyId> Parse for T {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let value = u32::parse(cursor)?;
        T::from_u32(value).ok_or(ParseError::InvalidId { value })
    }
}

impl<T: PolicyId> Serialize for T {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        self.as_u32().serialize(writer)
    }
}

impl<T: PolicyId> Parse for Option<T> {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let value = u32::parse(cursor)?;
        if value == 0 {
            Ok(None)
        } else {
            T::from_u32(value).map(Some).ok_or(ParseError::InvalidId { value })
        }
    }
}

impl<T: PolicyId> Serialize for Option<T> {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        match self {
            Some(id) => id.as_u32().serialize(writer),
            None => 0u32.serialize(writer),
        }
    }
}

/// Trait for policy elements with a byte slice name.
pub trait HasName {
    fn name(&self) -> &[u8];
}

/// Trait for policy elements that have a strongly-typed policy identifier.
pub trait HasPolicyId {
    type Id: PolicyId;
    fn id(&self) -> Self::Id;
}

impl<T: Validate> Validate for Box<T> {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        (**self).validate(policy)
    }
}

impl Validate for bool {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

impl<T: Validate> Validate for Option<T> {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        if let Some(value) = self {
            value.validate(policy)?;
        }
        Ok(())
    }
}
