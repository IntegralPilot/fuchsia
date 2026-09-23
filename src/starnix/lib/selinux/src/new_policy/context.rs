// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cmp::Ordering;

use selinux_policy_derive::{Parse, Serialize, Validate};

use super::error::{ParseError, SerializeError, ValidateError};
use super::parser::{PolicyCursor, PolicyWriter};
use super::traits::{Parse, Serialize, Validate};
use super::{CategoryId, CategorySet, RoleId, SensitivityId, TypeId, UserId};

/// Security level in MLS (Multi-Level Security), consisting of a sensitivity and a set of categories.
#[derive(Debug, Clone, PartialEq, Eq, Parse, Serialize, Validate)]
pub struct MlsLevel {
    sensitivity: SensitivityId,
    categories: CategorySet,
}

impl MlsLevel {
    /// Constructs a new [`MlsLevel`].
    pub fn new(sensitivity: SensitivityId, categories: CategorySet) -> Self {
        Self { sensitivity, categories }
    }

    /// Returns the sensitivity level.
    pub fn sensitivity(&self) -> SensitivityId {
        self.sensitivity
    }

    /// Returns the set of categories.
    pub fn categories(&self) -> &CategorySet {
        &self.categories
    }

    /// Returns an iterator over the category IDs.
    pub fn category_ids(&self) -> impl Iterator<Item = CategoryId> + use<'_> {
        self.categories.iter()
    }

    /// Compares two [`MlsLevel`]s. Returns [`None`] if they are incomparable.
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        let s_order = self.sensitivity().cmp(&other.sensitivity());
        let c_order = self.categories().compare(other.categories())?;
        if s_order == c_order {
            return Some(s_order);
        } else if c_order == Ordering::Equal {
            return Some(s_order);
        } else if s_order == Ordering::Equal {
            return Some(c_order);
        }
        None
    }

    /// Returns `true` if `self` dominates `other` (i.e. is greater than or equal to).
    pub fn dominates(&self, other: &Self) -> bool {
        self.sensitivity() >= other.sensitivity()
            && self.categories().is_superset(other.categories())
    }
}

/// Security range in MLS, consisting of a low level and an optional high level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsRange {
    low: MlsLevel,
    high: Option<MlsLevel>,
}

impl Parse for MlsRange {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let levels_count = u32::parse(cursor)?;
        let sensitivity_low = SensitivityId::parse(cursor)?;

        // If levels_count > 1, the MLS range contains both low and high security levels.
        if levels_count > 1 {
            let sensitivity_high = SensitivityId::parse(cursor)?;
            let low_categories = CategorySet::parse(cursor)?;
            let high_categories = CategorySet::parse(cursor)?;

            Ok(Self {
                low: MlsLevel { sensitivity: sensitivity_low, categories: low_categories },
                high: Some(MlsLevel { sensitivity: sensitivity_high, categories: high_categories }),
            })
        } else {
            let low_categories = CategorySet::parse(cursor)?;
            Ok(Self {
                low: MlsLevel { sensitivity: sensitivity_low, categories: low_categories },
                high: None,
            })
        }
    }
}

impl Serialize for MlsRange {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        if let Some(ref high) = self.high {
            2u32.serialize(writer)?;
            self.low.sensitivity.serialize(writer)?;
            high.sensitivity.serialize(writer)?;
            self.low.categories.serialize(writer)?;
            high.categories.serialize(writer)?;
        } else {
            1u32.serialize(writer)?;
            self.low.sensitivity.serialize(writer)?;
            self.low.categories.serialize(writer)?;
        }
        Ok(())
    }
}

impl Validate for MlsRange {
    fn validate(&self, policy: &super::NewPolicy) -> Result<(), ValidateError> {
        self.low.validate(policy)?;
        if let Some(ref high) = self.high {
            high.validate(policy)?;
            if !high.dominates(&self.low) {
                return Err(ValidateError::InvalidMlsRange);
            }
        }
        Ok(())
    }
}

impl MlsRange {
    /// Constructs a new [`MlsRange`].
    pub fn new(low: MlsLevel, high: Option<MlsLevel>) -> Self {
        Self { low, high }
    }

    /// Returns the low security level.
    pub fn low(&self) -> &MlsLevel {
        &self.low
    }

    /// Returns the high security level, if present.
    pub fn high(&self) -> &Option<MlsLevel> {
        &self.high
    }
}

/// Security context containing user, role, type, and MLS range.
#[derive(Debug, Clone, PartialEq, Eq, Parse, Serialize, Validate)]
pub struct Context {
    user: UserId,
    role: RoleId,
    context_type: TypeId,
    mls_range: MlsRange,
}

impl Context {
    /// Constructs a new [`Context`].
    pub fn new(user: UserId, role: RoleId, context_type: TypeId, mls_range: MlsRange) -> Self {
        Self { user, role, context_type, mls_range }
    }

    /// Returns the user ID.
    pub fn user(&self) -> UserId {
        self.user
    }

    /// Returns the role ID.
    pub fn role(&self) -> RoleId {
        self.role
    }

    /// Returns the type ID.
    pub fn type_(&self) -> TypeId {
        self.context_type
    }

    /// Returns the low security level.
    pub fn low_level(&self) -> &MlsLevel {
        &self.mls_range.low
    }

    /// Returns the high security level, if present.
    pub fn high_level(&self) -> Option<&MlsLevel> {
        self.mls_range.high.as_ref()
    }

    /// Returns the high level if distinct from the low level, or else returns the low level.
    pub fn effective_high_level(&self) -> &MlsLevel {
        self.high_level().unwrap_or_else(|| self.low_level())
    }
}
