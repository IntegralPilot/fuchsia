// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::index::PolicyIndex;
use super::new::{CategorySetBuilder, Context, IdSpan, MlsLevel, MlsRange};
use super::{CategoryId, RoleId, TypeId, UserId};
use crate::NullessByteStr;
use crate::new_policy::NewPolicy;
use crate::new_policy::traits::{HasName, HasPolicyId};

use bstr::BString;

use thiserror::Error;

/// Security context, a variable-length string associated with each SELinux object in the
/// system. Contains mandatory `user:role:type` components and an optional
/// [:range] component.
///
/// Security contexts are configured by userspace atop Starnix, and mapped to
/// [`SecurityId`]s for internal use in Starnix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecurityContext {
    inner: Context,
}

impl SecurityContext {
    /// Returns a new instance with the specified field values.
    /// Fields are not validated against the policy until explicitly via `validate()`,
    /// or implicitly via insertion into a [`SidTable`].
    pub(super) fn new(
        user: UserId,
        role: RoleId,
        type_: TypeId,
        low_level: MlsLevel,
        high_level: Option<MlsLevel>,
    ) -> Self {
        let inner = Context::new(user, role, type_, MlsRange::new(low_level, high_level));
        Self { inner }
    }

    pub(super) fn from_policy_context(context: &Context) -> SecurityContext {
        SecurityContext { inner: context.clone() }
    }
}

impl std::ops::Deref for SecurityContext {
    type Target = Context;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl SecurityContext {
    /// Returns [`SecurityContext`] parsed from `security_context`, against the supplied
    /// `policy`. The returned structure is guaranteed to be valid for this `policy`.
    ///
    /// Security Contexts in Multi-Level Security (MLS) and Multi-Category Security (MCS)
    /// policies take the form:
    ///   context := <user>:<role>:<type>:<levels>
    /// such that they always include user, role, type, and a range of
    /// security levels.
    ///
    /// The security levels part consists of a "low" value and optional "high"
    /// value, defining the range.  In MCS policies each level may optionally be
    /// associated with a set of categories:
    /// categories:
    ///   levels := <level>[-<level>]
    ///   level := <sensitivity>[:<category_spec>[,<category_spec>]*]
    ///
    /// Entries in the optional list of categories may specify individual
    /// categories, or ranges (from low to high):
    ///   category_spec := <category>[.<category>]
    ///
    /// e.g. "u:r:t:s0" has a single (low) sensitivity.
    /// e.g. "u:r:t:s0-s1" has a sensitivity range.
    /// e.g. "u:r:t:s0:c1,c2,c3" has a single sensitivity, with three categories.
    /// e.g. "u:r:t:s0:c1-s1:c1,c2,c3" has a sensitivity range, with categories
    ///      associated with both low and high ends.
    ///
    /// Returns an error if the [`security_context`] is not a syntactically valid
    /// Security Context string, or the fields are not valid under the current policy.
    pub(super) fn from_string(
        policy_index: &PolicyIndex,
        security_context: NullessByteStr<'_>,
    ) -> Result<Self, SecurityContextError> {
        let as_str = std::str::from_utf8(security_context.as_bytes())
            .map_err(|_| SecurityContextError::InvalidSyntax)?;

        // Parse the user, role, type and security level parts, to validate syntax.
        let mut items = as_str.splitn(4, ":");
        let user = items.next().ok_or(SecurityContextError::InvalidSyntax)?;
        let role = items.next().ok_or(SecurityContextError::InvalidSyntax)?;
        let type_ = items.next().ok_or(SecurityContextError::InvalidSyntax)?;

        // `next()` holds the remainder of the string, if any.
        let mut levels = items.next().ok_or(SecurityContextError::InvalidSyntax)?.split("-");
        let low_level = levels.next().ok_or(SecurityContextError::InvalidSyntax)?;
        if low_level.is_empty() {
            return Err(SecurityContextError::InvalidSyntax);
        }
        let high_level = levels.next();
        if let Some(high_level) = high_level {
            if high_level.is_empty() {
                return Err(SecurityContextError::InvalidSyntax);
            }
        }
        if levels.next() != None {
            return Err(SecurityContextError::InvalidSyntax);
        }

        // Resolve the user, role, type and security levels to identifiers.
        let user = policy_index
            .users()
            .get_by_name(user.as_bytes())
            .ok_or_else(|| SecurityContextError::UnknownUser { name: user.into() })?
            .id();
        let role = policy_index
            .roles()
            .get_by_name(role.as_bytes())
            .ok_or_else(|| SecurityContextError::UnknownRole { name: role.into() })?
            .id();
        let type_ = policy_index
            .types()
            .get_by_name(type_.as_bytes())
            .ok_or_else(|| SecurityContextError::UnknownType { name: type_.into() })?
            .id();

        let low_level = MlsLevel::from_string(policy_index, low_level)?;
        let high_level = high_level.map(|x| MlsLevel::from_string(policy_index, x)).transpose()?;

        Ok(Self::new(user, role, type_, low_level, high_level))
    }

    /// Returns this [`SecurityContext`] serialized to a byte string.
    pub(super) fn to_string(&self, policy_index: &PolicyIndex) -> Vec<u8> {
        let mut levels = self.low_level().to_string(policy_index);
        if let Some(high_level) = self.high_level() {
            levels.push(b'-');
            levels.extend(high_level.to_string(policy_index));
        }
        let type_ = policy_index.types().get_by_id(self.type_()).unwrap();
        let parts: [&[u8]; 4] = [
            policy_index.users().get_by_id(self.user()).unwrap().name(),
            policy_index.roles().get_by_id(self.role()).unwrap().name(),
            type_.name(),
            levels.as_slice(),
        ];
        parts.join(b":".as_ref())
    }

    /// Validates that this [`SecurityContext`]'s fields are consistent with policy constraints
    /// (e.g. that the role is valid for the user).
    pub(super) fn validate(&self, policy_index: &PolicyIndex) -> Result<(), SecurityContextError> {
        let user = policy_index.users().get_by_id(self.user()).unwrap();

        // Validation of the user/role/type relationships is skipped for the special "object_r"
        // role, which is applied by default to non-process/socket-like resources.
        if self.role() != policy_index.object_role() {
            // Validate that the selected role is valid for this user.
            if !user.roles().contains(self.role()) {
                return Err(SecurityContextError::InvalidRoleForUser {
                    role: policy_index.roles().get_by_id(self.role()).unwrap().name().into(),
                    user: user.name().into(),
                });
            }

            // Validate that the selected type is valid for this role.
            let role = policy_index.roles().get_by_id(self.role()).unwrap();
            if !role.types().contains(self.type_()) {
                return Err(SecurityContextError::InvalidTypeForRole {
                    type_: policy_index.types().get_by_id(self.type_()).unwrap().name().into(),
                    role: role.name().into(),
                });
            }
        }

        // Check that the security context's MLS range is valid for the user (steps 1, 2,
        // and 3 below).
        let valid_low = user.mls_range().low();
        let valid_high = user.mls_range().high().as_ref().unwrap_or(valid_low);

        // 1. Check that the security context's low level is in the valid range for the user.
        if !(self.low_level().dominates(valid_low) && valid_high.dominates(self.low_level())) {
            return Err(SecurityContextError::InvalidLevelForUser {
                level: self.low_level().to_string(policy_index).into(),
                user: user.name().into(),
            });
        }
        if let Some(high_level) = self.high_level() {
            // 2. Check that the security context's high level is in the valid range for the user.
            if !(valid_high.dominates(high_level) && high_level.dominates(valid_low)) {
                return Err(SecurityContextError::InvalidLevelForUser {
                    level: high_level.to_string(policy_index).into(),
                    user: user.name().into(),
                });
            }

            // 3. Check that the security context's levels are internally consistent: i.e.,
            //    that the high level dominates the low level.
            if !high_level.dominates(self.low_level()) {
                return Err(SecurityContextError::InvalidSecurityRange {
                    low: self.low_level().to_string(policy_index).into(),
                    high: high_level.to_string(policy_index).into(),
                });
            }
        }
        Ok(())
    }
}

impl MlsLevel {
    /// Parses [`MlsLevel`] from the supplied string slice.
    pub(super) fn from_string(
        policy_index: &PolicyIndex,
        level: &str,
    ) -> Result<Self, SecurityContextError> {
        if level.is_empty() {
            return Err(SecurityContextError::InvalidSyntax);
        }

        // Parse the parts before looking up values, to catch invalid syntax.
        let mut items = level.split(":");
        let sensitivity = items.next().ok_or(SecurityContextError::InvalidSyntax)?;
        let categories_item = items.next();
        if items.next() != None {
            return Err(SecurityContextError::InvalidSyntax);
        }

        // Lookup the sensitivity, and associated categories/ranges, if any.
        let sensitivity = policy_index
            .sensitivities()
            .get_by_name(sensitivity.as_bytes())
            .ok_or_else(|| SecurityContextError::UnknownSensitivity { name: sensitivity.into() })?
            .id();

        let mut categories = CategorySetBuilder::new();
        if let Some(categories_str) = categories_item {
            for entry in categories_str.split(",") {
                if let Some((low_str, high_str)) = entry.split_once(".") {
                    let low = Self::category_id_by_name(policy_index, low_str)?;
                    let high = Self::category_id_by_name(policy_index, high_str)?;
                    if high <= low {
                        return Err(SecurityContextError::InvalidSyntax);
                    }
                    categories.insert_range(low, high);
                } else {
                    let id = Self::category_id_by_name(policy_index, entry)?;
                    categories.insert(id);
                };
            }
        }

        Ok(Self::new(sensitivity, categories.build()))
    }

    fn category_id_by_name(
        policy_index: &PolicyIndex,
        name: &str,
    ) -> Result<CategoryId, SecurityContextError> {
        Ok(policy_index
            .categories()
            .get_by_name(name.as_bytes())
            .ok_or_else(|| SecurityContextError::UnknownCategory { name: name.into() })?
            .id())
    }

    pub fn category_spans(&self) -> impl Iterator<Item = CategorySpan> + '_ {
        self.categories().spans()
    }

    pub fn to_string(&self, policy: &NewPolicy) -> Vec<u8> {
        let sensitivity = policy.sensitivities().get_by_id(self.sensitivity()).unwrap().name();
        let categories = self
            .category_spans()
            .map(|x| x.to_string(policy))
            .collect::<Vec<Vec<u8>>>()
            .join(b",".as_ref());

        if categories.is_empty() {
            sensitivity.to_vec()
        } else {
            [sensitivity, categories.as_slice()].join(b":".as_ref())
        }
    }
}

/// Describes an entry in a category specification, which may be a single category
/// (in which case `low` = `high`) or a span of consecutive categories. The bounds
/// are included in the span.
pub type CategorySpan = IdSpan<CategoryId>;

impl IdSpan<CategoryId> {
    /// Returns `Vec<u8>` describing the category, or category range.
    fn to_string(&self, policy: &NewPolicy) -> Vec<u8> {
        match self.low() == self.high() {
            true => policy.categories().get_by_id(self.low()).unwrap().name().into(),
            false => [
                policy.categories().get_by_id(self.low()).unwrap().name(),
                policy.categories().get_by_id(self.high()).unwrap().name(),
            ]
            .join(b".".as_ref()),
        }
    }
}

/// Errors that may be returned when attempting to parse or validate a security context.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SecurityContextError {
    #[error("security context syntax is invalid")]
    InvalidSyntax,
    #[error("sensitivity {name:?} not defined by policy")]
    UnknownSensitivity { name: BString },
    #[error("category {name:?} not defined by policy")]
    UnknownCategory { name: BString },
    #[error("user {name:?} not defined by policy")]
    UnknownUser { name: BString },
    #[error("role {name:?} not defined by policy")]
    UnknownRole { name: BString },
    #[error("type {name:?} not defined by policy")]
    UnknownType { name: BString },
    #[error("role {role:?} not valid for {user:?}")]
    InvalidRoleForUser { role: BString, user: BString },
    #[error("type {type_:?} not valid for {role:?}")]
    InvalidTypeForRole { role: BString, type_: BString },
    #[error("security level {level:?} not valid for {user:?}")]
    InvalidLevelForUser { level: BString, user: BString },
    #[error("high security level {high:?} lower than low level {low:?}")]
    InvalidSecurityRange { low: BString, high: BString },
}

#[cfg(test)]
mod tests {
    use super::super::new::CategorySet;
    use super::super::{Policy, PolicyId, SensitivityId, parse_policy_by_value};
    use super::*;
    use std::cmp::Ordering;

    fn test_policy() -> Policy {
        const TEST_POLICY: &[u8] =
            include_bytes!("../../testdata/micro_policies/security_context_tests_policy");
        parse_policy_by_value(TEST_POLICY.to_vec()).unwrap().validate().unwrap()
    }

    // CategoryItem helper for tests.
    #[derive(Debug, Eq, PartialEq)]
    struct CategoryItem {
        low: String,
        high: String,
    }

    fn user_name(policy: &Policy, id: UserId) -> &str {
        std::str::from_utf8(policy.users().get_by_id(id).unwrap().name()).unwrap()
    }

    fn role_name(policy: &Policy, id: RoleId) -> &str {
        std::str::from_utf8(policy.roles().get_by_id(id).unwrap().name()).unwrap()
    }

    fn type_name(policy: &Policy, id: TypeId) -> &str {
        std::str::from_utf8(policy.types().get_by_id(id).unwrap().name()).unwrap()
    }

    fn sensitivity_name(policy: &Policy, id: SensitivityId) -> &str {
        std::str::from_utf8(policy.sensitivities().get_by_id(id).unwrap().name()).unwrap()
    }

    fn category_name(policy: &Policy, id: CategoryId) -> &str {
        std::str::from_utf8(policy.categories().get_by_id(id).unwrap().name()).unwrap()
    }

    fn category_span(policy: &Policy, category: &CategorySpan) -> CategoryItem {
        CategoryItem {
            low: category_name(policy, category.low()).into(),
            high: category_name(policy, category.high()).into(),
        }
    }

    fn category_spans(
        policy: &Policy,
        iter: impl Iterator<Item = CategorySpan>,
    ) -> Vec<CategoryItem> {
        iter.map(|x| category_span(policy, &x)).collect()
    }

    // Creates a category range for testing.
    fn cat(low: u32, high: u32) -> CategorySpan {
        CategorySpan::new(
            CategoryId::from_u32(low).expect("category ids are nonzero"),
            CategoryId::from_u32(high).expect("category ids are nonzero"),
        )
    }

    // Compares two sets of categories for testing.
    fn compare(lhs: &[CategorySpan], rhs: &[CategorySpan]) -> Option<Ordering> {
        let lhs_set = CategorySet::from_ids(lhs.iter().flat_map(|span| {
            (span.low().as_u32()..=span.high().as_u32()).map(|i| CategoryId::from_u32(i).unwrap())
        }));
        let rhs_set = CategorySet::from_ids(rhs.iter().flat_map(|span| {
            (span.low().as_u32()..=span.high().as_u32()).map(|i| CategoryId::from_u32(i).unwrap())
        }));
        lhs_set.compare(&rhs_set)
    }

    #[test]
    fn category_compare() {
        let cat_1 = cat(1, 1);
        let cat_2 = cat(1, 3);
        let cat_3 = cat(2, 3);
        assert_eq!(compare(&[cat_1.clone()], &[cat_1.clone()]), Some(Ordering::Equal));
        assert_eq!(compare(&[cat_1.clone()], &[cat_2.clone()]), Some(Ordering::Less));
        assert_eq!(compare(&[cat_1.clone()], &[cat_3.clone()]), None);
        assert_eq!(compare(&[cat_2.clone()], &[cat_1.clone()]), Some(Ordering::Greater));
        assert_eq!(compare(&[cat_2.clone()], &[cat_3.clone()]), Some(Ordering::Greater));
    }

    #[test]
    fn categories_compare_empty_iter() {
        let cats_0 = &[];
        let cats_1 = &[cat(1, 1)];
        assert_eq!(compare(cats_0, cats_0), Some(Ordering::Equal));
        assert_eq!(compare(cats_0, cats_1), Some(Ordering::Less));
        assert_eq!(compare(cats_1, cats_0), Some(Ordering::Greater));
    }

    #[test]
    fn categories_compare_same_length() {
        let cats_1 = &[cat(1, 1), cat(3, 3)];
        let cats_2 = &[cat(1, 1), cat(4, 4)];
        let cats_3 = &[cat(1, 2), cat(4, 4)];
        let cats_4 = &[cat(1, 2), cat(4, 5)];

        assert_eq!(compare(cats_1, cats_1), Some(Ordering::Equal));
        assert_eq!(compare(cats_1, cats_2), None);
        assert_eq!(compare(cats_1, cats_3), None);
        assert_eq!(compare(cats_1, cats_4), None);

        assert_eq!(compare(cats_2, cats_1), None);
        assert_eq!(compare(cats_2, cats_2), Some(Ordering::Equal));
        assert_eq!(compare(cats_2, cats_3), Some(Ordering::Less));
        assert_eq!(compare(cats_2, cats_4), Some(Ordering::Less));

        assert_eq!(compare(cats_3, cats_1), None);
        assert_eq!(compare(cats_3, cats_2), Some(Ordering::Greater));
        assert_eq!(compare(cats_3, cats_3), Some(Ordering::Equal));
        assert_eq!(compare(cats_3, cats_4), Some(Ordering::Less));

        assert_eq!(compare(cats_4, cats_1), None);
        assert_eq!(compare(cats_4, cats_2), Some(Ordering::Greater));
        assert_eq!(compare(cats_4, cats_3), Some(Ordering::Greater));
        assert_eq!(compare(cats_4, cats_4), Some(Ordering::Equal));
    }

    #[test]
    fn categories_compare_different_lengths() {
        let cats_1 = &[cat(1, 1)];
        let cats_2 = &[cat(1, 4)];
        let cats_3 = &[cat(1, 1), cat(4, 4)];
        let cats_4 = &[cat(1, 2), cat(4, 5), cat(7, 7)];

        assert_eq!(compare(cats_1, cats_3), Some(Ordering::Less));
        assert_eq!(compare(cats_1, cats_4), Some(Ordering::Less));

        assert_eq!(compare(cats_2, cats_3), Some(Ordering::Greater));
        assert_eq!(compare(cats_2, cats_4), None);

        assert_eq!(compare(cats_3, cats_1), Some(Ordering::Greater));
        assert_eq!(compare(cats_3, cats_2), Some(Ordering::Less));
        assert_eq!(compare(cats_3, cats_4), Some(Ordering::Less));

        assert_eq!(compare(cats_4, cats_1), Some(Ordering::Greater));
        assert_eq!(compare(cats_4, cats_2), None);
        assert_eq!(compare(cats_4, cats_3), Some(Ordering::Greater));
    }

    #[test]
    // Test cases where one interval appears before or after all intervals of the
    // other set, or in a gap between intervals of the other set.
    fn categories_compare_with_gaps() {
        let cats_1 = &[cat(1, 2), cat(4, 5)];
        let cats_2 = &[cat(4, 5)];
        let cats_3 = &[cat(2, 5), cat(10, 11)];
        let cats_4 = &[cat(2, 5), cat(7, 8), cat(10, 11)];

        assert_eq!(compare(cats_1, cats_2), Some(Ordering::Greater));
        assert_eq!(compare(cats_1, cats_3), None);
        assert_eq!(compare(cats_1, cats_4), None);

        assert_eq!(compare(cats_2, cats_1), Some(Ordering::Less));
        assert_eq!(compare(cats_2, cats_3), Some(Ordering::Less));
        assert_eq!(compare(cats_2, cats_4), Some(Ordering::Less));

        assert_eq!(compare(cats_3, cats_1), None);
        assert_eq!(compare(cats_3, cats_2), Some(Ordering::Greater));
        assert_eq!(compare(cats_3, cats_4), Some(Ordering::Less));

        assert_eq!(compare(cats_4, cats_1), None);
        assert_eq!(compare(cats_4, cats_2), Some(Ordering::Greater));
        assert_eq!(compare(cats_4, cats_3), Some(Ordering::Greater));
    }

    #[test]
    fn parse_security_context_single_sensitivity() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"user0:object_r:type0:s0".into())
            .expect("creating security context should succeed");
        assert_eq!(user_name(&policy, security_context.user()), "user0");
        assert_eq!(role_name(&policy, security_context.role()), "object_r");
        assert_eq!(type_name(&policy, security_context.type_()), "type0");
        assert_eq!(sensitivity_name(&policy, security_context.low_level().sensitivity()), "s0");
        assert!(category_spans(&policy, security_context.low_level().category_spans()).is_empty());
        assert_eq!(security_context.high_level(), None);
    }

    #[test]
    fn parse_security_context_with_sensitivity_range() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"user0:object_r:type0:s0-s1".into())
            .expect("creating security context should succeed");
        assert_eq!(user_name(&policy, security_context.user()), "user0");
        assert_eq!(role_name(&policy, security_context.role()), "object_r");
        assert_eq!(type_name(&policy, security_context.type_()), "type0");
        assert_eq!(sensitivity_name(&policy, security_context.low_level().sensitivity()), "s0");
        assert!(category_spans(&policy, security_context.low_level().category_spans()).is_empty());
        let high_level = security_context.high_level().unwrap();
        assert_eq!(sensitivity_name(&policy, high_level.sensitivity()), "s1");
        assert!(category_spans(&policy, high_level.category_spans()).is_empty());
    }

    #[test]
    fn parse_security_context_with_single_sensitivity_and_categories_interval() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"user0:object_r:type0:s1:c0.c4".into())
            .expect("creating security context should succeed");
        assert_eq!(user_name(&policy, security_context.user()), "user0");
        assert_eq!(role_name(&policy, security_context.role()), "object_r");
        assert_eq!(type_name(&policy, security_context.type_()), "type0");
        assert_eq!(sensitivity_name(&policy, security_context.low_level().sensitivity()), "s1");
        assert_eq!(
            category_spans(&policy, security_context.low_level().category_spans()),
            [CategoryItem { low: "c0".to_string(), high: "c4".to_string() }]
        );
        assert_eq!(security_context.high_level(), None);
    }

    #[test]
    fn parse_security_context_and_normalize_categories() {
        let policy = &test_policy();
        let normalize = {
            |security_context: &str| -> String {
                String::from_utf8(
                    policy.serialize_security_context(
                        &policy
                            .parse_security_context(security_context.into())
                            .expect("creating security context should succeed"),
                    ),
                )
                .unwrap()
            }
        };
        // Overlapping category ranges are merged.
        assert_eq!(normalize("user0:object_r:type0:s1:c0.c1,c1"), "user0:object_r:type0:s1:c0.c1");
        assert_eq!(
            normalize("user0:object_r:type0:s1:c0.c2,c1.c2"),
            "user0:object_r:type0:s1:c0.c2"
        );
        assert_eq!(
            normalize("user0:object_r:type0:s1:c0.c2,c1.c3"),
            "user0:object_r:type0:s1:c0.c3"
        );
        // Adjacent category ranges are merged.
        assert_eq!(normalize("user0:object_r:type0:s1:c0.c1,c2"), "user0:object_r:type0:s1:c0.c2");
        // Category ranges are ordered by first element.
        assert_eq!(
            normalize("user0:object_r:type0:s1:c2.c3,c0"),
            "user0:object_r:type0:s1:c0,c2.c3"
        );
    }

    #[test]
    fn parse_security_context_with_sensitivity_range_and_category_interval() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"user0:object_r:type0:s0-s1:c0.c4".into())
            .expect("creating security context should succeed");
        assert_eq!(user_name(&policy, security_context.user()), "user0");
        assert_eq!(role_name(&policy, security_context.role()), "object_r");
        assert_eq!(type_name(&policy, security_context.type_()), "type0");
        assert_eq!(sensitivity_name(&policy, security_context.low_level().sensitivity()), "s0");
        assert!(category_spans(&policy, security_context.low_level().category_spans()).is_empty());
        let high_level = security_context.high_level().unwrap();
        assert_eq!(sensitivity_name(&policy, high_level.sensitivity()), "s1");
        assert_eq!(
            category_spans(&policy, high_level.category_spans()),
            [CategoryItem { low: "c0".to_string(), high: "c4".to_string() }]
        );
    }

    #[test]
    fn parse_security_context_with_sensitivity_range_with_categories() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"user0:object_r:type0:s0:c0-s1:c0.c4".into())
            .expect("creating security context should succeed");
        assert_eq!(user_name(&policy, security_context.user()), "user0");
        assert_eq!(role_name(&policy, security_context.role()), "object_r");
        assert_eq!(type_name(&policy, security_context.type_()), "type0");
        assert_eq!(sensitivity_name(&policy, security_context.low_level().sensitivity()), "s0");
        assert_eq!(
            category_spans(&policy, security_context.low_level().category_spans()),
            [CategoryItem { low: "c0".to_string(), high: "c0".to_string() }]
        );

        let high_level = security_context.high_level().unwrap();
        assert_eq!(sensitivity_name(&policy, high_level.sensitivity()), "s1");
        assert_eq!(
            category_spans(&policy, high_level.category_spans()),
            [CategoryItem { low: "c0".to_string(), high: "c4".to_string() }]
        );
    }

    #[test]
    fn parse_security_context_with_single_sensitivity_and_category_list() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"user0:object_r:type0:s1:c0,c4".into())
            .expect("creating security context should succeed");
        assert_eq!(user_name(&policy, security_context.user()), "user0");
        assert_eq!(role_name(&policy, security_context.role()), "object_r");
        assert_eq!(type_name(&policy, security_context.type_()), "type0");
        assert_eq!(sensitivity_name(&policy, security_context.low_level().sensitivity()), "s1");
        assert_eq!(
            category_spans(&policy, security_context.low_level().category_spans()),
            [
                CategoryItem { low: "c0".to_string(), high: "c0".to_string() },
                CategoryItem { low: "c4".to_string(), high: "c4".to_string() }
            ]
        );
        assert_eq!(security_context.high_level(), None);
    }

    #[test]
    fn parse_security_context_with_single_sensitivity_and_category_list_and_range() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"user0:object_r:type0:s1:c0,c3.c4".into())
            .expect("creating security context should succeed");
        assert_eq!(user_name(&policy, security_context.user()), "user0");
        assert_eq!(role_name(&policy, security_context.role()), "object_r");
        assert_eq!(type_name(&policy, security_context.type_()), "type0");
        assert_eq!(sensitivity_name(&policy, security_context.low_level().sensitivity()), "s1");
        assert_eq!(
            category_spans(&policy, security_context.low_level().category_spans()),
            [
                CategoryItem { low: "c0".to_string(), high: "c0".to_string() },
                CategoryItem { low: "c3".to_string(), high: "c4".to_string() }
            ]
        );
        assert_eq!(security_context.high_level(), None);
    }

    #[test]
    fn parse_invalid_syntax() {
        let policy = test_policy();
        for invalid_label in [
            "user0",
            "user0:object_r",
            "user0:object_r:type0",
            "user0:object_r:type0:s0-",
            "user0:object_r:type0:s0:s0:s0",
            "user0:object_r:type0:s0:c0.c0", // Category upper bound is equal to lower bound.
            "user0:object_r:type0:s0:c1.c0", // Category upper bound is less than lower bound.
        ] {
            assert_eq!(
                policy.parse_security_context(invalid_label.as_bytes().into()),
                Err(SecurityContextError::InvalidSyntax),
                "validating {:?}",
                invalid_label
            );
        }
    }

    #[test]
    fn parse_invalid_sensitivity() {
        let policy = test_policy();
        for invalid_label in ["user0:object_r:type0:s_invalid", "user0:object_r:type0:s0-s_invalid"]
        {
            assert_eq!(
                policy.parse_security_context(invalid_label.as_bytes().into()),
                Err(SecurityContextError::UnknownSensitivity { name: "s_invalid".into() }),
                "validating {:?}",
                invalid_label
            );
        }
    }

    #[test]
    fn parse_invalid_category() {
        let policy = test_policy();
        for invalid_label in
            ["user0:object_r:type0:s1:c_invalid", "user0:object_r:type0:s1:c0.c_invalid"]
        {
            assert_eq!(
                policy.parse_security_context(invalid_label.as_bytes().into()),
                Err(SecurityContextError::UnknownCategory { name: "c_invalid".into() }),
                "validating {:?}",
                invalid_label
            );
        }
    }

    #[test]
    fn invalid_security_context_fields() {
        let policy = test_policy();

        // Fails validation because the security context's high level does not dominate its
        // low level: the low level has categories that the high level does not.
        let context = policy
            .parse_security_context(b"user0:object_r:type0:s1:c0,c3.c4-s1".into())
            .expect("successfully parsed");
        assert_eq!(
            policy.validate_security_context(&context),
            Err(SecurityContextError::InvalidSecurityRange {
                low: "s1:c0,c3.c4".into(),
                high: "s1".into()
            })
        );

        // Fails validation because the security context's high level does not dominate its
        // low level: the category sets of the high level and low level are not comparable.
        let context = policy
            .parse_security_context(b"user0:object_r:type0:s1:c0-s1:c1".into())
            .expect("successfully parsed");
        assert_eq!(
            policy.validate_security_context(&context),
            Err(SecurityContextError::InvalidSecurityRange {
                low: "s1:c0".into(),
                high: "s1:c1".into()
            })
        );

        // Fails validation because the security context's high level does not dominate its
        // low level: the sensitivity of the high level is lower than that of the low level.
        let context = policy
            .parse_security_context(b"user0:object_r:type0:s1:c0-s0:c0.c1".into())
            .expect("successfully parsed");
        assert_eq!(
            policy.validate_security_context(&context),
            Err(SecurityContextError::InvalidSecurityRange {
                low: "s1:c0".into(),
                high: "s0:c0.c1".into()
            })
        );

        // Fails validation because the policy's high level does not dominate the
        // security context's high level: the security context's high level has categories
        // that the policy's high level does not.
        let context = policy
            .parse_security_context(b"user1:subject_r:type0:s1-s1:c3".into())
            .expect("successfully parsed");
        assert_eq!(
            policy.validate_security_context(&context),
            Err(SecurityContextError::InvalidLevelForUser {
                level: "s1:c3".into(),
                user: "user1".into(),
            })
        );

        // Fails validation because the security context's low level does not dominate
        // the policy's low level: the security context's low level has a lower sensitivity
        // than the policy's low level.
        let context = policy
            .parse_security_context(b"user1:object_r:type0:s0".into())
            .expect("successfully parsed");
        assert_eq!(
            policy.validate_security_context(&context),
            Err(SecurityContextError::InvalidLevelForUser {
                level: "s0".into(),
                user: "user1".into(),
            })
        );

        // Fails validation because the sensitivity is not valid for the user.
        let context = policy
            .parse_security_context(b"user1:object_r:type0:s0".into())
            .expect("successfully parsed");
        assert!(policy.validate_security_context(&context).is_err());

        // Fails validation because the role is not valid for the user.
        let context = policy
            .parse_security_context(b"user0:subject_r:type0:s0".into())
            .expect("successfully parsed");
        assert!(policy.validate_security_context(&context).is_err());

        // Fails validation because the type is not valid for the role.
        let context = policy
            .parse_security_context(b"user1:subject_r:non_subject_t:s1".into())
            .expect("successfully parsed");
        assert!(policy.validate_security_context(&context).is_err());

        // Passes validation even though the role is not explicitly allowed for the user,
        // because it is the special "object_r" role, used when labelling resources.
        let context = policy
            .parse_security_context(b"user1:object_r:type0:s1".into())
            .expect("successfully parsed");
        assert!(policy.validate_security_context(&context).is_ok());
    }

    #[test]
    fn format_security_contexts() {
        let policy = test_policy();
        for label in [
            "user0:object_r:type0:s0",
            "user0:object_r:type0:s0-s1",
            "user0:object_r:type0:s1:c0.c4",
            "user0:object_r:type0:s0-s1:c0.c4",
            "user0:object_r:type0:s1:c0,c3",
            "user0:object_r:type0:s0-s1:c0,c2,c4",
            "user0:object_r:type0:s1:c0,c3.c4-s1:c0,c2.c4",
        ] {
            let security_context =
                policy.parse_security_context(label.as_bytes().into()).expect("should succeed");
            assert_eq!(policy.serialize_security_context(&security_context), label.as_bytes());
        }
    }
}
