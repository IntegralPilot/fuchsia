// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZeroU32;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not, Sub, SubAssign};
use std::sync::atomic::{AtomicU64, Ordering};

use hashbrown::HashTable;
use hashbrown::hash_table::Entry;
use rapidhash::RapidBuildHasher;
pub use selinux_policy_derive::{Parse, Serialize, Validate};

use super::error::{ParseError, SerializeError, ValidateError};
use super::parser::{Array, PolicyCursor, PolicyWriter};
use super::traits::{Parse, PolicyId, Serialize, Validate};
use super::{AccessVector, ClassId, ConditionalBooleanId, NewPolicy, TypeId, U24Index};

/// Flag bit for standard `allow` rules.
pub const AV_ALLOW_RULE_FLAG: u16 = 0x1;
/// Flag bit for `auditallow` rules.
pub const AV_AUDITALLOW_RULE_FLAG: u16 = 0x2;
/// Flag bit for `dontaudit` rules.
pub const AV_DONTAUDIT_RULE_FLAG: u16 = 0x4;

/// Flag bit for `type_transition` rules.
pub const AV_TYPE_TRANSITION_RULE_FLAG: u16 = 0x10;
/// Flag bit for `type_member` rules.
pub const AV_TYPE_MEMBER_RULE_FLAG: u16 = 0x20;
/// Flag bit for `type_change` rules.
pub const AV_TYPE_CHANGE_RULE_FLAG: u16 = 0x40;

/// Flag bit for `allowxperm` extended permissions rules.
pub const AV_ALLOWXPERM_RULE_FLAG: u16 = 0x100;
/// Flag bit for `auditallowxperm` extended permissions rules.
pub const AV_AUDITALLOWXPERM_RULE_FLAG: u16 = 0x200;
/// Flag bit for `dontauditxperm` extended permissions rules.
pub const AV_DONTAUDITXPERM_RULE_FLAG: u16 = 0x400;

/// Mask for high bit in rule type flags indicating whether rule is enabled.
pub const AV_ENABLED_RULE_FLAG: u16 = 0x8000;

/// [`AccessDecision::flags`] value indicating that policy marks source domain permissive.
pub const SELINUX_AVD_FLAGS_PERMISSIVE: u32 = 1;

/// Extended permissions type for ioctl driver prefix and 8-bit postfix sets.
pub const XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES: u8 = 1;
/// Extended permissions type for ioctl 8-bit driver prefixes.
pub const XPERMS_TYPE_IOCTL_PREFIXES: u8 = 2;
/// Extended permissions type for netlink message types.
pub const XPERMS_TYPE_NLMSG: u8 = 3;

/// Number of 64-bit words in 256-bit [`XpermsBitmap`].
pub const XPERMS_BITMAP_BLOCKS: usize = 4;

/// 256-bit bitmap used for extended permissions (such as ioctls and netlink messages).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct XpermsBitmap([u64; XPERMS_BITMAP_BLOCKS]);

impl Parse for XpermsBitmap {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let mut words = [0u64; XPERMS_BITMAP_BLOCKS];
        for word in words.iter_mut() {
            let low = cursor.parse::<u32>()? as u64;
            let high = cursor.parse::<u32>()? as u64;
            *word = low | (high << 32);
        }
        Ok(Self(words))
    }
}

impl Serialize for XpermsBitmap {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        for &word in self.0.iter() {
            (word as u32).serialize(writer)?;
            ((word >> 32) as u32).serialize(writer)?;
        }
        Ok(())
    }
}

impl XpermsBitmap {
    pub const BITMAP_BLOCKS: usize = XPERMS_BITMAP_BLOCKS;
    /// Bitmap with all 256 bits set to 1.
    pub const ALL: Self = Self([u64::MAX; Self::BITMAP_BLOCKS]);
    /// Empty bitmap with all bits set to 0.
    pub const NONE: Self = Self([0u64; Self::BITMAP_BLOCKS]);

    /// Constructs a new [`XpermsBitmap`] from an array of four 64-bit words.
    pub fn new(elements: [u64; Self::BITMAP_BLOCKS]) -> Self {
        Self(elements)
    }

    /// Returns `true` if the bit corresponding to `value` is set in this bitmap.
    pub fn contains(&self, value: u8) -> bool {
        let block_index = (value as usize) / (u64::BITS as usize);
        let bit_index = (value as usize) % (u64::BITS as usize);
        self.0[block_index] & (1u64 << bit_index) != 0
    }

    /// Constructs an [`XpermsBitmap`] by loading words from an array of atomic 64-bit integers using relaxed ordering.
    pub fn from_atomics(atomics: &[AtomicU64; Self::BITMAP_BLOCKS]) -> Self {
        let mut words = [0u64; Self::BITMAP_BLOCKS];
        for (i, word) in words.iter_mut().enumerate() {
            *word = atomics[i].load(Ordering::Relaxed);
        }
        Self(words)
    }

    /// Stores this bitmap into an array of atomic 64-bit integers using relaxed ordering.
    pub fn to_atomics(&self, atomics: &[AtomicU64; Self::BITMAP_BLOCKS]) {
        for (i, word) in self.0.iter().enumerate() {
            atomics[i].store(*word, Ordering::Relaxed);
        }
    }
}

impl BitAnd for XpermsBitmap {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(std::array::from_fn(|i| self.0[i] & rhs.0[i]))
    }
}

impl BitAndAssign for XpermsBitmap {
    fn bitand_assign(&mut self, rhs: Self) {
        for i in 0..4 {
            self.0[i] &= rhs.0[i];
        }
    }
}

impl BitOr for XpermsBitmap {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(std::array::from_fn(|i| self.0[i] | rhs.0[i]))
    }
}

impl BitOrAssign for XpermsBitmap {
    fn bitor_assign(&mut self, rhs: Self) {
        for i in 0..4 {
            self.0[i] |= rhs.0[i];
        }
    }
}

impl Sub for XpermsBitmap {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] & !rhs.0[i]))
    }
}

impl SubAssign for XpermsBitmap {
    fn sub_assign(&mut self, rhs: Self) {
        for i in 0..4 {
            self.0[i] &= !rhs.0[i];
        }
    }
}

impl Not for XpermsBitmap {
    type Output = Self;
    fn not(self) -> Self::Output {
        Self(self.0.map(|word| !word))
    }
}

impl Validate for XpermsBitmap {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

/// Extended permissions specification (e.g., ioctl commands or netlink message types) associated with an access vector rule.
#[derive(Clone, Debug, PartialEq, Eq, Parse, Serialize)]
pub struct ExtendedPermissions {
    xperms_type: u8,
    xperms_optional_prefix: u8,
    xperms_bitmap: XpermsBitmap,
}

impl Validate for ExtendedPermissions {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        match self.xperms_type {
            XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES
            | XPERMS_TYPE_IOCTL_PREFIXES
            | XPERMS_TYPE_NLMSG => Ok(()),
            v => Err(ValidateError::InvalidExtendedPermissionsType { value: v }),
        }
    }
}

impl ExtendedPermissions {
    /// Returns the raw extended permissions type identifier (e.g. ioctl or netlink message format).
    pub fn xperms_type(&self) -> u8 {
        self.xperms_type
    }

    /// Returns the optional 8-bit prefix specified by this extended permissions block, if any.
    pub fn xperms_optional_prefix(&self) -> u8 {
        self.xperms_optional_prefix
    }

    /// Returns a reference to the underlying [`XpermsBitmap`].
    pub fn xperms_bitmap(&self) -> &XpermsBitmap {
        &self.xperms_bitmap
    }

    /// Returns the total number of individual permissions specified by this bitmap.
    #[cfg(test)]
    pub fn count(&self) -> u64 {
        let count = self
            .xperms_bitmap
            .0
            .iter()
            .fold(0, |count, block| count as u64 + block.count_ones() as u64);
        match self.xperms_type {
            XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES | XPERMS_TYPE_NLMSG => count,
            XPERMS_TYPE_IOCTL_PREFIXES => count * 0x100,
            _ => unreachable!("invalid xperms_type in validated ExtendedPermissions"),
        }
    }

    /// Returns `true` if the specified extended permission `xperm` is included in this rule.
    #[cfg(test)]
    pub fn contains(&self, xperm: u16) -> bool {
        let [postfix, prefix] = xperm.to_le_bytes();
        if (self.xperms_type == XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES
            || self.xperms_type == XPERMS_TYPE_NLMSG)
            && self.xperms_optional_prefix != prefix
        {
            return false;
        }
        let value = match self.xperms_type {
            XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES | XPERMS_TYPE_NLMSG => postfix,
            XPERMS_TYPE_IOCTL_PREFIXES => prefix,
            _ => unreachable!("invalid xperms_type in validated ExtendedPermissions"),
        };
        self.xperms_bitmap.contains(value)
    }
}

/// Compact enum identifying the type and target array for a rule in sequential policy order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RuleKind {
    Allow,
    AuditAllow,
    DontAudit,
    TypeTransition,
    TypeMember,
    TypeChange,
    AllowXperm,
    AuditAllowXperm,
    DontAuditXperm,
}

impl TryFrom<u16> for RuleKind {
    type Error = ParseError;

    fn try_from(rule_type: u16) -> Result<Self, Self::Error> {
        let base_kind = rule_type & !AV_ENABLED_RULE_FLAG;
        match base_kind {
            AV_ALLOW_RULE_FLAG => Ok(Self::Allow),
            AV_AUDITALLOW_RULE_FLAG => Ok(Self::AuditAllow),
            AV_DONTAUDIT_RULE_FLAG => Ok(Self::DontAudit),
            AV_TYPE_TRANSITION_RULE_FLAG => Ok(Self::TypeTransition),
            AV_TYPE_MEMBER_RULE_FLAG => Ok(Self::TypeMember),
            AV_TYPE_CHANGE_RULE_FLAG => Ok(Self::TypeChange),
            AV_ALLOWXPERM_RULE_FLAG => Ok(Self::AllowXperm),
            AV_AUDITALLOWXPERM_RULE_FLAG => Ok(Self::AuditAllowXperm),
            AV_DONTAUDITXPERM_RULE_FLAG => Ok(Self::DontAuditXperm),
            _ => {
                Err(ParseError::InvalidEnumValue { enum_name: "RuleKind", value: rule_type as u64 })
            }
        }
    }
}

impl From<RuleKind> for u16 {
    fn from(kind: RuleKind) -> Self {
        match kind {
            RuleKind::Allow => AV_ALLOW_RULE_FLAG,
            RuleKind::AuditAllow => AV_AUDITALLOW_RULE_FLAG,
            RuleKind::DontAudit => AV_DONTAUDIT_RULE_FLAG,
            RuleKind::TypeTransition => AV_TYPE_TRANSITION_RULE_FLAG,
            RuleKind::TypeMember => AV_TYPE_MEMBER_RULE_FLAG,
            RuleKind::TypeChange => AV_TYPE_CHANGE_RULE_FLAG,
            RuleKind::AllowXperm => AV_ALLOWXPERM_RULE_FLAG,
            RuleKind::AuditAllowXperm => AV_AUDITALLOWXPERM_RULE_FLAG,
            RuleKind::DontAuditXperm => AV_DONTAUDITXPERM_RULE_FLAG,
        }
    }
}

impl Validate for RuleKind {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

/// Standard access vector rule (allow, auditallow, dontaudit).
#[derive(Clone, Debug, Validate)]
pub struct AccessRule {
    key: RuleKey,
    kind: RuleKind,
    access_vector: AccessVector,
    enabled: bool,
}

impl AccessRule {
    /// Returns the [`AccessVector`] for this rule.
    pub fn access_vector(&self) -> AccessVector {
        self.access_vector
    }

    /// Returns whether this rule is enabled.
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// Type transition, change, or member rule.
#[derive(Clone, Debug, Validate)]
pub struct TypeRule {
    key: RuleKey,
    kind: RuleKind,
    new_type: TypeId,
    enabled: bool,
}

impl TypeRule {
    /// Returns the target type ID for this rule transition.
    pub fn new_type(&self) -> TypeId {
        self.new_type
    }

    /// Returns whether this rule is enabled.
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// Extended permissions rule (allowxperm, auditallowxperm, dontauditxperm).
#[derive(Clone, Debug, Validate)]
pub struct XpermRule {
    key: RuleKey,
    kind: RuleKind,
    extended_permissions: ExtendedPermissions,
    enabled: bool,
}

impl XpermRule {
    /// Returns the extended permissions block for this rule.
    pub fn extended_permissions(&self) -> &ExtendedPermissions {
        &self.extended_permissions
    }

    /// Returns whether this rule is enabled.
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// Lookup key for indexing and matching access vector rules by source domain, target domain, and class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Validate)]
pub struct RuleKey {
    source_type: TypeId,
    target_type: TypeId,
    class: ClassId,
}

impl RuleKey {
    /// Constructs a [`RuleKey`] for the specified source domain, target domain, and security class.
    pub fn new(source_type: TypeId, target_type: TypeId, class: ClassId) -> Self {
        Self { source_type, target_type, class }
    }

    /// Hashes this [`RuleKey`] using `hasher`.
    pub fn hash(&self, hasher: &RapidBuildHasher) -> u64 {
        use std::hash::{BuildHasher, Hash, Hasher};
        let mut state = hasher.build_hasher();
        Hash::hash(self, &mut state);
        state.finish()
    }

    /// Constructs a [`BinaryAccessVectorRuleHeader`] for this [`RuleKey`], [`RuleKind`], and enabled flag.
    fn to_header(&self, kind: RuleKind, enabled: bool) -> BinaryAccessVectorRuleHeader {
        let mut rule_flags = u16::from(kind);
        if enabled {
            rule_flags |= AV_ENABLED_RULE_FLAG;
        }
        BinaryAccessVectorRuleHeader {
            source_type: self.source_type.as_u16(),
            target_type: self.target_type.as_u16(),
            class: self.class.as_u16(),
            rule_flags,
        }
    }
}

/// Trait implemented by rule types that provide a [`RuleKey`] and [`RuleKind`].
pub trait HasRuleKey {
    /// Returns the [`RuleKey`] for this rule.
    fn key(&self) -> RuleKey;

    /// Returns the [`RuleKind`] for this rule.
    fn kind(&self) -> RuleKind;
}

impl HasRuleKey for AccessRule {
    fn key(&self) -> RuleKey {
        self.key
    }
    fn kind(&self) -> RuleKind {
        self.kind
    }
}

impl HasRuleKey for TypeRule {
    fn key(&self) -> RuleKey {
        self.key
    }
    fn kind(&self) -> RuleKind {
        self.kind
    }
}

impl HasRuleKey for XpermRule {
    fn key(&self) -> RuleKey {
        self.key
    }
    fn kind(&self) -> RuleKind {
        self.kind
    }
}

/// Encapsulates the result of a permissions calculation, between
/// source & target domains, for a specific class. Decisions describe
/// which permissions are allowed, and whether permissions should be
/// audit-logged when allowed, and when denied.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessDecision {
    pub allow: AccessVector,
    pub auditallow: AccessVector,
    pub auditdeny: AccessVector,
    pub flags: u32,

    /// If this field is set then denials should be audit-logged with "todo_deny" as the reason, with
    /// the `bug` number included in the audit message.
    pub todo_bug: Option<NonZeroU32>,
}

impl Default for AccessDecision {
    fn default() -> Self {
        Self::allow(AccessVector::NONE)
    }
}

impl AccessDecision {
    /// Returns an [`AccessDecision`] with the specified permissions to `allow`, and default audit
    /// behaviour.
    pub const fn allow(allow: AccessVector) -> Self {
        Self {
            allow,
            auditallow: AccessVector::NONE,
            auditdeny: AccessVector::ALL,
            flags: 0,
            todo_bug: None,
        }
    }
}

/// Lookup table for SELinux rules, optimized for fast queries. It maps a [`RuleKey`]
/// (source, target, class) to matching rules.
///
/// Rules are grouped into three contiguous arrays based on their payload struct:
/// 1. [`AccessRule`] (`av_rules`): allow, auditallow, dontaudit.
/// 2. [`TypeRule`] (`type_rules`): transitions.
/// 3. [`XpermRule`] (`xperm_rules`): allowxperm, auditallowxperm, dontauditxperm.
///
/// Three corresponding [`HashTable`]s map [`RuleKey`]s to the index of the **first** matching rule.
/// Lookups return an iterator that starts at that index and yields rules until the
/// key changes. This works because binary policies guarantee rules for the same key
/// are contiguous.
#[derive(Debug, Clone)]
pub struct AccessVectorRules {
    av_rules: Box<[AccessRule]>,
    type_rules: Box<[TypeRule]>,
    xperm_rules: Box<[XpermRule]>,
    rule_order: Box<[RuleKind]>,
}

impl AccessVectorRules {
    /// Returns the standard access vector rules.
    #[cfg(test)]
    pub fn av_rules(&self) -> &[AccessRule] {
        &self.av_rules
    }
}

impl Parse for AccessVectorRules {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let count = u32::parse(cursor)? as usize;

        let mut av_rules = Vec::new();
        let mut type_rules = Vec::new();
        let mut xperm_rules = Vec::new();
        let mut rule_order = Vec::with_capacity(count);

        // Split rules out based on the three different kinds of payload (access vector, type, or
        // extended permissions block).
        for _ in 0..count {
            let header = BinaryAccessVectorRuleHeader::parse(cursor)?;
            let kind = RuleKind::try_from(header.rule_flags)?;
            let enabled = (header.rule_flags & AV_ENABLED_RULE_FLAG) != 0;

            let source_val = header.source_type;
            let source_type = TypeId::from_u16(source_val)
                .ok_or(ParseError::InvalidId { value: source_val as u32 })?;

            let target_val = header.target_type;
            let target_type = TypeId::from_u16(target_val)
                .ok_or(ParseError::InvalidId { value: target_val as u32 })?;

            let class_val = header.class;
            let class = ClassId::from_u16(class_val)
                .ok_or(ParseError::InvalidId { value: class_val as u32 })?;

            let key = RuleKey::new(source_type, target_type, class);

            match kind {
                RuleKind::AllowXperm | RuleKind::AuditAllowXperm | RuleKind::DontAuditXperm => {
                    let extended_permissions = ExtendedPermissions::parse(cursor)?;
                    xperm_rules.push(XpermRule { key, kind, extended_permissions, enabled });
                    rule_order.push(kind);
                }
                RuleKind::TypeTransition | RuleKind::TypeChange | RuleKind::TypeMember => {
                    let new_type = TypeId::parse(cursor)?;
                    type_rules.push(TypeRule { key, kind, new_type, enabled });
                    rule_order.push(kind);
                }
                RuleKind::Allow | RuleKind::AuditAllow | RuleKind::DontAudit => {
                    let access_vector = AccessVector::parse(cursor)?;
                    av_rules.push(AccessRule { key, kind, access_vector, enabled });
                    rule_order.push(kind);
                }
            }
        }

        let av_rules = av_rules.into_boxed_slice();
        let type_rules = type_rules.into_boxed_slice();
        let xperm_rules = xperm_rules.into_boxed_slice();
        let rule_order = rule_order.into_boxed_slice();

        Ok(Self { av_rules, type_rules, xperm_rules, rule_order })
    }
}

impl Serialize for AccessVectorRules {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let count = self.rule_order.len() as u32;
        count.serialize(writer)?;

        let mut av_rules = self.av_rules.iter();
        let mut type_rules = self.type_rules.iter();
        let mut xperm_rules = self.xperm_rules.iter();

        for &kind in self.rule_order.iter() {
            match kind {
                RuleKind::Allow | RuleKind::AuditAllow | RuleKind::DontAudit => {
                    let rule = av_rules.next().unwrap();
                    rule.key.to_header(kind, rule.enabled).serialize(writer)?;
                    let val: u32 = rule.access_vector.into();
                    val.serialize(writer)?;
                }
                RuleKind::TypeTransition | RuleKind::TypeChange | RuleKind::TypeMember => {
                    let rule = type_rules.next().unwrap();
                    rule.key.to_header(kind, rule.enabled).serialize(writer)?;
                    rule.new_type.serialize(writer)?;
                }
                RuleKind::AllowXperm | RuleKind::AuditAllowXperm | RuleKind::DontAuditXperm => {
                    let rule = xperm_rules.next().unwrap();
                    rule.key.to_header(kind, rule.enabled).serialize(writer)?;
                    rule.extended_permissions.serialize(writer)?;
                }
            }
        }
        Ok(())
    }
}

impl Validate for AccessVectorRules {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        self.av_rules.validate(policy)?;
        self.type_rules.validate(policy)?;
        self.xperm_rules.validate(policy)?;
        Ok(())
    }
}

/// Global access vector rules wrapper that indexes rules by source, target, and class.
///
/// Binary policies guarantee that rules for the same key in the global table are contiguous.
/// Three corresponding [`HashTable`]s map [`RuleKey`]s to the index of the **first** matching rule.
/// Lookups return an iterator that starts at that index and yields rules until the key changes.
#[derive(Debug)]
pub struct IndexedAccessVectorRules {
    rules: AccessVectorRules,
    av_table: HashTable<U24Index>,
    type_transition_table: HashTable<U24Index>,
    xperms_table: HashTable<U24Index>,
    hasher: RapidBuildHasher,
}

impl IndexedAccessVectorRules {
    /// Builds an index over the specified access vector rules.
    ///
    /// Returns [`ParseError::DuplicateAccessVectorRule`] if non-contiguous rules share the same key.
    pub fn new(rules: AccessVectorRules) -> Result<Self, ParseError> {
        let hasher = RapidBuildHasher::default();
        let av_table = build_index(&rules.av_rules, &hasher)?;
        let type_transition_table = build_index(&rules.type_rules, &hasher)?;
        let xperms_table = build_index(&rules.xperm_rules, &hasher)?;

        Ok(Self { rules, av_table, type_transition_table, xperms_table, hasher })
    }

    /// Returns a reference to the underlying unindexed access vector rules.
    pub fn rules(&self) -> &AccessVectorRules {
        &self.rules
    }

    fn find_rules<'a, R: HasRuleKey>(
        table: &HashTable<U24Index>,
        rules: &'a [R],
        key: RuleKey,
        hasher: &RapidBuildHasher,
    ) -> impl Iterator<Item = &'a R> {
        let hash = key.hash(hasher);
        let slice = match table.find(hash, |&i| rules[usize::from(i)].key() == key) {
            Some(&i) => &rules[usize::from(i)..],
            None => &[],
        };
        slice.iter().take_while(move |rule| rule.key() == key)
    }

    /// Returns an iterator yielding matching standard access vector rules for the specified tuple.
    pub fn find_av_rules(
        &self,
        source: TypeId,
        target: TypeId,
        class: ClassId,
    ) -> impl Iterator<Item = &AccessRule> {
        Self::find_rules(
            &self.av_table,
            &self.rules.av_rules,
            RuleKey::new(source, target, class),
            &self.hasher,
        )
    }

    /// Returns an iterator yielding matching type transition, change, or member rules for the specified tuple.
    pub fn find_type_rules(
        &self,
        source: TypeId,
        target: TypeId,
        class: ClassId,
    ) -> impl Iterator<Item = &TypeRule> {
        Self::find_rules(
            &self.type_transition_table,
            &self.rules.type_rules,
            RuleKey::new(source, target, class),
            &self.hasher,
        )
    }

    /// Returns an iterator yielding matching extended permission rules for the specified tuple.
    pub fn find_xperm_rules(
        &self,
        source: TypeId,
        target: TypeId,
        class: ClassId,
    ) -> impl Iterator<Item = &XpermRule> {
        Self::find_rules(
            &self.xperms_table,
            &self.rules.xperm_rules,
            RuleKey::new(source, target, class),
            &self.hasher,
        )
    }
}

impl Parse for IndexedAccessVectorRules {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let rules = AccessVectorRules::parse(cursor)?;
        Self::new(rules)
    }
}

impl Serialize for IndexedAccessVectorRules {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        self.rules.serialize(writer)
    }
}

impl Validate for IndexedAccessVectorRules {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        self.rules.validate(policy)
    }
}

/// Builds a [`HashTable<U24Index>`] mapping [`RuleKey`] hashes to the index of the first rule in each contiguous run.
///
/// Binary SELinux policies index rules into buckets by `(source, target, class)`, within which rules are sorted by
/// `(source, target, class, rule_kind)`. Therefore, all rules sharing a given [`RuleKey`] are guaranteed to be contiguous
/// and well-ordered.
///
/// Returns [`ParseError::DuplicateAccessVectorRule`] if non-contiguous rules share the same key.
fn build_index<R: HasRuleKey>(
    rules: &[R],
    hasher: &RapidBuildHasher,
) -> Result<HashTable<U24Index>, ParseError> {
    let mut table = HashTable::new();
    let mut offset = 0;

    for chunk in rules.chunk_by(|a, b| a.key() == b.key()) {
        let key = chunk[0].key();
        let u24_idx = offset.try_into()?;
        let hash = key.hash(hasher);

        let Entry::Vacant(entry) = table.entry(
            hash,
            |&i| rules[usize::from(i)].key() == key,
            |&i| rules[usize::from(i)].key().hash(hasher),
        ) else {
            return Err(ParseError::DuplicateAccessVectorRule { key, kind: chunk[0].kind() });
        };
        entry.insert(u24_idx);
        offset += chunk.len();
    }

    Ok(table)
}

/// Expression element kind bit for boolean variable operands.
pub const COND_EXPR_BOOL: u32 = 1;
/// Expression element kind bit for unary NOT operator.
pub const COND_EXPR_NOT: u32 = 2;
/// Expression element kind bit for binary OR operator.
pub const COND_EXPR_OR: u32 = 3;
/// Expression element kind bit for binary AND operator.
pub const COND_EXPR_AND: u32 = 4;
/// Expression element kind bit for binary EQUALS operator.
pub const COND_EXPR_EQ: u32 = 5;
/// Expression element kind bit for binary NOT-EQUALS operator.
pub const COND_EXPR_NEQ: u32 = 6;

/// Individual element in a conditional boolean expression sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ConditionalExpressionElement {
    /// Conditional boolean variable operand.
    Boolean(ConditionalBooleanId),
    /// Unary boolean NOT operator.
    Not,
    /// Binary boolean OR operator.
    Or,
    /// Binary boolean AND operator.
    And,
    /// Binary boolean EQUALS operator.
    Equals,
    /// Binary boolean NOT-EQUALS operator.
    NotEquals,
}

impl Parse for ConditionalExpressionElement {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let expr_type = u32::parse(cursor)?;
        let boolean_id = u32::parse(cursor)?;
        match expr_type {
            COND_EXPR_BOOL => {
                let id = ConditionalBooleanId::from_u32(boolean_id)
                    .ok_or(ParseError::InvalidId { value: boolean_id })?;
                Ok(Self::Boolean(id))
            }
            COND_EXPR_NOT => Ok(Self::Not),
            COND_EXPR_OR => Ok(Self::Or),
            COND_EXPR_AND => Ok(Self::And),
            COND_EXPR_EQ => Ok(Self::Equals),
            COND_EXPR_NEQ => Ok(Self::NotEquals),
            invalid => Err(ParseError::InvalidEnumValue {
                enum_name: "ConditionalExpressionElement",
                value: invalid as u64,
            }),
        }
    }
}

impl Serialize for ConditionalExpressionElement {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let (expr_type, boolean_id) = match self {
            Self::Boolean(id) => (COND_EXPR_BOOL, id.as_u32()),
            Self::Not => (COND_EXPR_NOT, 0),
            Self::Or => (COND_EXPR_OR, 0),
            Self::And => (COND_EXPR_AND, 0),
            Self::Equals => (COND_EXPR_EQ, 0),
            Self::NotEquals => (COND_EXPR_NEQ, 0),
        };
        expr_type.serialize(writer)?;
        boolean_id.serialize(writer)?;
        Ok(())
    }
}

impl Validate for ConditionalExpressionElement {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        if let Self::Boolean(id) = self {
            id.validate(policy)?;
        }
        Ok(())
    }
}

/// Parsed SELinux conditional node containing expression AST and true/false branch rule sets.
#[derive(Clone, Debug, Parse, Serialize, Validate)]
pub struct ConditionalNode {
    state: u32,
    expression_elements: Array<ConditionalExpressionElement>,
    true_rules: AccessVectorRules,
    false_rules: AccessVectorRules,
}

/// On-wire header identifying the source, target, class, and rule flags of an access vector rule.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Parse, Serialize)]
struct BinaryAccessVectorRuleHeader {
    source_type: u16,
    target_type: u16,
    class: u16,
    rule_flags: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::NewPolicy;
    use crate::new_policy::metadata::PolicyVersion;
    use crate::new_policy::parser::PolicyWriter;
    use crate::new_policy::traits::{HasPolicyId, PolicyId};

    #[test]
    fn test_xperms_bitmap_ops_and_contains() {
        let mut bitmap1 = XpermsBitmap::NONE;
        let mut bitmap2 = XpermsBitmap::NONE;
        assert!(!bitmap1.contains(10));
        assert!(!bitmap2.contains(10));

        bitmap1.0[0] = 1 << 10;
        bitmap2.0[0] = 1 << 20;
        assert!(bitmap1.contains(10));
        assert!(!bitmap1.contains(20));
        assert!(bitmap2.contains(20));

        let or_bitmap = bitmap1 | bitmap2;
        assert!(or_bitmap.contains(10));
        assert!(or_bitmap.contains(20));

        let and_bitmap = or_bitmap & bitmap1;
        assert!(and_bitmap.contains(10));
        assert!(!and_bitmap.contains(20));

        let sub_bitmap = or_bitmap - bitmap1;
        assert!(!sub_bitmap.contains(10));
        assert!(sub_bitmap.contains(20));

        let not_bitmap = !XpermsBitmap::NONE;
        assert_eq!(not_bitmap, XpermsBitmap::ALL);
        assert!(not_bitmap.contains(0));
        assert!(not_bitmap.contains(255));
    }

    #[test]
    fn test_av_rule_parse_and_serialize() {
        let data = [
            1, 0, 0, 0, // count = 1
            1, 0, // source_type = 1
            2, 0, // target_type = 2
            3, 0, // class = 3
            1, 0, // rule_type = 1 (ALLOW)
            5, 0, 0, 0, // access_vector = 5
        ];
        let mut cursor = PolicyCursor::new(&data);
        let av_rules = AccessVectorRules::parse(&mut cursor).expect("parse rules table");
        assert_eq!(av_rules.av_rules.len(), 1);
        assert_eq!(av_rules.av_rules[0].key.source_type, TypeId::from_u32(1).unwrap());
        assert_eq!(av_rules.av_rules[0].key.target_type, TypeId::from_u32(2).unwrap());
        assert_eq!(av_rules.av_rules[0].key.class, ClassId::from_u32(3).unwrap());
        assert_eq!(av_rules.av_rules[0].access_vector, AccessVector::from(5));

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        av_rules.serialize(&mut policy_writer).expect("serialize rules table");
        assert_eq!(writer.as_slice(), &data);
    }

    #[test]
    fn test_av_rule_type_transition_parse_and_serialize() {
        let data = [
            1, 0, 0, 0, // count = 1
            1, 0, // source_type = 1
            2, 0, // target_type = 2
            3, 0, // class = 3
            16, 0, // rule_type = 16 (TYPE_TRANSITION)
            10, 0, 0, 0, // new_type = 10
        ];
        let mut cursor = PolicyCursor::new(&data);
        let av_rules = AccessVectorRules::parse(&mut cursor).expect("parse rules table");
        assert_eq!(av_rules.type_rules.len(), 1);
        assert_eq!(av_rules.type_rules[0].key.source_type, TypeId::from_u32(1).unwrap());
        assert_eq!(av_rules.type_rules[0].key.target_type, TypeId::from_u32(2).unwrap());
        assert_eq!(av_rules.type_rules[0].key.class, ClassId::from_u32(3).unwrap());
        assert_eq!(av_rules.type_rules[0].new_type, TypeId::from_u32(10).unwrap());

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        av_rules.serialize(&mut policy_writer).expect("serialize rules table");
        assert_eq!(writer.as_slice(), &data);
    }

    #[test]
    fn test_av_rule_xperm_parse_and_serialize() {
        let mut data = vec![
            1, 0, 0, 0, // count = 1
            1, 0, // source_type = 1
            2, 0, // target_type = 2
            3, 0, // class = 3
            0, 1, // rule_type = 0x0100 (ALLOWXPERM)
            1, // xperms_type = 1 (XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES)
            0, // xperms_optional_prefix = 0
        ];
        data.extend_from_slice(&[
            1, 0, 0, 0, 0, 0, 0, 0, // word 0 = 1
            0, 0, 0, 0, 0, 0, 0, 0, // word 1 = 0
            0, 0, 0, 0, 0, 0, 0, 0, // word 2 = 0
            0, 0, 0, 0, 0, 0, 0, 0, // word 3 = 0
        ]);
        let mut cursor = PolicyCursor::new(&data);
        let av_rules = AccessVectorRules::parse(&mut cursor).expect("parse rules table");
        assert_eq!(av_rules.xperm_rules.len(), 1);
        let xp = &av_rules.xperm_rules[0].extended_permissions;
        assert_eq!(xp.xperms_type, XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES);
        assert!(xp.xperms_bitmap.contains(0));
        assert!(!xp.xperms_bitmap.contains(1));

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        av_rules.serialize(&mut policy_writer).expect("serialize rules table");
        assert_eq!(writer.as_slice(), &data);
    }

    #[test]
    fn test_access_vector_rules_indexing_and_decisions() {
        let data = [
            2, 0, 0, 0, // count = 2 rules
            // Rule 1: ALLOW (source 1, target 2, class 3)
            1, 0, // source = 1
            2, 0, // target = 2
            3, 0, // class = 3
            1, 0, // rule_type = 1 (ALLOW)
            7, 0, 0, 0, // access_vector = 7
            // Rule 2: TYPE_TRANSITION (source 1, target 2, class 3 -> new_type 9)
            1, 0, // source = 1
            2, 0, // target = 2
            3, 0, // class = 3
            16, 0, // rule_type = 16 (TYPE_TRANSITION)
            9, 0, 0, 0, // new_type = 9
        ];

        let mut cursor = PolicyCursor::new(&data);
        let av_rules = IndexedAccessVectorRules::parse(&mut cursor).expect("parse rules table");

        let s1 = TypeId::from_u32(1).unwrap();
        let t2 = TypeId::from_u32(2).unwrap();
        let c3 = ClassId::from_u32(3).unwrap();

        let av_rules_list: Vec<_> = av_rules.find_av_rules(s1, t2, c3).collect();
        assert_eq!(av_rules_list.len(), 1);
        assert_eq!(av_rules_list[0].kind(), RuleKind::Allow);
        assert_eq!(av_rules_list[0].access_vector(), AccessVector::from(7));

        let type_rules_list: Vec<_> = av_rules.find_type_rules(s1, t2, c3).collect();
        assert_eq!(type_rules_list.len(), 1);
        assert_eq!(type_rules_list[0].kind(), RuleKind::TypeTransition);
        assert_eq!(type_rules_list[0].new_type(), TypeId::from_u32(9).unwrap());
    }

    #[test]
    fn test_unindexed_access_vector_rules_allows_duplicate_keys() {
        let data = [
            3, 0, 0, 0, // count = 3 rules
            // Rule 1: ALLOW (source 1, target 2, class 3)
            1, 0, 2, 0, 3, 0, 1, 0, 7, 0, 0, 0, // access_vector = 7
            // Rule 2: ALLOW (source 4, target 5, class 6)
            4, 0, 5, 0, 6, 0, 1, 0, 8, 0, 0, 0, // access_vector = 8
            // Rule 3: ALLOW (source 1, target 2, class 3) -- non-consecutive duplicate key
            1, 0, 2, 0, 3, 0, 1, 0, 9, 0, 0, 0, // access_vector = 9
        ];

        let mut cursor = PolicyCursor::new(&data);
        let av_rules =
            AccessVectorRules::parse(&mut cursor).expect("unindexed rules parse duplicate keys");
        assert_eq!(av_rules.av_rules.len(), 3);

        let mut cursor = PolicyCursor::new(&data);
        let err = IndexedAccessVectorRules::parse(&mut cursor)
            .expect_err("indexed rules reject duplicate keys");
        assert!(matches!(
            err,
            ParseError::DuplicateAccessVectorRule { key: _, kind: RuleKind::Allow }
        ));
    }

    #[test]
    fn test_conditional_nodes() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/conditional_policy");
        let new_policy = NewPolicy::parse(policy_bytes).expect("parse conditional policy");
        new_policy.validate().expect("validate conditional policy");

        let nodes = new_policy.conditional_nodes();
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[0].true_rules.av_rules().len(), 1);
        assert_eq!(nodes[0].false_rules.av_rules().len(), 0);
        assert_eq!(nodes[1].true_rules.av_rules().len(), 1);
        assert_eq!(nodes[1].false_rules.av_rules().len(), 1);
        assert_eq!(nodes[2].true_rules.av_rules().len(), 2);
        assert_eq!(nodes[2].false_rules.av_rules().len(), 0);
        assert_eq!(nodes[3].true_rules.av_rules().len(), 1);
        assert_eq!(nodes[3].false_rules.av_rules().len(), 0);
    }

    #[test]
    fn parse_allowxperm_one_ioctl() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = NewPolicy::parse(policy_bytes).expect("parse policy");
        policy.validate().expect("validate policy");

        let class_id =
            policy.classes().get_by_name(b"class_one_ioctl").expect("look up class_one_ioctl").id();
        let type0 = policy.types().get_by_name(b"type0").expect("look up type0").id();
        let rules: Vec<_> = policy
            .access_vector_rules()
            .find_xperm_rules(type0, type0, class_id)
            .filter(|r| r.kind() == RuleKind::AllowXperm)
            .map(|r| r.extended_permissions())
            .collect();

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].count(), 1);
        assert!(rules[0].contains(0xabcd));
    }

    // `ioctl` extended permissions that are declared in the same rule, and have the same
    // high byte, are stored in the same `AccessVectorRule` in the compiled policy.
    #[test]
    fn parse_allowxperm_two_ioctls_same_range_coalesced() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = NewPolicy::parse(policy_bytes).expect("parse policy");
        policy.validate().expect("validate policy");

        let class_id = policy
            .classes()
            .get_by_name(b"class_two_ioctls_same_range")
            .expect("look up class_two_ioctls_same_range")
            .id();
        let type0 = policy.types().get_by_name(b"type0").expect("look up type0").id();
        let rules: Vec<_> = policy
            .access_vector_rules()
            .find_xperm_rules(type0, type0, class_id)
            .filter(|r| r.kind() == RuleKind::AllowXperm)
            .map(|r| r.extended_permissions())
            .collect();

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].xperms_type(), XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES);
        assert_eq!(rules[0].xperms_optional_prefix(), 0x12);
        assert_eq!(rules[0].count(), 2);
        assert!(rules[0].contains(0x1234));
        assert!(rules[0].contains(0x1256));
    }

    #[test]
    fn parse_allowxperm_one_driver_range() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = NewPolicy::parse(policy_bytes).expect("parse policy");
        policy.validate().expect("validate policy");

        let class_id = policy
            .classes()
            .get_by_name(b"class_one_driver_range")
            .expect("look up class_one_driver_range")
            .id();
        let type0 = policy.types().get_by_name(b"type0").expect("look up type0").id();
        let rules: Vec<_> = policy
            .access_vector_rules()
            .find_xperm_rules(type0, type0, class_id)
            .filter(|r| r.kind() == RuleKind::AllowXperm)
            .map(|r| r.extended_permissions())
            .collect();

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].xperms_type(), XPERMS_TYPE_IOCTL_PREFIXES);
        assert_eq!(rules[0].count(), 0x100);
        assert!(rules[0].contains(0x1000));
        assert!(rules[0].contains(0x10ab));
    }

    #[test]
    fn parse_allowxperm_two_ioctls_different_range() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = NewPolicy::parse(policy_bytes).expect("parse policy");
        policy.validate().expect("validate policy");

        let class_id = policy
            .classes()
            .get_by_name(b"class_two_ioctls_diff_range")
            .expect("look up class_two_ioctls_diff_range")
            .id();
        let type0 = policy.types().get_by_name(b"type0").expect("look up type0").id();
        let rules: Vec<_> = policy
            .access_vector_rules()
            .find_xperm_rules(type0, type0, class_id)
            .filter(|r| r.kind() == RuleKind::AllowXperm)
            .map(|r| r.extended_permissions())
            .collect();

        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].xperms_type(), XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES);
        assert_eq!(rules[0].xperms_optional_prefix(), 0x56);
        assert_eq!(rules[0].count(), 1);
        assert!(rules[0].contains(0x5678));
        assert_eq!(rules[1].xperms_type(), XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES);
        assert_eq!(rules[1].xperms_optional_prefix(), 0x12);
        assert_eq!(rules[1].count(), 1);
        assert!(rules[1].contains(0x1234));
    }

    #[test]
    fn parse_allowxperm_one_nlmsg() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = NewPolicy::parse(policy_bytes).expect("parse policy");
        policy.validate().expect("validate policy");

        let class_id =
            policy.classes().get_by_name(b"class_one_nlmsg").expect("look up class_one_nlmsg").id();
        let type0 = policy.types().get_by_name(b"type0").expect("look up type0").id();
        let rules: Vec<_> = policy
            .access_vector_rules()
            .find_xperm_rules(type0, type0, class_id)
            .filter(|r| r.kind() == RuleKind::AllowXperm)
            .map(|r| r.extended_permissions())
            .collect();

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].xperms_type(), XPERMS_TYPE_NLMSG);
        assert_eq!(rules[0].count(), 1);
        assert!(rules[0].contains(0x12));
    }

    // If an allowxperm rule and an auditallowxperm rule specify exactly the same permissions, they
    // are not coalesced into a single `AccessVectorRule` in the policy; two rules appear in the
    // policy.
    #[test]
    fn parse_auditallowxperm_not_coalesced() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = NewPolicy::parse(policy_bytes).expect("parse policy");
        policy.validate().expect("validate policy");

        let class_id = policy
            .classes()
            .get_by_name(b"class_auditallowxperm_not_coalesced")
            .expect("look up class_auditallowxperm_not_coalesced")
            .id();
        let type0 = policy.types().get_by_name(b"type0").expect("look up type0").id();
        let allow_rules: Vec<_> = policy
            .access_vector_rules()
            .find_xperm_rules(type0, type0, class_id)
            .filter(|r| r.kind() == RuleKind::AllowXperm)
            .map(|r| r.extended_permissions())
            .collect();
        let auditallow_rules: Vec<_> = policy
            .access_vector_rules()
            .find_xperm_rules(type0, type0, class_id)
            .filter(|r| r.kind() == RuleKind::AuditAllowXperm)
            .map(|r| r.extended_permissions())
            .collect();

        assert_eq!(allow_rules.len(), 1);
        assert_eq!(allow_rules[0].count(), 1);
        assert!(allow_rules[0].contains(0xabcd));
        assert_eq!(auditallow_rules.len(), 1);
        assert_eq!(auditallow_rules[0].count(), 1);
        assert!(auditallow_rules[0].contains(0xabcd));
    }

    #[test]
    fn parse_dontauditxperm() {
        let policy_bytes = include_bytes!("../../testdata/micro_policies/allowxperm_policy");
        let policy = NewPolicy::parse(policy_bytes).expect("parse policy");
        policy.validate().expect("validate policy");

        let class_id = policy
            .classes()
            .get_by_name(b"class_dontauditxperm")
            .expect("look up class_dontauditxperm")
            .id();
        let type0 = policy.types().get_by_name(b"type0").expect("look up type0").id();
        let rules: Vec<_> = policy
            .access_vector_rules()
            .find_xperm_rules(type0, type0, class_id)
            .filter(|r| r.kind() == RuleKind::DontAuditXperm)
            .map(|r| r.extended_permissions())
            .collect();

        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].xperms_type(), XPERMS_TYPE_NLMSG);
        assert_eq!(rules[0].xperms_optional_prefix(), 0x00);
        assert_eq!(rules[0].count(), 1);
        assert!(rules[0].contains(0x11));
        assert_eq!(rules[1].xperms_type(), XPERMS_TYPE_IOCTL_PREFIX_AND_POSTFIXES);
        assert_eq!(rules[1].xperms_optional_prefix(), 0x10);
        assert_eq!(rules[1].count(), 1);
        assert!(rules[1].contains(0x1000));
    }
}
