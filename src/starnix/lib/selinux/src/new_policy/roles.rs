// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZeroU16;

use selinux_policy_derive::{HasName, HasPolicyId, Parse, Serialize, Validate};

use super::bitmap::IdSet;
use super::error::{ParseError, SerializeError, ValidateError};
use super::id_type::IdType;
use super::parser::{PolicyCursor, PolicyWriter};
use super::traits::{Parse, PolicyId, Serialize, Validate};
use super::{ClassId, NewPolicy, TypeId, TypeSet};

/// Tag type for type safety of policy role identifiers.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct RoleTag;

/// Identifies a role within a policy.
pub type RoleId = IdType<NonZeroU16, RoleTag>;

/// Set of [`RoleId`]s.
pub type RoleSet = IdSet<RoleId>;

#[derive(Parse, Serialize)]
struct BinaryRoleMetadata {
    key_length: u32,
    id: RoleId,
    bounds: Option<RoleId>,
}

/// Parsed SELinux [`Role`] definition.
#[derive(Debug, Validate, HasName, HasPolicyId)]
pub struct Role {
    id: RoleId,
    name: Box<[u8]>,
    bounds: Option<RoleId>,
    dominates: RoleSet,
    types: TypeSet,
}

impl Role {
    pub fn bounds(&self) -> Option<RoleId> {
        self.bounds
    }

    pub fn types(&self) -> &TypeSet {
        &self.types
    }
}

impl Parse for Role {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let metadata = BinaryRoleMetadata::parse(cursor)?;
        let name = Box::from(cursor.read_bytes(metadata.key_length as usize)?);
        let dominates = RoleSet::parse(cursor)?;
        let types = TypeSet::parse(cursor)?;

        Ok(Self { id: metadata.id, name, bounds: metadata.bounds, dominates, types })
    }
}

impl Serialize for Role {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let metadata = BinaryRoleMetadata {
            key_length: self.name.len() as u32,
            id: self.id,
            bounds: self.bounds,
        };
        metadata.serialize(writer)?;
        writer.write_bytes(&self.name);
        self.dominates.serialize(writer)?;
        self.types.serialize(writer)?;
        Ok(())
    }
}
impl Validate for RoleId {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        policy
            .roles()
            .get_by_id(*self)
            .map(|_| ())
            .ok_or_else(|| ValidateError::UnknownId { kind: "role", id: self.as_u32() })
    }
}

/// SELinux policy role transition rule (`role_transition`).
#[derive(Debug, Parse, Serialize, Validate)]
pub struct RoleTransition {
    current_role: RoleId,
    type_: TypeId,
    new_role: RoleId,
    class: ClassId,
}

impl RoleTransition {
    pub fn current_role(&self) -> RoleId {
        self.current_role
    }

    pub fn type_(&self) -> TypeId {
        self.type_
    }

    pub fn new_role(&self) -> RoleId {
        self.new_role
    }

    pub fn class(&self) -> ClassId {
        self.class
    }
}

/// SELinux policy role allow rule (`allow` for roles).
#[derive(Debug, Parse, Serialize, Validate)]
pub struct RoleAllow {
    source_role: RoleId,
    new_role: RoleId,
}

impl RoleAllow {
    pub fn source_role(&self) -> RoleId {
        self.source_role
    }

    pub fn new_role(&self) -> RoleId {
        self.new_role
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::metadata::PolicyVersion;
    use crate::new_policy::parser::PolicyWriter;
    use crate::new_policy::traits::{HasName, HasPolicyId, PolicyId};

    #[test]
    fn test_role_parse_and_serialize() {
        let data = [
            // BinaryRoleMetadata
            4, 0, 0, 0, // key_length = 4
            1, 0, 0, 0, // id = 1
            0, 0, 0, 0, // bounds = 0
            // name: "test"
            b't', b'e', b's', b't', // dominates (empty ExtensibleBitmap)
            64, 0, 0, 0, // map_item_size_bits = 64
            0, 0, 0, 0, // high_bit = 0
            0, 0, 0, 0, // items_count = 0
            // types (empty ExtensibleBitmap)
            64, 0, 0, 0, // map_item_size_bits = 64
            0, 0, 0, 0, // high_bit = 0
            0, 0, 0, 0, // items_count = 0
        ];
        let mut cursor = PolicyCursor::new(&data);
        let role = Role::parse(&mut cursor).unwrap();
        assert_eq!(role.id(), RoleId::from_u32(1).unwrap());
        assert_eq!(role.name(), b"test");
        assert!(role.bounds().is_none());
        // dominates and types are empty dynamically validated by round-trip

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        role.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, data);
    }

    #[test]
    fn test_role_transition_parse_and_serialize() {
        let data = [
            1, 0, 0, 0, // current_role = 1
            2, 0, 0, 0, // type_ = 2
            3, 0, 0, 0, // new_role = 3
            4, 0, 0, 0, // class = 4
        ];
        let mut cursor = PolicyCursor::new(&data);
        let trans = RoleTransition::parse(&mut cursor).unwrap();
        assert_eq!(trans.current_role(), RoleId::from_u32(1).unwrap());
        assert_eq!(trans.type_(), TypeId::from_u32(2).unwrap());
        assert_eq!(trans.new_role(), RoleId::from_u32(3).unwrap());
        assert_eq!(trans.class(), ClassId::from_u32(4).unwrap());

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        trans.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, data);
    }

    #[test]
    fn test_role_allow_parse_and_serialize() {
        let data = [
            1, 0, 0, 0, // source_role = 1
            2, 0, 0, 0, // new_role = 2
        ];
        let mut cursor = PolicyCursor::new(&data);
        let allow = RoleAllow::parse(&mut cursor).unwrap();
        assert_eq!(allow.source_role(), RoleId::from_u32(1).unwrap());
        assert_eq!(allow.new_role(), RoleId::from_u32(2).unwrap());

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        allow.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, data);
    }

    #[test]
    fn test_role_policy_elements() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/role_transition_policy");
        let new_policy = NewPolicy::parse(policy_bytes).expect("parse role_transition policy");
        new_policy.validate().expect("validate role_transition policy");

        assert_eq!(new_policy.role_transitions().len(), 2);
        assert_eq!(new_policy.role_allowlist().len(), 1);
    }
}
