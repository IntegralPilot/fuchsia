// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::constraints::evaluate_constraint;
use super::error::{ParseError, ValidateError};
use super::parser::PolicyData;
use super::security_context::SecurityContext;
use super::{
    AccessDecision, AccessVector, ClassId, SELINUX_AVD_FLAGS_PERMISSIVE, TypeId,
    XpermsAccessDecision, XpermsKind,
};
use crate::PolicyCap;
use crate::new_policy::rules::{
    ExtendedPermissions, HasRuleKey, RuleKind, XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES,
    XPERMS_TYPE_IOCTL_PREFIXES, XPERMS_TYPE_NLMSG, XpermsBitmap,
};
use crate::new_policy::traits::HasPolicyId;
use crate::new_policy::{Class, GenfsConPath, NewPolicy};
use std::ops::Deref;
use std::sync::Arc;

use anyhow::Context as _;
use std::iter::Iterator;

// As of 2026-01-30, more than five times larger than any policy seen in production or tests.
const MAXIMUM_POLICY_SIZE: usize = 1 << 24;

/// Parsed binary policy.
#[derive(Debug)]
pub struct ParsedPolicy {
    /// [`NewPolicy`] that handles the policy tables and verification.
    new_policy: Arc<NewPolicy>,
}

impl Deref for ParsedPolicy {
    type Target = NewPolicy;
    fn deref(&self) -> &Self::Target {
        &self.new_policy
    }
}

impl ParsedPolicy {
    /// Returns true if the specified capability is in the policy's enabled capabilities set.
    pub fn has_policycap(&self, policy_cap: PolicyCap) -> bool {
        self.new_policy.policy_capabilities().contains(policy_cap)
    }

    /// Computes the access granted to `source_type` on `target_type`, for the specified
    /// `target_class`. The result is a set of access vectors with bits set for each
    /// `target_class` permission, describing which permissions are allowed, and
    /// which should have access checks audit-logged when denied, or allowed.
    ///
    /// An [`AccessDecision`] is accumulated, starting from no permissions to be granted,
    /// nor audit-logged if allowed, and all permissions to be audit-logged if denied.
    /// Permissions that are explicitly `allow`ed, but that are subject to unsatisfied
    /// constraints, are removed from the allowed set. Matching policy statements then
    /// add permissions to the granted & audit-allow sets, or remove them from the
    /// audit-deny set.
    pub(super) fn compute_access_decision(
        &self,
        source_context: &SecurityContext,
        target_context: &SecurityContext,
        target_class: &Class,
    ) -> AccessDecision {
        let mut access_decision = self.compute_explicitly_allowed(
            source_context.type_(),
            target_context.type_(),
            target_class,
        );
        access_decision.allow -=
            self.compute_denied_by_constraints(source_context, target_context, target_class);
        access_decision
    }

    /// Computes the access granted to `source_type` on `target_type`, for the specified
    /// `target_class`. The result is a set of access vectors with bits set for each
    /// `target_class` permission, describing which permissions are explicitly allowed,
    /// and which should have access checks audit-logged when denied, or allowed.
    pub(super) fn compute_explicitly_allowed(
        &self,
        source_type: TypeId,
        target_type: TypeId,
        target_class: &Class,
    ) -> AccessDecision {
        let target_class_id = target_class.id();

        let mut computed_access_vector = AccessVector::NONE;
        let mut computed_audit_allow = AccessVector::NONE;
        let mut computed_audit_deny = AccessVector::ALL;

        let source_attribute_set = &self.type_attribute_maps()[source_type];
        let target_attribute_set = &self.type_attribute_maps()[target_type];

        for source_id in source_attribute_set.iter() {
            for target_id in target_attribute_set.iter() {
                for rule in self.new_policy.access_vector_rules().find_av_rules(
                    source_id,
                    target_id,
                    target_class_id,
                ) {
                    match rule.kind() {
                        RuleKind::Allow => computed_access_vector |= rule.access_vector(),
                        RuleKind::AuditAllow => computed_audit_allow |= rule.access_vector(),
                        RuleKind::DontAudit => computed_audit_deny &= rule.access_vector(),
                        _ => {}
                    }
                }
            }
        }

        // If the `source_type` is bounded by some `parent_type` then bound the allowed permissions
        // to those available to the parent. Doing the calculation here ensures that type-bounds
        // take into account bounding ancestors, if any.
        if let Some(parent) = self.types().get_by_id(source_type).unwrap().bounded_by() {
            // If `source_type`==`target_type` then this is a "self" permission check, which should
            // be bounded to the parent domain's "self" permissions.
            let access = if source_type == target_type {
                self.compute_explicitly_allowed(parent, parent, target_class)
            } else {
                self.compute_explicitly_allowed(parent, target_type, target_class)
            };
            computed_access_vector &= access.allow;
        }

        let mut flags = 0;
        if self.permissive_map().contains(source_type) {
            flags |= SELINUX_AVD_FLAGS_PERMISSIVE;
        }
        AccessDecision {
            allow: computed_access_vector,
            auditallow: computed_audit_allow,
            auditdeny: computed_audit_deny,
            flags,
            todo_bug: None,
        }
    }

    /// A permission is denied if it matches at least one unsatisfied constraint.
    fn compute_denied_by_constraints(
        &self,
        source_context: &SecurityContext,
        target_context: &SecurityContext,
        target_class: &Class,
    ) -> AccessVector {
        let mut denied = AccessVector::NONE;
        for constraint in target_class.constraints() {
            if !evaluate_constraint(constraint.constraint_expr(), source_context, target_context) {
                denied |= constraint.access_vector();
            }
        }
        denied
    }

    /// Computes the access decision for set of extended permissions of a given kind and with a
    /// given prefix byte, for a particular source and target context and target class.
    pub(super) fn compute_xperms_access_decision(
        &self,
        xperms_kind: XpermsKind,
        source_context: &SecurityContext,
        target_context: &SecurityContext,
        target_class: &Class,
        xperms_prefix: u8,
    ) -> XpermsAccessDecision {
        let target_class_id = target_class.id();

        let mut explicit_allow: Option<XpermsBitmap> = None;
        let mut auditallow = XpermsBitmap::NONE;
        let mut auditdeny = XpermsBitmap::ALL;

        let xperms_types = match xperms_kind {
            XpermsKind::Ioctl => {
                [XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES, XPERMS_TYPE_IOCTL_PREFIXES].as_slice()
            }
            XpermsKind::Nlmsg => [XPERMS_TYPE_NLMSG].as_slice(),
        };
        let bitmap_if_prefix_matches =
            |xperms_prefix: u8, xperms: &ExtendedPermissions| match xperms_kind {
                XpermsKind::Ioctl => match xperms.xperms_type() {
                    XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES => (xperms.xperms_optional_prefix()
                        == xperms_prefix)
                        .then_some(*xperms.xperms_bitmap()),
                    XPERMS_TYPE_IOCTL_PREFIXES => {
                        xperms.xperms_bitmap().contains(xperms_prefix).then_some(XpermsBitmap::ALL)
                    }
                    _ => None,
                },
                XpermsKind::Nlmsg => match xperms.xperms_type() {
                    XPERMS_TYPE_NLMSG => (xperms.xperms_optional_prefix() == xperms_prefix)
                        .then_some(*xperms.xperms_bitmap()),
                    _ => None,
                },
            };

        let source_attribute_set = &self.type_attribute_maps()[source_context.type_()];
        let target_attribute_set = &self.type_attribute_maps()[target_context.type_()];

        for source_id in source_attribute_set.iter() {
            for target_id in target_attribute_set.iter() {
                for rule in self.new_policy.access_vector_rules().find_xperm_rules(
                    source_id,
                    target_id,
                    target_class_id,
                ) {
                    let xperms = rule.extended_permissions();
                    if rule.kind() == RuleKind::AllowXperm
                        && xperms_types.contains(&xperms.xperms_type())
                    {
                        explicit_allow.get_or_insert(XpermsBitmap::NONE);
                    }

                    if let Some(xperms_bitmap) = bitmap_if_prefix_matches(xperms_prefix, xperms) {
                        match rule.kind() {
                            RuleKind::AllowXperm => {
                                (*explicit_allow.get_or_insert(XpermsBitmap::NONE)) |=
                                    xperms_bitmap;
                            }
                            RuleKind::AuditAllowXperm => auditallow |= xperms_bitmap,
                            RuleKind::DontAuditXperm => auditdeny -= xperms_bitmap,
                            _ => {}
                        }
                    }
                }
            }
        }
        let allow = explicit_allow.unwrap_or(XpermsBitmap::ALL);
        XpermsAccessDecision { allow, auditallow, auditdeny }
    }

    pub(super) fn genfscon_find_all<'a>(
        &'a self,
        fs_type: &'a [u8],
    ) -> impl Iterator<Item = &'a GenfsConPath> {
        self.generic_fs_contexts()
            .iter()
            .filter(move |entry| entry.fs_type() == fs_type)
            .flat_map(|entry| entry.paths().iter())
    }

    pub(super) fn compute_filename_transition(
        &self,
        source_type: TypeId,
        target_type: TypeId,
        class: ClassId,
        name: &[u8],
    ) -> Option<TypeId> {
        self.new_policy.filename_transitions().compute_filename_transition(
            source_type,
            target_type,
            class,
            name,
        )
    }

    pub(super) fn initial_context(&self, mut id: crate::InitialSid) -> &crate::new_policy::Context {
        let need_init_sid = self.has_policycap(PolicyCap::UserspaceInitialContext);
        if id == crate::InitialSid::Init && !need_init_sid {
            id = crate::InitialSid::Kernel;
        }
        self.new_policy
            .initial_sids()
            .get_by_id(id as u32)
            .expect("initial SID must be present in validated policy")
    }
}

impl ParsedPolicy {
    /// Parses the binary policy stored in `bytes`. It is an error for `bytes` to have trailing
    /// bytes after policy parsing completes.
    pub(super) fn parse(data: PolicyData) -> Result<Self, anyhow::Error> {
        let policy_size = data.len();
        if MAXIMUM_POLICY_SIZE <= policy_size {
            return Err(anyhow::Error::from(ParseError::UnsupportedlyLarge {
                observed: policy_size,
                limit: MAXIMUM_POLICY_SIZE,
            }));
        }
        let new_policy =
            NewPolicy::parse(&data).map_err(|e| anyhow::anyhow!("new parser failed: {:?}", e))?;
        new_policy.validate().context("validating new policy structure")?;

        Ok(ParsedPolicy { new_policy: Arc::new(new_policy) })
    }
}

impl ParsedPolicy {
    pub fn validate(&self) -> Result<(), anyhow::Error> {
        // Validate that all kernel-required initial SIDs are present in the policy.
        let need_init_sid = self.has_policycap(PolicyCap::UserspaceInitialContext);
        for initial_sid in crate::InitialSid::all_variants() {
            if *initial_sid == crate::InitialSid::Init && !need_init_sid {
                continue;
            }
            self.new_policy
                .initial_sids()
                .get_by_id(*initial_sid as u32)
                .ok_or(ValidateError::MissingInitialSid { initial_sid: *initial_sid })?;
        }

        // To-do comments for cross-policy validations yet to be implemented go here.
        // TODO(b/356569876): Determine which "bounds" should be verified for correctness here.

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::TypeSet;
    use crate::new_policy::traits::PolicyId;
    use crate::policy::Parse;
    use crate::policy::parser::PolicyCursor;
    use std::sync::Arc;

    #[test]
    fn test_id_set_parse_compatibility() {
        let bytes = [
            64, 0, 0, 0, // map_item_size_bits = 64
            128, 0, 0, 0, // high_bit = 128
            2, 0, 0, 0, // count = 2
            // Item 1
            0, 0, 0, 0, // start_bit = 0
            5, 0, 0, 0, 0, 0, 0, 0, // map = 5 (bits 0 and 2 set)
            // Item 2
            64, 0, 0, 0, // start_bit = 64
            2, 0, 0, 0, 0, 0, 0, 0, // map = 2 (bit 65 set)
        ];
        let data: PolicyData = Arc::from(bytes);
        let cursor = PolicyCursor::new(&data);
        let (id_set, tail) = TypeSet::parse(cursor).unwrap();
        assert_eq!(tail.offset(), bytes.len() as u32);
        assert!(id_set.contains(TypeId::from_u32(1).unwrap()));
        assert!(!id_set.contains(TypeId::from_u32(2).unwrap()));
        assert!(id_set.contains(TypeId::from_u32(3).unwrap()));
        assert!(id_set.contains(TypeId::from_u32(66).unwrap()));
    }
}
