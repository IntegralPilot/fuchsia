// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod error;
pub mod index;
pub mod parsed_policy;
pub mod parser;

mod constraints;
mod security_context;

pub use crate::new_policy::traits::{HasName, HasPolicyId, PolicyId};
pub use crate::new_policy::{
    AccessDecision, AccessVector, AccessVectorRules, CategoryId, ClassId, FsUseType, HandleUnknown,
    IndexedAccessVectorRules, MlsLevel, MlsRange, POLICYDB_VERSION_MAX, PermissionId, RoleId,
    SELINUX_AVD_FLAGS_PERMISSIVE, SensitivityId, TypeId, User, UserId, XpermsBitmap,
};
use crate::{ClassPermission, KernelClass, NullessByteStr, ObjectClass, new_policy as new};
pub use index::FsUseLabelAndType;
use index::PolicyIndex;
use parsed_policy::ParsedPolicy;
pub use parser::PolicyCursor;
use parser::PolicyData;
pub use security_context::{SecurityContext, SecurityContextError};

use anyhow::Context as _;
use std::fmt::Debug;
use std::num::NonZeroU32;
use std::ops::Deref;

use std::sync::Arc;
use zerocopy::little_endian as le;

/// Encapsulates the result of a permissions calculation, between
/// source & target domains, for a specific class. Decisions describe

/// A kind of extended permission, corresponding to the base permission that should trigger a check
/// of an extended permission.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum XpermsKind {
    Ioctl,
    Nlmsg,
}

/// Encapsulates the result of an extended permissions calculation, between source & target
/// domains, for a specific class, a specific kind of extended permissions, and for a specific
/// xperm prefix byte. Decisions describe which 16-bit xperms are allowed, and whether xperms
/// should be audit-logged when allowed, and when denied.
#[derive(Debug, Clone, PartialEq)]
pub struct XpermsAccessDecision {
    pub allow: XpermsBitmap,
    pub auditallow: XpermsBitmap,
    pub auditdeny: XpermsBitmap,
}

impl XpermsAccessDecision {
    pub const DENY_ALL: Self = Self {
        allow: XpermsBitmap::NONE,
        auditallow: XpermsBitmap::NONE,
        auditdeny: XpermsBitmap::ALL,
    };
    pub const ALLOW_ALL: Self = Self {
        allow: XpermsBitmap::ALL,
        auditallow: XpermsBitmap::NONE,
        auditdeny: XpermsBitmap::ALL,
    };
}

/// Parses `binary_policy` by value; that is, copies underlying binary data out in addition to
/// building up parser output structures. This function returns
/// `(unvalidated_parser_output, binary_policy)` on success, or an error if parsing failed. Note
/// that the second component of the success case contains precisely the same bytes as the input.
/// This function depends on a uniformity of interface between the "by value" and "by reference"
/// strategies, but also requires an `unvalidated_parser_output` type that is independent of the
/// `binary_policy` lifetime. Taken together, these requirements demand the "move-in + move-out"
/// interface for `binary_policy`.
pub fn parse_policy_by_value(binary_policy: Vec<u8>) -> Result<Unvalidated, anyhow::Error> {
    let policy_data: PolicyData = Arc::from(binary_policy);
    let policy = ParsedPolicy::parse(policy_data).context("parsing policy")?;
    Ok(Unvalidated(policy))
}

#[derive(Debug)]
pub struct Policy(PolicyIndex);

impl Deref for Policy {
    type Target = PolicyIndex;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Policy {
    /// Serializes the policy back into [`PolicyData`].
    pub fn serialize(&self) -> PolicyData {
        let mut bytes = Vec::new();
        self.0.serialize(&mut bytes).expect("serialization of new_policy should succeed");
        std::sync::Arc::from(bytes)
    }

    pub fn conditional_booleans<'a>(&'a self) -> Vec<(&'a [u8], bool)> {
        self.0
            .conditional_booleans()
            .iter()
            .map(|boolean| (boolean.name(), boolean.active()))
            .collect()
    }

    /// Returns the set of permissions for the given class, including both the
    /// explicitly owned permissions and the inherited ones from common symbols.
    /// Each permission is a tuple of the permission identifier (in the scope of
    /// the given class) and the permission name.
    pub fn find_class_permissions_by_name(
        &self,
        class_name: &str,
    ) -> Result<Vec<(PermissionId, Vec<u8>)>, ()> {
        let classes = self.classes();
        let class = classes.get_by_name(class_name.as_bytes()).ok_or(())?;
        let owned_permissions = class.permissions();

        let mut result: Vec<_> = owned_permissions
            .iter()
            .map(|permission| (permission.id(), permission.name().to_vec()))
            .collect();

        // common_name() is empty when the class doesn't inherit from a CommonSymbol.
        if class.common_name().is_empty() {
            return Ok(result);
        }

        let common_symbol_permissions =
            self.common_symbols().get_by_name(class.common_name()).ok_or(())?.permissions();

        result.append(
            &mut common_symbol_permissions
                .iter()
                .map(|permission| (permission.id(), permission.name().to_vec()))
                .collect(),
        );

        Ok(result)
    }

    /// If there is an fs_use statement for the given filesystem type, returns the associated
    /// [`SecurityContext`] and [`FsUseType`].
    pub fn fs_use_label_and_type(&self, fs_type: NullessByteStr<'_>) -> Option<FsUseLabelAndType> {
        self.0.fs_use_label_and_type(fs_type)
    }

    /// If there is a genfscon statement for the given filesystem type, returns the associated
    /// [`SecurityContext`].
    pub fn genfscon_label_for_fs_and_path(
        &self,
        fs_type: NullessByteStr<'_>,
        node_path: NullessByteStr<'_>,
        class_id: Option<KernelClass>,
    ) -> Option<SecurityContext> {
        self.0.genfscon_label_for_fs_and_path(fs_type, node_path, class_id)
    }

    /// Returns the [`SecurityContext`] defined by this policy for the specified
    /// well-known (or "initial") Id.
    pub fn initial_context(&self, id: crate::InitialSid) -> security_context::SecurityContext {
        self.0.initial_context(id)
    }

    /// Returns a [`SecurityContext`] with fields parsed from the supplied Security Context string.
    pub fn parse_security_context(
        &self,
        security_context: NullessByteStr<'_>,
    ) -> Result<security_context::SecurityContext, security_context::SecurityContextError> {
        security_context::SecurityContext::from_string(&self.0, security_context)
    }

    /// Validates a [`SecurityContext`] against this policy's constraints.
    pub fn validate_security_context(
        &self,
        security_context: &SecurityContext,
    ) -> Result<(), SecurityContextError> {
        security_context.validate(&self.0)
    }

    /// Returns a byte string describing the supplied [`SecurityContext`].
    pub fn serialize_security_context(&self, security_context: &SecurityContext) -> Vec<u8> {
        security_context.to_string(&self.0)
    }

    /// Returns the security context that should be applied to a newly created SELinux
    /// object according to `source` and `target` security contexts, as well as the new object's
    /// `class` and `name`.
    ///
    /// Computation follows the "create" algorithm for labeling newly created objects:
    /// - user is taken from the `source` by default, or `target` if specified by policy.
    /// - role, type and range are taken from the matching transition rules, if any.
    /// - role, type and range fall-back to the `source` or `target` values according to policy.
    ///
    /// Callers pass an empty slice (`&[]`) for `name` to express nameless transitions.
    /// When a non-empty `name` is provided, filename transition rules are checked first.
    /// If no transitions apply, and the policy does not explicitly specify defaults then the
    /// role, type and range values have defaults chosen based on the `class`:
    /// - For "process", and socket-like classes, role, type and range are taken from the `source`.
    /// - Otherwise role is "object_r", type is taken from `target` and range is set to the
    ///   low level of the `source` range.
    ///
    /// Returns an error if the Security Context for such an object is not valid under this
    /// [`Policy`] (e.g. if the type is not permitted for the chosen role, etc).
    pub fn compute_create_context(
        &self,
        source: &SecurityContext,
        target: &SecurityContext,
        class: impl Into<ObjectClass>,
        name: &[u8],
    ) -> SecurityContext {
        self.0.compute_create_context(source, target, class.into(), name)
    }

    /// Computes the access vector that associates type `source_type_name` and
    /// `target_type_name` via an explicit `allow [...];` statement in the
    /// binary policy, subject to any matching constraint statements. Computes
    /// `AccessVector::NONE` if no such statement exists.
    ///
    /// Access decisions are currently based on explicit "allow" rules and
    /// "constrain" or "mlsconstrain" statements. A permission is allowed if
    /// it is allowed by an explicit "allow", and if in addition, all matching
    /// constraints are satisfied.
    pub fn compute_access_decision(
        &self,
        source_context: &SecurityContext,
        target_context: &SecurityContext,
        object_class: impl Into<ObjectClass>,
    ) -> AccessDecision {
        if let Some(target_class) = self.0.class(object_class.into()) {
            self.0.compute_access_decision(source_context, target_context, &target_class)
        } else {
            let mut decision = AccessDecision::allow(AccessVector::NONE);
            if self.is_permissive(source_context.type_()) {
                decision.flags |= SELINUX_AVD_FLAGS_PERMISSIVE;
            }
            decision
        }
    }

    /// Computes the extended permissions that should be allowed, audited when allowed, and audited
    /// when denied, for a given kind of extended permissions (`ioctl` or `nlmsg`), source context,
    /// target context, target class, and xperms prefix byte.
    pub fn compute_xperms_access_decision(
        &self,
        xperms_kind: XpermsKind,
        source_context: &SecurityContext,
        target_context: &SecurityContext,
        object_class: impl Into<ObjectClass>,
        xperms_prefix: u8,
    ) -> XpermsAccessDecision {
        if let Some(target_class) = self.0.class(object_class.into()) {
            self.0.compute_xperms_access_decision(
                xperms_kind,
                source_context,
                target_context,
                &target_class,
                xperms_prefix,
            )
        } else {
            XpermsAccessDecision::DENY_ALL
        }
    }

    pub fn is_bounded_by(&self, bounded_type: TypeId, parent_type: TypeId) -> bool {
        self.0.types().get_by_id(bounded_type).unwrap().bounded_by() == Some(parent_type)
    }

    /// Returns true if the policy has the marked the type/domain for permissive checks.
    pub fn is_permissive(&self, type_: TypeId) -> bool {
        self.0.permissive_map().contains(type_)
    }
}

impl AccessVectorComputer for Policy {
    fn access_decision_to_kernel_access_decision(
        &self,
        class: KernelClass,
        av: AccessDecision,
    ) -> KernelAccessDecision {
        let mut kernel_allow;
        let mut kernel_audit;
        // Set the default values of the bits as appropriate for the policy's handle_unknown value.
        // Bits corresponding to policy-known permissions will be overwritten.
        if self.0.handle_unknown() == HandleUnknown::Allow {
            // If we allow unknown permissions, a bit will be by default allowed and not audited.
            kernel_allow = 0xffffffffu32;
            kernel_audit = 0u32;
        } else {
            // Otherwise, a bit is by default audited and not allowed.
            kernel_allow = 0u32;
            kernel_audit = 0xffffffffu32;
        }

        let decision_allow = av.allow;
        let decision_audit = (av.allow & av.auditallow) | (!av.allow & av.auditdeny);
        for permission in class.permissions() {
            if let Some(permission_access_vector) =
                self.0.kernel_permission_to_access_vector(permission.clone())
            {
                // If the permission is known, set the corresponding bit according to
                // `decision_allow` and `decision_audit`.
                let bit = 1 << permission.id();
                let allow = decision_allow & permission_access_vector == permission_access_vector;
                let audit = decision_audit & permission_access_vector == permission_access_vector;
                kernel_allow = (kernel_allow & !bit) | ((allow as u32) << permission.id());
                kernel_audit = (kernel_audit & !bit) | ((audit as u32) << permission.id());
            }
        }
        KernelAccessDecision {
            allow: AccessVector::from(kernel_allow),
            audit: AccessVector::from(kernel_audit),
            flags: av.flags,
            todo_bug: av.todo_bug,
        }
    }
}

/// A [`Policy`] that has been successfully parsed, but not validated.
pub struct Unvalidated(ParsedPolicy);

impl Unvalidated {
    pub fn validate(self) -> Result<Policy, anyhow::Error> {
        self.0.validate().context("validating parsed policy")?;
        let index = PolicyIndex::new(self.0).context("building index")?;
        Ok(Policy(index))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelAccessDecision {
    pub allow: AccessVector,
    pub audit: AccessVector,
    pub flags: u32,
    pub todo_bug: Option<NonZeroU32>,
}

/// An owner of policy information that can translate [`crate::Permission`] values into
/// [`AccessVector`] values that are consistent with the owned policy.
pub trait AccessVectorComputer {
    /// Translates the given [`AccessDecision`] to a [`KernelAccessDecision`].
    ///
    /// The loaded policy's "handle unknown" configuration determines how `permissions`
    /// entries not explicitly defined by the policy are handled. Allow-unknown will
    /// result in unknown `permissions` being allowed, while they are denied (and audited)
    /// if the policy uses deny-unknown.
    fn access_decision_to_kernel_access_decision(
        &self,
        class: KernelClass,
        av: AccessDecision,
    ) -> KernelAccessDecision;
}

/// A data structure that can be parsed as a part of a binary policy.
pub trait Parse: Sized {
    /// The type of error that may be returned from `parse()`, usually [`ParseError`] or
    /// [`anyhow::Error`].
    type Error: Into<anyhow::Error>;

    /// Parses a `Self` from `bytes`, returning the `Self` and trailing bytes, or an error if
    /// bytes corresponding to a `Self` are malformed.
    fn parse<'a>(bytes: PolicyCursor<'a>) -> Result<(Self, PolicyCursor<'a>), Self::Error>;
}

/// Treat a type as metadata that contains a count of subsequent data.
impl Parse for le::U32 {
    type Error = anyhow::Error;

    fn parse<'a>(bytes: PolicyCursor<'a>) -> Result<(Self, PolicyCursor<'a>), Self::Error> {
        bytes.parse::<le::U32>().map_err(anyhow::Error::from)
    }
}

impl<T: crate::new_policy::traits::Parse> Parse for T {
    type Error = anyhow::Error;

    fn parse<'a>(bytes: PolicyCursor<'a>) -> Result<(Self, PolicyCursor<'a>), Self::Error> {
        let offset = bytes.offset() as usize;
        let slice = &bytes.data().as_ref()[offset..];
        let mut new_cursor = crate::new_policy::parser::PolicyCursor::new(slice);
        let item = <T as crate::new_policy::traits::Parse>::parse(&mut new_cursor)
            .map_err(|e| anyhow::anyhow!("Parse error: {:?}", e))?;
        let new_offset = bytes.offset() + new_cursor.offset() as u32;
        Ok((item, PolicyCursor::new_at(bytes.data(), new_offset)))
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::security_context::SecurityContext;
    use super::{
        AccessVector, ClassId, HandleUnknown, Policy, TypeId, XpermsAccessDecision, XpermsBitmap,
        XpermsKind, parse_policy_by_value,
    };
    use crate::new_policy::traits::HasPolicyId;
    use crate::{FileClass, InitialSid, KernelClass};
    use anyhow::Context as _;
    use serde::Deserialize;

    /// Returns whether the input types are explicitly granted `permission` via an `allow [...];`
    /// policy statement.
    ///
    /// # Panics
    /// If supplied with type Ids not previously obtained from the `Policy` itself; validation
    /// ensures that all such Ids have corresponding definitions.
    /// If either of `target_class` or `permission` cannot be resolved in the policy.
    fn is_explicitly_allowed(
        policy: &Policy,
        source_type: TypeId,
        target_type: TypeId,
        target_class: &str,
        permission: &str,
    ) -> bool {
        let classes = policy.classes();
        let class = classes.get_by_name(target_class.as_bytes()).expect("class not found");
        let class_permissions = policy
            .find_class_permissions_by_name(target_class)
            .expect("class permissions not found");
        let (permission_id, _) = class_permissions
            .iter()
            .find(|(_, name)| permission.as_bytes() == name)
            .expect("permission not found");
        let permission_bit = AccessVector::from(*permission_id);
        let access_decision = policy.0.compute_explicitly_allowed(source_type, target_type, class);
        permission_bit == access_decision.allow & permission_bit
    }

    #[derive(Debug, Deserialize)]
    struct Expectations {
        expected_policy_version: u32,
        expected_handle_unknown: LocalHandleUnknown,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "snake_case")]
    enum LocalHandleUnknown {
        Deny,
        Reject,
        Allow,
    }

    impl PartialEq<HandleUnknown> for LocalHandleUnknown {
        fn eq(&self, other: &HandleUnknown) -> bool {
            match self {
                LocalHandleUnknown::Deny => *other == HandleUnknown::Deny,
                LocalHandleUnknown::Reject => *other == HandleUnknown::Reject,
                LocalHandleUnknown::Allow => *other == HandleUnknown::Allow,
            }
        }
    }

    /// Given a vector of integer (u8) values, returns a bitmap in which the set bits correspond to
    /// the indices of the provided values.
    fn xperms_bitmap_from_elements(elements: &[u8]) -> XpermsBitmap {
        let mut bitmap = [0u64; 4];
        for element in elements {
            let block_index = (*element as usize) / 64;
            let bit_index = (*element as usize) % 64;
            bitmap[block_index] |= 1u64 << bit_index;
        }
        XpermsBitmap::new(bitmap)
    }

    #[test]
    fn known_policies() {
        let policies_and_expectations = [
            [
                b"testdata/policies/emulator".to_vec(),
                include_bytes!("../../testdata/policies/emulator").to_vec(),
                include_bytes!("../../testdata/expectations/emulator").to_vec(),
            ],
            [
                b"testdata/policies/selinux_testsuite".to_vec(),
                include_bytes!("../../testdata/policies/selinux_testsuite").to_vec(),
                include_bytes!("../../testdata/expectations/selinux_testsuite").to_vec(),
            ],
        ];

        for [policy_path, policy_bytes, expectations_bytes] in policies_and_expectations {
            let expectations = serde_json5::from_reader::<_, Expectations>(
                &mut std::io::Cursor::new(expectations_bytes),
            )
            .expect("deserialize expectations");

            // Test parse-by-value.

            let unvalidated_policy =
                parse_policy_by_value(policy_bytes.clone()).expect("parse policy");

            let policy = unvalidated_policy
                .validate()
                .with_context(|| {
                    format!(
                        "policy path: {:?}",
                        std::str::from_utf8(policy_path.as_slice()).unwrap()
                    )
                })
                .expect("validate policy");

            assert_eq!(expectations.expected_policy_version, policy.version().get());
            assert_eq!(expectations.expected_handle_unknown, policy.handle_unknown());

            // Returned policy bytes must be identical to input policy bytes.
            let binary_policy = policy.serialize();
            assert_eq!(policy_bytes, binary_policy.as_ref());
        }
    }

    #[test]
    fn policy_lookup() {
        let policy_bytes = include_bytes!("../../testdata/policies/selinux_testsuite");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate selinux testsuite policy");

        let unconfined_t = policy.types().get_by_name(b"unconfined_t").expect("look up type").id();

        assert!(is_explicitly_allowed(&policy, unconfined_t, unconfined_t, "process", "fork",));
    }

    #[test]
    fn initial_contexts() {
        let policy_bytes =
            include_bytes!("../../testdata/micro_policies/multiple_levels_and_categories_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let kernel_context = policy.initial_context(InitialSid::Kernel);
        assert_eq!(
            policy.serialize_security_context(&kernel_context),
            b"user0:object_r:type0:s0:c0-s1:c0.c2,c4"
        )
    }

    #[test]
    fn explicit_allow_type_type() {
        let policy_bytes =
            include_bytes!("../../testdata/micro_policies/allow_a_t_b_t_class0_perm0_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let a_t = policy.types().get_by_name(b"a_t").expect("look up type").id();
        let b_t = policy.types().get_by_name(b"b_t").expect("look up type").id();

        assert!(is_explicitly_allowed(&policy, a_t, b_t, "class0", "perm0"));
    }

    #[test]
    fn no_explicit_allow_type_type() {
        let policy_bytes =
            include_bytes!("../../testdata/micro_policies/no_allow_a_t_b_t_class0_perm0_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let a_t = policy.types().get_by_name(b"a_t").expect("look up type").id();
        let b_t = policy.types().get_by_name(b"b_t").expect("look up type").id();

        assert!(!is_explicitly_allowed(&policy, a_t, b_t, "class0", "perm0"));
    }

    #[test]
    fn explicit_allow_type_attr() {
        let policy_bytes =
            include_bytes!("../../testdata/micro_policies/allow_a_t_b_attr_class0_perm0_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let a_t = policy.types().get_by_name(b"a_t").expect("look up type").id();
        let b_t = policy.types().get_by_name(b"b_t").expect("look up type").id();

        assert!(is_explicitly_allowed(&policy, a_t, b_t, "class0", "perm0"));
    }

    #[test]
    fn no_explicit_allow_type_attr() {
        let policy_bytes =
            include_bytes!("../../testdata/micro_policies/no_allow_a_t_b_attr_class0_perm0_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let a_t = policy.types().get_by_name(b"a_t").expect("look up type").id();
        let b_t = policy.types().get_by_name(b"b_t").expect("look up type").id();

        assert!(!is_explicitly_allowed(&policy, a_t, b_t, "class0", "perm0"));
    }

    #[test]
    fn explicit_allow_attr_attr() {
        let policy_bytes =
            include_bytes!("../../testdata/micro_policies/allow_a_attr_b_attr_class0_perm0_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let a_t = policy.types().get_by_name(b"a_t").expect("look up type").id();
        let b_t = policy.types().get_by_name(b"b_t").expect("look up type").id();

        assert!(is_explicitly_allowed(&policy, a_t, b_t, "class0", "perm0"));
    }

    #[test]
    fn no_explicit_allow_attr_attr() {
        let policy_bytes = include_bytes!(
            "../../testdata/micro_policies/no_allow_a_attr_b_attr_class0_perm0_policy"
        );
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let a_t = policy.types().get_by_name(b"a_t").expect("look up type").id();
        let b_t = policy.types().get_by_name(b"b_t").expect("look up type").id();

        assert!(!is_explicitly_allowed(&policy, a_t, b_t, "class0", "perm0"));
    }

    #[test]
    fn compute_explicitly_allowed_multiple_attributes() {
        let policy_bytes = include_bytes!(
            "../../testdata/micro_policies/allow_a_t_a1_attr_class0_perm0_a2_attr_class0_perm1_policy"
        );
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let a_t = policy.types().get_by_name(b"a_t").expect("look up type").id();

        let classes = policy.classes();
        let class = classes.get_by_name(b"class0").expect("class not found");
        let raw_access_vector = policy.0.compute_explicitly_allowed(a_t, a_t, class).allow.value();

        // Two separate attributes are each allowed one permission on `[attr] self:class0`. Both
        // attributes are associated with "a_t". No other `allow` statements appear in the policy
        // in relation to "a_t". Therefore, we expect exactly two 1's in the access vector for
        // query `("a_t", "a_t", "class0")`.
        assert_eq!(2, raw_access_vector.count_ones());
    }

    #[test]
    fn compute_access_decision_with_constraints() {
        let policy_bytes =
            include_bytes!("../../testdata/micro_policies/allow_with_constraints_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");

        let target_context_satisfied: SecurityContext = source_context.clone();
        let decision_satisfied = policy.compute_access_decision(
            &source_context,
            &target_context_satisfied,
            KernelClass::File,
        );
        // The class `file` has 4 permissions, 3 of which are explicitly
        // allowed for this target context. All of those permissions satisfy all
        // matching constraints.
        assert_eq!(decision_satisfied.allow, AccessVector::from(7));

        let target_context_unsatisfied: SecurityContext = policy
            .parse_security_context(b"user1:object_r:type0:s0:c0-s0:c0".into())
            .expect("create target security context failing some constraints");
        let decision_unsatisfied = policy.compute_access_decision(
            &source_context,
            &target_context_unsatisfied,
            KernelClass::File,
        );
        // Two of the explicitly-allowed permissions fail to satisfy a matching
        // constraint. Only 1 is allowed in the final access decision.
        assert_eq!(decision_unsatisfied.allow, AccessVector::from(4));
    }

    #[test]
    fn compute_ioctl_access_decision_explicitly_allowed() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `file` class:
        //
        // `allowxperm type0 self:file ioctl { 0xabcd };`
        // `allowxperm type0 self:file ioctl { 0xabef };`
        // `allowxperm type0 self:file ioctl { 0x1000 - 0x10ff };`
        //
        // `auditallowxperm` rules for the `file` class:
        //
        // auditallowxperm type0 self:file ioctl { 0xabcd };
        // auditallowxperm type0 self:file ioctl { 0xabef };
        // auditallowxperm type0 self:file ioctl { 0x1000 - 0x10ff };
        //
        // `dontauditxperm` rules for the `file` class:
        //
        // dontauditxperm type0 self:file ioctl { 0xabcd };
        // dontauditxperm type0 self:file ioctl { 0xabef };
        // dontauditxperm type0 self:file ioctl { 0x1000 - 0x10ff };
        let decision_single = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            KernelClass::File,
            0xab,
        );

        let mut expected_auditdeny =
            xperms_bitmap_from_elements((0x0..=0xff).collect::<Vec<_>>().as_slice());
        expected_auditdeny -= xperms_bitmap_from_elements(&[0xcd, 0xef]);

        let expected_decision_single = XpermsAccessDecision {
            allow: xperms_bitmap_from_elements(&[0xcd, 0xef]),
            auditallow: xperms_bitmap_from_elements(&[0xcd, 0xef]),
            auditdeny: expected_auditdeny,
        };
        assert_eq!(decision_single, expected_decision_single);

        let decision_range = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            KernelClass::File,
            0x10,
        );
        let expected_decision_range = XpermsAccessDecision {
            allow: XpermsBitmap::ALL,
            auditallow: XpermsBitmap::ALL,
            auditdeny: XpermsBitmap::NONE,
        };
        assert_eq!(decision_range, expected_decision_range);
    }

    #[test]
    fn compute_ioctl_access_decision_denied() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id = unvalidated
            .0
            .classes()
            .get_by_name(b"class_one_ioctl")
            .expect("look up class_one_ioctl")
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_one_ioctl` class:
        //
        // `allowxperm type0 self:class_one_ioctl ioctl { 0xabcd };`
        let decision_single = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xdb,
        );

        let expected_decision = XpermsAccessDecision {
            allow: XpermsBitmap::NONE,
            auditallow: XpermsBitmap::NONE,
            auditdeny: XpermsBitmap::ALL,
        };
        assert_eq!(decision_single, expected_decision);
    }

    #[test]
    fn compute_ioctl_access_decision_unmatched() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");

        // No matching ioctl xperm-related statements for this target's type
        let target_context_unmatched: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type1:s0-s0".into())
            .expect("create source security context");

        for prefix in 0x0..=0xff {
            let decision = policy.compute_xperms_access_decision(
                XpermsKind::Ioctl,
                &source_context,
                &target_context_unmatched,
                KernelClass::File,
                prefix,
            );
            assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        }
    }

    #[test]
    fn compute_ioctl_earlier_redundant_prefixful_not_coalesced_into_prefixless() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id = unvalidated
            .0
            .classes()
            .get_by_name(b"class_earlier_redundant_prefixful_not_coalesced_into_prefixless")
            .expect("look up class_earlier_redundant_prefixful_not_coalesced_into_prefixless")
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_earlier_redundant_prefixful_not_coalesced_into_prefixless` class:
        //
        // `allowxperm type0 self:class_earlier_redundant_prefixful_not_coalesced_into_prefixless ioctl { 0x8001-0x8002 };`
        // `allowxperm type0 self:class_earlier_redundant_prefixful_not_coalesced_into_prefixless ioctl { 0x8000-0x80ff };`
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x7f,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x80,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x81,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
    }

    #[test]
    fn compute_ioctl_later_redundant_prefixful_not_coalesced_into_prefixless() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id = unvalidated
            .0
            .classes()
            .get_by_name(b"class_later_redundant_prefixful_not_coalesced_into_prefixless")
            .expect("look up class_later_redundant_prefixful_not_coalesced_into_prefixless")
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_later_redundant_prefixful_not_coalesced_into_prefixless` class:
        //
        // `allowxperm type0 self:class_later_redundant_prefixful_not_coalesced_into_prefixless ioctl { 0x9000-0x90ff };`
        // `allowxperm type0 self:class_later_redundant_prefixful_not_coalesced_into_prefixless ioctl { 0x90fd-0x90fe };`
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x8f,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x90,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x91,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
    }

    #[test]
    fn compute_ioctl_earlier_and_later_redundant_prefixful_not_coalesced_into_prefixless() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id = unvalidated
            .0
            .classes()
            .get_by_name(
                b"class_earlier_and_later_redundant_prefixful_not_coalesced_into_prefixless",
            )
            .expect(
                "look up class_earlier_and_later_redundant_prefixful_not_coalesced_into_prefixless",
            )
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_earlier_and_later_redundant_prefixful_not_coalesced_into_prefixless` class:
        //
        // `allowxperm type0 self:class_earlier_and_later_redundant_prefixful_not_coalesced_into_prefixless ioctl { 0xa001-0xa002 };`
        // `allowxperm type0 self:class_earlier_and_later_redundant_prefixful_not_coalesced_into_prefixless ioctl { 0xa000-0xa03f 0xa040-0xa0ff };`
        // `allowxperm type0 self:class_earlier_and_later_redundant_prefixful_not_coalesced_into_prefixless ioctl { 0xa0fd-0xa0fe };`
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x9f,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xa0,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xa1,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
    }

    #[test]
    fn compute_ioctl_prefixfuls_that_coalesce_to_prefixless() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id: ClassId = unvalidated
            .0
            .classes()
            .get_by_name(b"class_prefixfuls_that_coalesce_to_prefixless")
            .expect("look up class_prefixfuls_that_coalesce_to_prefixless")
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_prefixfuls_that_coalesce_to_prefixless` class:
        //
        // `allowxperm type0 self:class_prefixfuls_that_coalesce_to_prefixless ioctl { 0xb000 0xb001 0xb002 };`
        // `allowxperm type0 self:class_prefixfuls_that_coalesce_to_prefixless ioctl { 0xb003-0xb0fc };`
        // `allowxperm type0 self:class_prefixfuls_that_coalesce_to_prefixless ioctl { 0xb0fd 0xb0fe 0xb0ff };`
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xaf,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xb0,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xb1,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
    }

    #[test]
    fn compute_ioctl_prefixfuls_that_coalesce_to_prefixless_just_before_prefixless() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id = unvalidated
            .0
            .classes()
            .get_by_name(b"class_prefixfuls_that_coalesce_to_prefixless_just_before_prefixless")
            .expect("look up class_prefixfuls_that_coalesce_to_prefixless_just_before_prefixless")
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_prefixfuls_that_coalesce_to_prefixless_just_before_prefixless` class:
        //
        // `allowxperm type0 self:class_prefixfuls_that_coalesce_to_prefixless_just_before_prefixless ioctl { 0xc000 0xc001 0xc002 0xc003 };`
        // `allowxperm type0 self:class_prefixfuls_that_coalesce_to_prefixless_just_before_prefixless ioctl { 0xc004-0xc0fb };`
        // `allowxperm type0 self:class_prefixfuls_that_coalesce_to_prefixless_just_before_prefixless ioctl { 0xc0fc 0xc0fd 0xc0fe 0xc0ff };`
        // `allowxperm type0 self:class_prefixfuls_that_coalesce_to_prefixless_just_before_prefixless ioctl { 0xc100-0xc1ff };`
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xbf,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xc0,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xc1,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xc2,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
    }

    #[test]
    fn compute_ioctl_prefixless_just_before_prefixfuls_that_coalesce_to_prefixless() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id = unvalidated
            .0
            .classes()
            .get_by_name(b"class_prefixless_just_before_prefixfuls_that_coalesce_to_prefixless")
            .expect("look up class_prefixless_just_before_prefixfuls_that_coalesce_to_prefixless")
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_prefixless_just_before_prefixfuls_that_coalesce_to_prefixless` class:
        //
        // `allowxperm type0 self:class_prefixless_just_before_prefixfuls_that_coalesce_to_prefixless ioctl { 0xd600-0xd6ff };`
        // `allowxperm type0 self:class_prefixless_just_before_prefixfuls_that_coalesce_to_prefixless ioctl { 0xd700 0xd701 0xd702 0xd703 };`
        // `allowxperm type0 self:class_prefixless_just_before_prefixfuls_that_coalesce_to_prefixless ioctl { 0xd704-0xd7fb };`
        // `allowxperm type0 self:class_prefixless_just_before_prefixfuls_that_coalesce_to_prefixless ioctl { 0xd7fc 0xd7fd 0xd7fe 0xd7ff };`
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xd5,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xd6,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xd7,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xd8,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
    }

    // As of 2025-12, the policy compiler generates allow rules in an unexpected order in the
    // policy binary for this oddly-expressed policy text content (with one "prefixful" rule
    // of type [`XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES`], then the "prefixless" rule of type
    // `XPERMS_TYPE_IOCTL_PREFIXES`, and then two more rules of type
    // `XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES`). These rules are still contiguous and without
    // interruption by rules of other source-target-class-type quadruplets; it's just unexpected
    // that the "prefixless" one falls in the middle of the "prefixful" ones rather than
    // consistently at the beginning or the end of the "prefixful" ones. We don't directly test
    // that our odd text content leads to this curious binary content, but we do test that we
    // make correct access decisions.
    #[test]
    fn compute_ioctl_ridiculous_permission_ordering() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id = unvalidated
            .0
            .classes()
            .get_by_name(b"class_ridiculous_permission_ordering")
            .expect("look up class_ridiculous_permission_ordering")
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_ridiculous_permission_ordering` class:
        //
        // `allowxperm type0 self:class_ridiculous_permission_ordering ioctl { 0xfdfa-0xfdfd 0xf001 };`
        // `allowxperm type0 self:class_ridiculous_permission_ordering ioctl { 0x0080-0x00ff 0xfdfa-0xfdfd 0x0011-0x0017 0x0001 0x0001 0x0001 0xc000-0xcff2 0x0000 0x0011-0x0017 0x0001 0x0005-0x0015 0x0002-0x007f };`
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x00,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x01,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xbf,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xc0,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xce,
        );
        assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xcf,
        );
        assert_eq!(
            decision,
            XpermsAccessDecision {
                allow: xperms_bitmap_from_elements((0x0..=0xf2).collect::<Vec<_>>().as_slice()),
                auditallow: XpermsBitmap::NONE,
                auditdeny: XpermsBitmap::ALL,
            }
        );
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xd0,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xe9,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xf0,
        );
        assert_eq!(
            decision,
            XpermsAccessDecision {
                allow: xperms_bitmap_from_elements(&[0x01]),
                auditallow: XpermsBitmap::NONE,
                auditdeny: XpermsBitmap::ALL,
            }
        );
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xf1,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xfc,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xfd,
        );
        assert_eq!(
            decision,
            XpermsAccessDecision {
                allow: xperms_bitmap_from_elements((0xfa..=0xfd).collect::<Vec<_>>().as_slice()),
                auditallow: XpermsBitmap::NONE,
                auditdeny: XpermsBitmap::ALL,
            }
        );
        let decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0xfe,
        );
        assert_eq!(decision, XpermsAccessDecision::DENY_ALL);
    }

    #[test]
    fn compute_nlmsg_access_decision_explicitly_allowed() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `netlink_route_socket` class:
        //
        // `allowxperm type0 self:netlink_route_socket nlmsg { 0xabcd };`
        // `allowxperm type0 self:netlink_route_socket nlmsg { 0xabef };`
        // `allowxperm type0 self:netlink_route_socket nlmsg { 0x1000 - 0x10ff };`
        //
        // `auditallowxperm` rules for the `netlink_route_socket` class:
        //
        // auditallowxperm type0 self:netlink_route_socket nlmsg { 0xabcd };
        // auditallowxperm type0 self:netlink_route_socket nlmsg { 0xabef };
        // auditallowxperm type0 self:netlink_route_socket nlmsg { 0x1000 - 0x10ff };
        //
        // `dontauditxperm` rules for the `netlink_route_socket` class:
        //
        // dontauditxperm type0 self:netlink_route_socket nlmsg { 0xabcd };
        // dontauditxperm type0 self:netlink_route_socket nlmsg { 0xabef };
        // dontauditxperm type0 self:netlink_route_socket nlmsg { 0x1000 - 0x10ff };
        let decision_single = policy.compute_xperms_access_decision(
            XpermsKind::Nlmsg,
            &source_context,
            &target_context_matched,
            KernelClass::NetlinkRouteSocket,
            0xab,
        );

        let mut expected_auditdeny =
            xperms_bitmap_from_elements((0x0..=0xff).collect::<Vec<_>>().as_slice());
        expected_auditdeny -= xperms_bitmap_from_elements(&[0xcd, 0xef]);

        let expected_decision_single = XpermsAccessDecision {
            allow: xperms_bitmap_from_elements(&[0xcd, 0xef]),
            auditallow: xperms_bitmap_from_elements(&[0xcd, 0xef]),
            auditdeny: expected_auditdeny,
        };
        assert_eq!(decision_single, expected_decision_single);

        let decision_range = policy.compute_xperms_access_decision(
            XpermsKind::Nlmsg,
            &source_context,
            &target_context_matched,
            KernelClass::NetlinkRouteSocket,
            0x10,
        );
        let expected_decision_range = XpermsAccessDecision {
            allow: XpermsBitmap::ALL,
            auditallow: XpermsBitmap::ALL,
            auditdeny: XpermsBitmap::NONE,
        };
        assert_eq!(decision_range, expected_decision_range);
    }

    #[test]
    fn compute_nlmsg_access_decision_unmatched() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");

        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");

        // No matching nlmsg xperm-related statements for this target's type
        let target_context_unmatched: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type1:s0-s0".into())
            .expect("create source security context");

        for prefix in 0x0..=0xff {
            let decision = policy.compute_xperms_access_decision(
                XpermsKind::Nlmsg,
                &source_context,
                &target_context_unmatched,
                KernelClass::NetlinkRouteSocket,
                prefix,
            );
            assert_eq!(decision, XpermsAccessDecision::ALLOW_ALL);
        }
    }

    #[test]
    fn compute_ioctl_grant_does_not_cause_nlmsg_deny() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id = unvalidated
            .0
            .classes()
            .get_by_name(b"class_ioctl_grant_does_not_cause_nlmsg_deny")
            .expect("look up class_ioctl_grant_does_not_cause_nlmsg_deny")
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_ioctl_grant_does_not_cause_nlmsg_deny` class:
        //
        // `allowxperm type0 self:class_ioctl_grant_does_not_cause_nlmsg_deny ioctl { 0x0002 };`
        let ioctl_decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x00,
        );
        assert_eq!(
            ioctl_decision,
            XpermsAccessDecision {
                allow: xperms_bitmap_from_elements(&[0x0002]),
                auditallow: XpermsBitmap::NONE,
                auditdeny: XpermsBitmap::ALL,
            }
        );
        let nlmsg_decision = policy.compute_xperms_access_decision(
            XpermsKind::Nlmsg,
            &source_context,
            &target_context_matched,
            class_id,
            0x00,
        );
        assert_eq!(nlmsg_decision, XpermsAccessDecision::ALLOW_ALL);
    }

    #[test]
    fn compute_nlmsg_grant_does_not_cause_ioctl_deny() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let unvalidated = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let class_id = unvalidated
            .0
            .classes()
            .get_by_name(b"class_nlmsg_grant_does_not_cause_ioctl_deny")
            .expect("look up class_nlmsg_grant_does_not_cause_ioctl_deny")
            .id();
        let policy = unvalidated.validate().expect("validate policy");
        let source_context: SecurityContext = policy
            .parse_security_context(b"user0:object_r:type0:s0-s0".into())
            .expect("create source security context");
        let target_context_matched: SecurityContext = source_context.clone();

        // `allowxperm` rules for the `class_nlmsg_grant_does_not_cause_ioctl_deny` class:
        //
        // `allowxperm type0 self:class_nlmsg_grant_does_not_cause_ioctl_deny nlmsg { 0x0003 };`
        let nlmsg_decision = policy.compute_xperms_access_decision(
            XpermsKind::Nlmsg,
            &source_context,
            &target_context_matched,
            class_id,
            0x00,
        );
        assert_eq!(
            nlmsg_decision,
            XpermsAccessDecision {
                allow: xperms_bitmap_from_elements(&[0x0003]),
                auditallow: XpermsBitmap::NONE,
                auditdeny: XpermsBitmap::ALL,
            }
        );
        let ioctl_decision = policy.compute_xperms_access_decision(
            XpermsKind::Ioctl,
            &source_context,
            &target_context_matched,
            class_id,
            0x00,
        );
        assert_eq!(ioctl_decision, XpermsAccessDecision::ALLOW_ALL);
    }

    #[test]
    fn compute_create_context_minimal() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/minimal_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, FileClass::File, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"source_u:object_r:target_t:s0:c0".into())
            .expect("valid expected security context");

        assert_eq!(expected, actual);
    }

    #[test]
    fn new_security_context_minimal() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/minimal_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, KernelClass::Process, &[]);

        assert_eq!(source, actual);
    }

    #[test]
    fn compute_create_context_class_defaults() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/class_defaults_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c0-s1:c0.c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, FileClass::File, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"target_u:source_r:source_t:s1:c0-s1:c0.c1".into())
            .expect("valid expected security context");

        assert_eq!(expected, actual);
    }

    #[test]
    fn new_security_context_class_defaults() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/class_defaults_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c0-s1:c0.c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, KernelClass::Process, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"target_u:source_r:source_t:s1:c0-s1:c0.c1".into())
            .expect("valid expected security context");

        assert_eq!(expected, actual);
    }

    #[test]
    fn compute_create_context_role_transition() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/role_transition_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, FileClass::File, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"source_u:transition_r:target_t:s0:c0".into())
            .expect("valid expected security context");

        assert_eq!(expected, actual);
    }

    #[test]
    fn new_security_context_role_transition() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/role_transition_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, KernelClass::Process, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"source_u:transition_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid expected security context");

        assert_eq!(expected, actual);
    }

    #[test]
    fn compute_create_context_role_transition_not_allowed() {
        let policy_bytes = include_bytes!(
            "../../testdata/composite_policies/compiled/role_transition_not_allowed_policy"
        );
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, FileClass::File, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"source_u:transition_r:target_t:s0:c0".into())
            .expect("valid expected security context");

        // Role-allow rules are not checked during `compute_create_context()`; they are checked
        // during `compute_access_decision()` on the "process" class.
        assert_eq!(expected, actual);
        assert!(policy.validate_security_context(&actual).is_ok());
    }

    #[test]
    fn compute_access_decision_role_allow() {
        // 1. With `role_transition_policy`, `allow source_r transition_r;` is present.
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/role_transition_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"source_u:transition_r:target_t:s0:c0".into())
            .expect("valid target security context");

        let decision_allowed =
            policy.compute_access_decision(&source, &target, KernelClass::Process);
        assert_ne!(decision_allowed.allow, AccessVector::NONE);

        // 2. With `role_transition_not_allowed_policy`, `allow source_r transition_r;` is missing.
        let policy_bytes = include_bytes!(
            "../../testdata/composite_policies/compiled/role_transition_not_allowed_policy"
        );
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"source_u:transition_r:target_t:s0:c0".into())
            .expect("valid target security context");

        let decision_denied =
            policy.compute_access_decision(&source, &target, KernelClass::Process);
        assert_eq!(decision_denied.allow, AccessVector::NONE);
    }

    #[test]
    fn compute_create_context_type_transition() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/type_transition_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, FileClass::File, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"source_u:object_r:transition_t:s0:c0".into())
            .expect("valid expected security context");

        assert_eq!(expected, actual);
    }

    #[test]
    fn new_security_context_type_transition() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/type_transition_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, KernelClass::Process, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"source_u:source_r:transition_t:s0:c0-s2:c0.c1".into())
            .expect("valid expected security context");

        assert_eq!(expected, actual);
    }

    #[test]
    fn compute_create_context_range_transition() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/range_transition_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, FileClass::File, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"source_u:object_r:target_t:s1:c1-s2:c1.c2".into())
            .expect("valid expected security context");

        assert_eq!(expected, actual);
    }

    #[test]
    fn new_security_context_range_transition() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/range_transition_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate policy");
        let source = policy
            .parse_security_context(b"source_u:source_r:source_t:s0:c0-s2:c0.c1".into())
            .expect("valid source security context");
        let target = policy
            .parse_security_context(b"target_u:target_r:target_t:s1:c1".into())
            .expect("valid target security context");

        let actual = policy.compute_create_context(&source, &target, KernelClass::Process, &[]);
        let expected: SecurityContext = policy
            .parse_security_context(b"source_u:source_r:source_t:s1:c1-s2:c1.c2".into())
            .expect("valid expected security context");

        assert_eq!(expected, actual);
    }

    #[test]
    fn access_vector_formats() {
        assert_eq!(format!("{:x}", AccessVector::NONE), "0");
        assert_eq!(format!("{:x}", AccessVector::ALL), "ffffffff");
        assert_eq!(format!("{:?}", AccessVector::NONE), "AccessVector(00000000)");
        assert_eq!(format!("{:?}", AccessVector::ALL), "AccessVector(ffffffff)");
    }

    #[test]
    fn policy_genfscon_root_path() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/genfscon_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate selinux policy");

        {
            let context = policy.genfscon_label_for_fs_and_path(
                "fs_with_path_rules".into(),
                "/".into(),
                None,
            );
            assert_eq!(
                policy.serialize_security_context(&context.unwrap()),
                b"system_u:object_r:fs_with_path_rules_t:s0"
            )
        }
        {
            let context = policy.genfscon_label_for_fs_and_path(
                "fs_2_with_path_rules".into(),
                "/".into(),
                None,
            );
            assert_eq!(
                policy.serialize_security_context(&context.unwrap()),
                b"system_u:object_r:fs_2_with_path_rules_t:s0"
            )
        }
    }

    #[test]
    fn policy_genfscon_subpaths() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/genfscon_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate selinux policy");

        let path_label_expectations = [
            // Matching paths defined in the policy:
            //    /a1/    -> fs_with_path_rules_a1_t
            //    /a1/b/c -> fs_with_path_rules_a1_b_c_t
            ("/a1/", "system_u:object_r:fs_with_path_rules_a1_t:s0"),
            ("/a1/b", "system_u:object_r:fs_with_path_rules_a1_t:s0"),
            ("/a1/b/c", "system_u:object_r:fs_with_path_rules_a1_b_c_t:s0"),
            // Matching paths defined in the policy:
            //    /a2/b    -> fs_with_path_rules_a2_b_t
            ("/a2/", "system_u:object_r:fs_with_path_rules_t:s0"),
            ("/a2/b/c/d", "system_u:object_r:fs_with_path_rules_a2_b_t:s0"),
            // Matching paths defined in the policy:
            //    /a3    -> fs_with_path_rules_a3_t
            ("/a3/b/c/d", "system_u:object_r:fs_with_path_rules_a3_t:s0"),
        ];
        for (path, expected_label) in path_label_expectations {
            let context = policy.genfscon_label_for_fs_and_path(
                "fs_with_path_rules".into(),
                path.into(),
                None,
            );
            assert_eq!(
                policy.serialize_security_context(&context.unwrap()),
                expected_label.as_bytes()
            )
        }
    }

    #[test]
    fn policy_genfscon_mixed_order() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/genfscon_policy");
        let policy = parse_policy_by_value(policy_bytes.to_vec()).expect("parse policy");
        let policy = policy.validate().expect("validate selinux policy");

        let path_label_expectations = [
            ("/", "system_u:object_r:fs_mixed_order_t:s0"),
            ("/a", "system_u:object_r:fs_mixed_order_a_t:s0"),
            ("/a/a", "system_u:object_r:fs_mixed_order_a_a_t:s0"),
            ("/a/b", "system_u:object_r:fs_mixed_order_a_b_t:s0"),
            ("/a/b/c", "system_u:object_r:fs_mixed_order_a_b_t:s0"),
        ];
        for (path, expected_label) in path_label_expectations {
            let context =
                policy.genfscon_label_for_fs_and_path("fs_mixed_order".into(), path.into(), None);
            assert_eq!(
                policy.serialize_security_context(&context.unwrap()),
                expected_label.as_bytes()
            );
        }
    }
}
