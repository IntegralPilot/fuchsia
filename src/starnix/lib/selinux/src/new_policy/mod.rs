// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub(super) mod access_vector;
pub(super) mod bitmap;
pub(super) mod booleans;
pub(super) mod classes;
pub(super) mod common_symbols;
pub(super) mod constraints;
pub(super) mod context;
pub(super) mod error;
pub(super) mod filename_transitions;
pub(super) mod id_type;
pub(super) mod indexed;
pub(super) mod initial_sids;
pub(super) mod metadata;
pub(super) mod mls;
pub(super) mod object_contexts;
pub(super) mod parser;
pub(super) mod permissions;
pub(super) mod policy_cap;
pub(super) mod roles;
pub(super) mod rules;
pub(super) mod traits;
pub(super) mod types;
pub(super) mod u24_index;
pub(super) mod users;

use selinux_policy_derive::{Parse, Serialize, Validate};

pub use access_vector::AccessVector;
pub use bitmap::IdSpan;
pub use booleans::{ConditionalBoolean, ConditionalBooleanId};
pub use classes::{Class, ClassDefault, ClassDefaultRange, ClassId};
pub use common_symbols::CommonSymbol;
pub use constraints::{
    ConstraintOperator, ConstraintSubject, ConstraintTerm, MlsOperands, MlsOperator, NameExpression,
};
pub use context::{Context, MlsLevel, MlsRange};
use error::{ParseError, SerializeError, ValidateError};
pub use filename_transitions::FilenameTransitions;
pub use id_type::*;
pub use indexed::IdAndNameIndexed;
pub use initial_sids::InitialSids;
use metadata::{Config, Counts, Magic, Signature};
pub use metadata::{HandleUnknown, POLICYDB_VERSION_MAX, PolicyVersion};
pub use mls::{Category, RangeTransition, Sensitivity};
pub use object_contexts::{FsUseType, GenfsCon, GenfsConPath, ObjectContexts};
use parser::{Array, PolicyCursor};
pub use parser::{PolicyWriter, SymbolArray};
pub use permissions::PermissionId;
pub use policy_cap::{PolicyCap, PolicyCapSet};
pub use roles::{Role, RoleAllow, RoleId, RoleSet, RoleTransition};
pub use rules::{
    AccessDecision, AccessVectorRules, ConditionalNode, IndexedAccessVectorRules,
    SELINUX_AVD_FLAGS_PERMISSIVE, XpermsBitmap,
};
use traits::{Serialize, Validate};
pub use types::*;
pub use u24_index::U24Index;
pub use users::User;

/// Tag type for type safety of policy user identifiers.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct UserTag;

/// Identifies a user within a policy.
pub type UserId = IdType<std::num::NonZeroU16, UserTag>;

/// Tag type for type safety of policy sensitivity identifiers.
#[derive(Copy, Clone, Debug, Hash, Eq, Ord, PartialEq, PartialOrd)]
pub struct SensitivityTag;

/// Identifies a sensitivity level within a policy.
pub type SensitivityId = IdType<std::num::NonZeroU16, SensitivityTag>;

/// Tag type for type safety of policy category identifiers.
#[derive(Copy, Clone, Debug, Hash, Eq, Ord, PartialEq, PartialOrd)]
pub struct CategoryTag;

/// Identifies a security category within a policy.
pub type CategoryId = IdType<std::num::NonZeroU16, CategoryTag>;

/// Set of security categories.
pub type CategorySet = bitmap::IdSet<CategoryId>;

/// Builder for constructing [`CategorySet`]s dynamically.
pub type CategorySetBuilder = bitmap::IdSetBuilder<CategoryId>;

/// Top-level [`NewPolicy`] structure representing a parsed SELinux binary policy.
#[derive(Debug, Parse, Serialize, Validate)]
pub struct NewPolicy {
    magic: Magic,
    signature: Signature,
    version: PolicyVersion,
    config: Config,
    counts: Counts,
    policy_capabilities: PolicyCapSet,
    permissive_map: PermissiveTypeSet,
    common_symbols: IdAndNameIndexed<SymbolArray<CommonSymbol>>,
    classes: IdAndNameIndexed<SymbolArray<Class>>,
    roles: IdAndNameIndexed<SymbolArray<Role>>,
    types: Types,
    users: IdAndNameIndexed<SymbolArray<User>>,
    conditional_booleans: IdAndNameIndexed<SymbolArray<ConditionalBoolean>>,
    sensitivities: IdAndNameIndexed<SymbolArray<Sensitivity>>,
    categories: IdAndNameIndexed<SymbolArray<Category>>,
    access_vector_rules: IndexedAccessVectorRules,
    conditional_nodes: Array<ConditionalNode>,
    role_transitions: Array<RoleTransition>,
    role_allowlist: Array<RoleAllow>,
    filename_transitions: FilenameTransitions,
    initial_sids: InitialSids,
    object_contexts: ObjectContexts,
    generic_fs_contexts: Array<GenfsCon>,
    range_transitions: Array<RangeTransition>,
    type_attribute_maps: TypeAttributeMaps,
}

impl NewPolicy {
    /// Parses a [`NewPolicy`] from the raw binary data.
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let mut cursor = PolicyCursor::new(data);
        let policy = cursor.parse()?;
        if cursor.offset() < data.len() {
            return Err(ParseError::TrailingBytes { num_bytes: data.len() - cursor.offset() });
        }
        Ok(policy)
    }

    /// Validates the parsed policy.
    pub fn validate(&self) -> Result<(), ValidateError> {
        Validate::validate(self, self)
    }

    /// Serializes the policy to binary representation.
    pub fn serialize(&self, writer: &mut Vec<u8>) -> Result<(), SerializeError> {
        let mut policy_writer = PolicyWriter::new(self.version, writer);
        Serialize::serialize(self, &mut policy_writer)
    }

    /// Returns the policy version.
    pub fn version(&self) -> PolicyVersion {
        self.version
    }

    /// Returns the [`HandleUnknown`] configuration.
    pub fn handle_unknown(&self) -> HandleUnknown {
        self.config.handle_unknown()
    }

    /// Returns the policy capabilities set.
    pub fn policy_capabilities(&self) -> &PolicyCapSet {
        &self.policy_capabilities
    }

    /// Returns the permissive types set.
    pub fn permissive_map(&self) -> &PermissiveTypeSet {
        &self.permissive_map
    }

    /// Returns the common symbols table.
    pub fn common_symbols(&self) -> &IdAndNameIndexed<SymbolArray<CommonSymbol>> {
        &self.common_symbols
    }

    /// Returns the object classes table.
    pub fn classes(&self) -> &IdAndNameIndexed<SymbolArray<Class>> {
        &self.classes
    }

    /// Returns the roles table.
    pub fn roles(&self) -> &IdAndNameIndexed<SymbolArray<Role>> {
        &self.roles
    }

    /// Returns the types table.
    pub fn types(&self) -> &Types {
        &self.types
    }

    /// Returns the users table.
    pub fn users(&self) -> &IdAndNameIndexed<SymbolArray<User>> {
        &self.users
    }

    /// Returns the conditional booleans table.
    pub fn conditional_booleans(&self) -> &IdAndNameIndexed<SymbolArray<ConditionalBoolean>> {
        &self.conditional_booleans
    }

    /// Returns the sensitivities table.
    pub fn sensitivities(&self) -> &IdAndNameIndexed<SymbolArray<Sensitivity>> {
        &self.sensitivities
    }

    /// Returns the categories table.
    pub fn categories(&self) -> &IdAndNameIndexed<SymbolArray<Category>> {
        &self.categories
    }

    /// Returns the access vector rules table.
    pub fn access_vector_rules(&self) -> &IndexedAccessVectorRules {
        &self.access_vector_rules
    }

    /// Returns the list of conditional nodes.
    #[cfg(test)]
    pub(crate) fn conditional_nodes(&self) -> &[ConditionalNode] {
        self.conditional_nodes.as_ref()
    }

    /// Returns the role transitions rules array.
    pub(crate) fn role_transitions(&self) -> &[RoleTransition] {
        self.role_transitions.as_ref()
    }

    /// Returns the role allow rules array.
    pub(crate) fn role_allowlist(&self) -> &[RoleAllow] {
        self.role_allowlist.as_ref()
    }

    /// Returns the filename transitions table.
    pub fn filename_transitions(&self) -> &FilenameTransitions {
        &self.filename_transitions
    }

    /// Returns the initial SIDs table.
    pub fn initial_sids(&self) -> &InitialSids {
        &self.initial_sids
    }

    /// Returns the object contexts table.
    pub fn object_contexts(&self) -> &ObjectContexts {
        &self.object_contexts
    }

    /// Returns generic filesystem labeling statements (`genfscon`).
    pub(crate) fn generic_fs_contexts(&self) -> &[GenfsCon] {
        &self.generic_fs_contexts
    }

    /// Returns the MLS range transitions array.
    pub(crate) fn range_transitions(&self) -> &[RangeTransition] {
        &self.range_transitions
    }

    /// Returns the type attribute maps table.
    pub fn type_attribute_maps(&self) -> &TypeAttributeMaps {
        &self.type_attribute_maps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::traits::{Parse, Serialize};

    #[derive(Copy, Clone, Debug, Eq, PartialEq, Parse, Serialize, Validate)]
    #[policy(wire_type = u32)]
    enum TestEnum {
        ValueOne = 1,
        ValueTwo = 2,
    }

    #[test]
    fn test_enum_derive() {
        let mut cursor = PolicyCursor::new(&[1, 0, 0, 0]);
        let parsed = TestEnum::parse(&mut cursor).unwrap();
        assert_eq!(parsed, TestEnum::ValueOne);

        let mut cursor = PolicyCursor::new(&[2, 0, 0, 0]);
        let parsed = TestEnum::parse(&mut cursor).unwrap();
        assert_eq!(parsed, TestEnum::ValueTwo);

        let mut cursor = PolicyCursor::new(&[3, 0, 0, 0]);
        let err = TestEnum::parse(&mut cursor).unwrap_err();
        assert!(matches!(err, ParseError::InvalidEnumValue { enum_name: "TestEnum", value: 3 }));

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        TestEnum::ValueOne.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, vec![1, 0, 0, 0]);

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        TestEnum::ValueTwo.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, vec![2, 0, 0, 0]);

        let policy_bytes = include_bytes!("../../testdata/policies/selinux_testsuite");
        let policy = NewPolicy::parse(policy_bytes).unwrap();
        TestEnum::ValueOne.validate(&policy).unwrap();
    }

    const TEST_POLICIES: &[(&str, &[u8])] = &[
        ("selinux_testsuite", include_bytes!("../../testdata/policies/selinux_testsuite")),
        ("emulator", include_bytes!("../../testdata/policies/emulator")),
        (
            "conditional_policy",
            include_bytes!("../../testdata/composite_policies/compiled/conditional_policy"),
        ),
        (
            "minimal_policy",
            include_bytes!("../../testdata/composite_policies/compiled/minimal_policy"),
        ),
        (
            "allow_fork_policy",
            include_bytes!("../../testdata/composite_policies/compiled/allow_fork_policy"),
        ),
        (
            "class_defaults_policy",
            include_bytes!("../../testdata/composite_policies/compiled/class_defaults_policy"),
        ),
        (
            "genfscon_policy",
            include_bytes!("../../testdata/composite_policies/compiled/genfscon_policy"),
        ),
        (
            "handle_unknown_policy-allow",
            include_bytes!(
                "../../testdata/composite_policies/compiled/handle_unknown_policy-allow"
            ),
        ),
        (
            "handle_unknown_policy-deny",
            include_bytes!("../../testdata/composite_policies/compiled/handle_unknown_policy-deny"),
        ),
        (
            "handle_unknown_policy-reject",
            include_bytes!(
                "../../testdata/composite_policies/compiled/handle_unknown_policy-reject"
            ),
        ),
        (
            "range_transition_policy",
            include_bytes!("../../testdata/composite_policies/compiled/range_transition_policy"),
        ),
        (
            "role_transition_policy",
            include_bytes!("../../testdata/composite_policies/compiled/role_transition_policy"),
        ),
        (
            "type_transition_policy",
            include_bytes!("../../testdata/composite_policies/compiled/type_transition_policy"),
        ),
    ];

    #[test]
    fn test_all_compiled_policies_roundtrip() {
        for (name, policy_bytes) in TEST_POLICIES {
            let new_policy = NewPolicy::parse(policy_bytes)
                .unwrap_or_else(|e| panic!("Failed to parse {name}: {e:?}"));
            new_policy.validate().unwrap_or_else(|e| panic!("Failed to validate {name}: {e:?}"));

            if new_policy.version() >= PolicyVersion::V33 {
                let mut serialized = Vec::new();
                new_policy
                    .serialize(&mut serialized)
                    .unwrap_or_else(|e| panic!("Failed to serialize {name}: {e:?}"));
                assert_bytes_eq(&serialized, policy_bytes);
            }
        }
    }

    #[test]
    fn test_initial_sids_policy_elements() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/minimal_policy");
        let new_policy = NewPolicy::parse(policy_bytes).expect("parse minimal policy");
        new_policy.validate().expect("validate minimal policy");

        assert!(!new_policy.initial_sids().is_empty());
        assert!(new_policy.initial_sids().len() >= crate::InitialSid::all_variants().len() - 1);

        let kernel_context =
            new_policy.initial_sids().get_by_id(crate::InitialSid::Kernel as u32).unwrap();
        assert!(new_policy.users().get_by_id(kernel_context.user()).is_some());
        assert!(new_policy.roles().get_by_id(kernel_context.role()).is_some());

        let unlabeled_context =
            new_policy.initial_sids().get_by_id(crate::InitialSid::Unlabeled as u32).unwrap();
        assert!(new_policy.users().get_by_id(unlabeled_context.user()).is_some());
        assert!(new_policy.roles().get_by_id(unlabeled_context.role()).is_some());
    }
}

#[cfg(test)]
pub(crate) fn assert_bytes_eq(left: &[u8], right: &[u8]) {
    if left != right {
        let min_len = std::cmp::min(left.len(), right.len());
        for i in 0..min_len {
            if left[i] != right[i] {
                let start = i.saturating_sub(8);
                let end = std::cmp::min(i + 16, min_len);
                panic!(
                    "Byte mismatch at offset {i} (0x{i:x}): actual=0x{:02x} vs expected=0x{:02x}.\nActual   [{start}..{end}]: {:02x?}\nExpected [{start}..{end}]: {:02x?}",
                    left[i],
                    right[i],
                    &left[start..end],
                    &right[start..end]
                );
            }
        }
        panic!("Length mismatch: actual={}, expected={}", left.len(), right.len());
    }
}
