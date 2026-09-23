// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::security_context::SecurityContext;
use super::{
    AccessDecision, AccessVector, ClassId, MlsLevel, ParsedPolicy, PermissionId, RoleId, TypeId,
};
use crate::new_policy::rules::{HasRuleKey, RuleKind};
use crate::new_policy::traits::{HasName, HasPolicyId};
use crate::new_policy::{
    Class, ClassDefault, ClassDefaultRange, CommonSymbol, FsUseType, GenfsConPath, HandleUnknown,
    IdAndNameIndexed, SymbolArray,
};
use crate::{
    ClassPermission as _, KernelClass, KernelPermission, NullessByteStr, PolicyCap,
    ProcessPermission,
};

use std::collections::HashMap;
use std::ops::Deref;

use strum::VariantArray as _;

/// The [`SecurityContext`] and [`FsUseType`] derived from some `fs_use_*` line of the policy.
pub struct FsUseLabelAndType {
    pub context: SecurityContext,
    pub use_type: FsUseType,
}

/// Array of `PermissionId` values each of a kernel security class' permissions.
type KernelPermissionIdsArray = [Option<PermissionId>; 32];

/// An index for facilitating fast lookup of common abstractions inside parsed binary policy data
/// structures. Typically, data is indexed by an enum that describes a well-known value and the
/// index stores the offset of the data in the binary policy to avoid scanning a collection to find
/// an element that contains a matching string. For example, the policy contains a collection of
/// classes that are identified by string names included in each collection entry. However,
/// `policy_index.classes(KernelClass::Process).unwrap()` yields the offset in the policy's
/// collection of classes where the "process" class resides.
#[derive(Debug)]
pub struct PolicyIndex {
    /// Map from [`KernelClass`]es to their corresponding [`ClassId`]s in the associated policy's
    /// [`super::symbols::Classes`] collection.
    classes: HashMap<KernelClass, ClassId>,
    /// Index mapping kernel class permissions to their policy-specific `AccessVector` bit index.
    permissions: [KernelPermissionIdsArray; KernelClass::VARIANTS.len()],
    /// The parsed binary policy.
    parsed_policy: ParsedPolicy,
    /// The "object_r" role used as a fallback for new file context transitions.
    cached_object_r_role: RoleId,
    /// The cached `ClassId` for the "process" class, if defined by the policy.
    cached_process_class: Option<ClassId>,
}

impl PolicyIndex {
    /// Constructs a [`PolicyIndex`] that indexes over well-known policy elements.
    ///
    /// [`Class`]es and [`Permission`]s used by the kernel are amongst the indexed elements.
    /// The policy's `handle_unknown()` configuration determines whether the policy can be loaded even
    /// if it omits classes or permissions expected by the kernel, and whether to allow or deny those
    /// permissions if so.
    pub fn new(parsed_policy: ParsedPolicy) -> Result<Self, anyhow::Error> {
        let policy_classes = parsed_policy.classes();
        let common_symbols = parsed_policy.common_symbols();

        let mut classes = HashMap::with_capacity(crate::KernelClass::VARIANTS.len());

        // Insert elements for each kernel object class. If the policy defines that unknown
        // kernel classes should cause rejection then return an error describing the missing
        // element.
        for known_class in crate::KernelClass::VARIANTS {
            match policy_classes.get_by_name(known_class.name().as_bytes()) {
                Some(class) => {
                    classes.insert(*known_class, class.id());
                }
                None => {
                    if parsed_policy.handle_unknown() == HandleUnknown::Reject {
                        return Err(anyhow::anyhow!("missing object class {:?}", known_class,));
                    }
                }
            }
        }

        // Allow unused space in the classes map to be released.
        classes.shrink_to_fit();

        // Accumulate permissions indexed by kernel permission enum. If the policy defines that
        // unknown permissions or classes should cause rejection then return an error describing the
        // missing element.
        let mut permissions = [KernelPermissionIdsArray::default(); _];
        for kernel_permission in crate::KernelPermission::all_variants() {
            let kernel_class_name = kernel_permission.class().name();
            if let Some(class) = policy_classes.get_by_name(kernel_class_name.as_bytes()) {
                if let Some(permission_id) =
                    get_permission_id_by_name(common_symbols, class, kernel_permission.name())
                {
                    let kernel_class_id = kernel_permission.class() as usize;
                    let kernel_permission_id = kernel_permission.id() as usize;
                    permissions[kernel_class_id][kernel_permission_id] = Some(permission_id);
                } else if parsed_policy.handle_unknown() == HandleUnknown::Reject {
                    return Err(anyhow::anyhow!(
                        "missing permission {:?}:{:?}",
                        kernel_class_name,
                        kernel_permission.name(),
                    ));
                }
            }
        }

        // Locate the "object_r" role.
        let cached_object_r_role = parsed_policy
            .roles()
            .get_by_name(b"object_r")
            .ok_or_else(|| anyhow::anyhow!("missing 'object_r' role"))?
            .id();

        let cached_process_class = classes.get(&KernelClass::Process).copied();

        let index = Self {
            classes,
            permissions,
            parsed_policy,
            cached_object_r_role,
            cached_process_class,
        };

        Ok(index)
    }

    /// Returns the policy entry for a class identified either by its well-known kernel object class
    /// enum value, or its policy-defined Id.
    pub(super) fn class(&self, object_class: crate::ObjectClass) -> Option<&Class> {
        match object_class {
            crate::ObjectClass::Kernel(kernel_class) => {
                let &class_id = self.classes.get(&kernel_class)?;
                self.classes().get_by_id(class_id)
            }
            crate::ObjectClass::ClassId(class_id) => self.classes().get_by_id(class_id),
        }
    }

    /// Returns the policy entry for a well-known kernel object class permission.
    pub fn kernel_permission_to_access_vector<P: Into<KernelPermission>>(
        &self,
        permission: P,
    ) -> Option<AccessVector> {
        let permission = permission.into();
        let class_index = permission.class() as usize;
        let permission_index = permission.id() as usize;
        let permission_id = self.permissions[class_index][permission_index]?;
        Some(permission_id.into())
    }

    /// Returns the security context that should be applied to a newly created SELinux
    /// object according to `source` and `target` security contexts, as well as the new object's
    /// `class` and `name`.
    ///
    /// Computation follows the "create" algorithm for labeling newly created objects:
    /// - user is taken from the `source`.
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
    pub fn compute_create_context(
        &self,
        source: &SecurityContext,
        target: &SecurityContext,
        class: crate::ObjectClass,
        name: &[u8],
    ) -> SecurityContext {
        let override_type = if !name.is_empty() {
            self.class(class).and_then(|policy_class| {
                self.type_transition_new_type_with_name(
                    source.type_(),
                    target.type_(),
                    &policy_class,
                    name,
                )
            })
        } else {
            None
        };
        self.new_security_context_internal(source, target, class, override_type)
    }

    /// Internal implementation used by [`Self::compute_create_context`] to implement the policy transition calculations.
    /// If `override_type` is specified then the supplied value will be applied rather than a value
    /// being calculated based on the policy; this is used by [`Self::compute_create_context`]
    /// when a filename transition matches to shortcut the default `type_transition` lookup.
    fn new_security_context_internal(
        &self,
        source: &SecurityContext,
        target: &SecurityContext,
        target_class: crate::ObjectClass,
        override_type: Option<TypeId>,
    ) -> SecurityContext {
        let Some(policy_class) = self.class(target_class) else {
            // If the class is not defined in the policy then there can be no transitions, nor
            // class-defined choice of defaults, so default to the non-process-or-socket behaviour.
            // TODO: https://fxbug.dev/361552580 - For `KernelClass`es, apply the kernel's notion
            // of whether the class is "process", or socket-like?
            return SecurityContext::new(
                source.user(),
                self.cached_object_r_role,
                target.type_(),
                source.low_level().clone(),
                None,
            );
        };

        let is_process_or_socket =
            policy_class.name() == b"process" || policy_class.common_name() == b"socket";
        let (unspecified_role, unspecified_type, unspecified_low, unspecified_high) =
            if is_process_or_socket {
                (source.role(), source.type_(), source.low_level(), source.high_level())
            } else {
                (self.cached_object_r_role, target.type_(), source.low_level(), None)
            };
        let class_defaults = policy_class.defaults();

        let user = match class_defaults.user() {
            ClassDefault::Source => source.user(),
            ClassDefault::Target => target.user(),
            ClassDefault::Unspecified => source.user(),
        };

        let role = match self.role_transition_new_role(source.role(), target.type_(), &policy_class)
        {
            Some(new_role) => new_role,
            None => match class_defaults.role() {
                ClassDefault::Source => source.role(),
                ClassDefault::Target => target.role(),
                ClassDefault::Unspecified => unspecified_role,
            },
        };

        let type_ = override_type.unwrap_or_else(|| {
            let transition = self
                .access_vector_rules()
                .find_type_rules(source.type_(), target.type_(), policy_class.id())
                .find(|rule| rule.kind() == RuleKind::TypeTransition)
                .map(|rule| rule.new_type());
            match transition {
                Some(new_type) => new_type,
                None => match class_defaults.type_() {
                    ClassDefault::Source => source.type_(),
                    ClassDefault::Target => target.type_(),
                    ClassDefault::Unspecified => unspecified_type,
                },
            }
        });

        let (low_level, high_level) =
            match self.range_transition_new_range(source.type_(), target.type_(), &policy_class) {
                Some((low_level, high_level)) => (low_level, high_level),
                None => match class_defaults.range() {
                    ClassDefaultRange::SourceLow => (source.low_level().clone(), None),
                    ClassDefaultRange::SourceHigh => {
                        (source.high_level().unwrap_or_else(|| source.low_level()).clone(), None)
                    }
                    ClassDefaultRange::SourceLowHigh => {
                        (source.low_level().clone(), source.high_level().cloned())
                    }
                    ClassDefaultRange::TargetLow => (target.low_level().clone(), None),
                    ClassDefaultRange::TargetHigh => {
                        (target.high_level().unwrap_or_else(|| target.low_level()).clone(), None)
                    }
                    ClassDefaultRange::TargetLowHigh => {
                        (target.low_level().clone(), target.high_level().cloned())
                    }
                    ClassDefaultRange::Unspecified => {
                        (unspecified_low.clone(), unspecified_high.cloned())
                    }
                    ClassDefaultRange::UnknownUsedValue => {
                        unreachable!("Invalid ClassDefaultRange in validated policy")
                    }
                },
            };

        // TODO(http://b/334968228): Validate domain & role transitions are allowed?
        SecurityContext::new(user, role, type_, low_level, high_level)
    }

    /// Evaluates the access rights allowed, and whether an audit should be emitted for any allowed
    /// or denied permissions, by `source_context` acting on `target_context` as `target_class`.
    pub(super) fn compute_access_decision(
        &self,
        source_context: &SecurityContext,
        target_context: &SecurityContext,
        target_class: &Class,
    ) -> AccessDecision {
        let mut access_decision = self.parsed_policy.compute_access_decision(
            source_context,
            target_context,
            target_class,
        );

        // Process domain transitions ("transition" and "dyntransition") across different roles
        // require explicit authorization in policy via a role allow rule ("allow old_role new_role;").
        if source_context.role() != target_context.role()
            && Some(target_class.id()) == self.cached_process_class
        {
            let process_trans_perms = self.process_trans_perms();
            if (access_decision.allow & process_trans_perms) != AccessVector::NONE
                && !self.role_transition_is_explicitly_allowed(
                    source_context.role(),
                    target_context.role(),
                )
            {
                // The source is granted one or both of the "transition" permissions, but the role
                // transition is not explicitly allowed, so remove those permissions from the
                // returned decision.
                access_decision.allow -= process_trans_perms;
            }
        }

        access_decision
    }

    /// Returns the combined permissions mask for process `transition` and `dyntransition`.
    fn process_trans_perms(&self) -> AccessVector {
        let mut perms = self
            .kernel_permission_to_access_vector(ProcessPermission::Transition)
            .unwrap_or(AccessVector::NONE);
        perms |= self
            .kernel_permission_to_access_vector(ProcessPermission::DynTransition)
            .unwrap_or(AccessVector::NONE);
        perms
    }

    /// Returns the Id of the "object_r" role within the `parsed_policy`, for use when validating
    /// Security Context fields.
    pub(super) fn object_role(&self) -> RoleId {
        self.cached_object_r_role
    }

    /// Returns the [`SecurityContext`] defined by this policy for the specified
    /// well-known (or "initial") Id.
    pub(super) fn initial_context(&self, id: crate::InitialSid) -> SecurityContext {
        // All [`InitialSid`] have already been verified as resolvable, by `new()`.
        SecurityContext::from_policy_context(self.parsed_policy.initial_context(id))
    }

    /// If there is an fs_use statement for the given filesystem type, returns the associated
    /// [`SecurityContext`] and [`FsUseType`].
    pub(super) fn fs_use_label_and_type(
        &self,
        fs_type: NullessByteStr<'_>,
    ) -> Option<FsUseLabelAndType> {
        self.object_contexts()
            .fs_uses()
            .iter()
            .find(|fs_use| fs_use.fs_type() == fs_type.as_bytes())
            .map(|fs_use| FsUseLabelAndType {
                context: SecurityContext::from_policy_context(fs_use.context()),
                use_type: fs_use.behavior(),
            })
    }

    /// If there is a genfscon statement for the given filesystem type, returns the associated
    /// [`SecurityContext`], taking the `node_path` into account. `class_id` defines the type
    /// of the file in the given `node_path`. It can only be omitted when looking up the filesystem
    /// label.
    pub(super) fn genfscon_label_for_fs_and_path(
        &self,
        fs_type: NullessByteStr<'_>,
        node_path: NullessByteStr<'_>,
        class: Option<crate::KernelClass>,
    ) -> Option<SecurityContext> {
        let node_path = if class == Some(crate::FileClass::LnkFile.into())
            && !self.has_policycap(PolicyCap::GenfsSeclabelSymlinks)
        {
            // Symlinks receive the filesystem root label by default, rather than a label dependent on
            // the `node_path`. Path based labels may be enabled with the "genfs_seclabel_symlinks"
            // policy capability.
            "/".into()
        } else {
            node_path
        };

        let class_id = class.and_then(|class| self.class(class.into())).map(|class| class.id());

        // All contexts listed in the policy for the file system type.
        let fs_contexts = self.genfscon_find_all(fs_type.as_bytes());

        #[derive(PartialEq)]
        enum OrderType {
            Alphabetic,
            ByLength,
            Unknown,
        }
        // The correct match is the closest parent among the ones given in the policy file.
        // E.g. if in the policy we have
        //     genfscon foofs "/" label1
        //     genfscon foofs "/abc/" label2
        //     genfscon foofs "/abc/def" label3
        //
        // The correct label for a file "/abc/def/g/h/i" is label3, as "/abc/def" is the closest parent
        // among those defined.
        //
        // Partial paths are prefix-matched, so that "/abc/default" would also be assigned label3.
        //
        // TODO(372212126): Optimize the algorithm.
        let mut result: Option<&GenfsConPath> = None;
        let mut order_type = OrderType::Unknown;
        let mut prev_path_bytes: Option<Vec<u8>> = None;
        for fs_context in fs_contexts {
            // Determine the order type based on the first entries.
            let path = fs_context.partial_path();
            if order_type == OrderType::Unknown {
                if let Some(prev) = &prev_path_bytes {
                    if path.len() > prev.len() {
                        order_type = OrderType::Alphabetic;
                    } else if path < prev.as_slice() {
                        order_type = OrderType::ByLength;
                    }
                }
                prev_path_bytes = Some(path.to_vec());
            }

            // Check if the class matches.
            let class_matches = class_id.is_none()
                || fs_context
                    .class()
                    .map(|other| other == class_id.unwrap().into())
                    .unwrap_or(true);
            if !class_matches {
                continue;
            }

            if order_type == OrderType::Alphabetic && fs_context.partial_path() > node_path.0 {
                // We know that:
                // - We have alphabetic order,
                // - The current path is lexicographically greater than our target path.
                // We can infer that we have passed any potential prefixes in alphabetical order.
                break;
            }

            if node_path.0.starts_with(fs_context.partial_path()) {
                if result
                    .as_ref()
                    .map_or(true, |c| c.partial_path().len() < fs_context.partial_path().len())
                {
                    // The path matches, and it's the closest parent so far.
                    result = Some(fs_context);
                    if order_type == OrderType::ByLength {
                        break;
                    }
                }
            }
        }

        // The returned SecurityContext must be valid with respect to the policy, since otherwise
        // we'd have rejected the policy load.
        Some(SecurityContext::from_policy_context(result?.context()))
    }

    fn role_transition_new_role(
        &self,
        current_role: RoleId,
        type_: TypeId,
        class: &Class,
    ) -> Option<RoleId> {
        self.role_transitions()
            .iter()
            .find(|role_transition| {
                role_transition.current_role() == current_role
                    && role_transition.type_() == type_
                    && role_transition.class() == class.id().into()
            })
            .map(|x| x.new_role())
    }

    fn role_transition_is_explicitly_allowed(&self, source_role: RoleId, new_role: RoleId) -> bool {
        self.role_allowlist().iter().any(|role_allow| {
            role_allow.source_role() == source_role && role_allow.new_role() == new_role
        })
    }

    fn type_transition_new_type_with_name(
        &self,
        source_type: TypeId,
        target_type: TypeId,
        class: &Class,
        name: &[u8],
    ) -> Option<TypeId> {
        self.compute_filename_transition(source_type, target_type, class.id().into(), name)
    }

    fn range_transition_new_range(
        &self,
        source_type: TypeId,
        target_type: TypeId,
        class: &Class,
    ) -> Option<(MlsLevel, Option<MlsLevel>)> {
        for range_transition in self.range_transitions() {
            if range_transition.source_type() == source_type
                && range_transition.target_type() == target_type
                && range_transition.target_class() == class.id()
            {
                let mls_range = range_transition.mls_range();
                let low_level = mls_range.low().clone();
                let high_level = mls_range.high().clone();
                return Some((low_level, high_level));
            }
        }

        None
    }
}

/// Returns the bit index of the specified permission for the specified security `class`, looking
/// up the permission in the class' common symbol, if any.
fn get_permission_id_by_name(
    common_symbols: &IdAndNameIndexed<SymbolArray<CommonSymbol>>,
    class: &Class,
    name: &str,
) -> Option<PermissionId> {
    let name = name.as_bytes();
    if let Some(permission) = class.permissions().iter().find(|p| p.name() == name) {
        return Some(permission.id());
    }
    let common_name = class.common_name();
    if !common_name.is_empty() {
        let common_symbol = common_symbols.get_by_name(common_name)?;
        let permission = common_symbol.permissions().iter().find(|p| p.name() == name)?;
        return Some(permission.id());
    }
    None
}

impl Deref for PolicyIndex {
    type Target = ParsedPolicy;

    fn deref(&self) -> &Self::Target {
        &self.parsed_policy
    }
}
