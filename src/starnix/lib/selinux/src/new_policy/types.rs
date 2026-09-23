// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZeroU16;
use std::ops::Index;

use hashbrown::HashTable;
use hashbrown::hash_table::Entry;
use selinux_policy_derive::{HasPolicyId, Parse, Serialize, Validate};

use super::bitmap::IdSet;
use super::error::{ParseError, SerializeError, ValidateError};
use super::id_type::IdType;
use super::indexed::hash_name;
use super::parser::{Array, PolicyCursor, PolicyWriter};
use super::traits::{HasName, Parse, PolicyId, Serialize, Validate};
use super::{NewPolicy, U24Index};

/// Tag type for type safety of policy type identifiers.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct TypeTag;

/// Identifies a type (or type attribute) within a policy.
pub type TypeId = IdType<NonZeroU16, TypeTag>;

/// Set of types that are marked permissive.
pub type PermissiveTypeSet = IdSet<TypeId, true>;

/// Set of [`TypeId`]s.
pub type TypeSet = IdSet<TypeId>;

impl Validate for TypeId {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        policy
            .types()
            .get_by_id(*self)
            .map(|_| ())
            .ok_or_else(|| ValidateError::UnknownId { kind: "type", id: self.as_u32() })
    }
}

/// Classification of a type symbol (alias, primary type, or attribute).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Parse, Serialize, Validate)]
#[policy(wire_type = u32)]
pub enum TypeKind {
    Alias = 0,
    Type = 1,
    Attribute = 3,
}

/// Parsed SELinux type, containing an ID, a name, properties, and optional bounds.
#[derive(Debug, Validate, HasPolicyId)]
pub struct Type {
    id: TypeId,
    name: Box<[u8]>,
    properties: TypeKind,
    bounds: Option<TypeId>,
}

impl HasName for Type {
    fn name(&self) -> &[u8] {
        &self.name
    }
}

impl Type {
    pub fn bounded_by(&self) -> Option<TypeId> {
        self.bounds
    }
}

#[derive(Parse, Serialize)]
struct BinaryTypeMetadata {
    length: u32,
    id: TypeId,
    properties: TypeKind,
    bounds: Option<TypeId>,
}

impl Parse for Type {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let metadata = BinaryTypeMetadata::parse(cursor)?;
        let name = cursor.read_bytes(metadata.length as usize)?.to_vec().into_boxed_slice();

        Ok(Self { id: metadata.id, name, properties: metadata.properties, bounds: metadata.bounds })
    }
}

impl Serialize for Type {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let metadata = BinaryTypeMetadata {
            length: self.name.len() as u32,
            id: self.id,
            properties: self.properties,
            bounds: self.bounds,
        };
        metadata.serialize(writer)?;
        writer.write_bytes(&self.name);
        Ok(())
    }
}

/// Container for all types in the policy, providing indices for fast lookup by ID and Name.
#[derive(Debug)]
pub struct Types {
    primary_names_count: u32,
    /// In-order list of all types, attributes, and aliases.
    ordered: Array<Type>,

    /// Maps TypeId -> index in `ordered`. Only contains Types and Attributes.
    /// Index is `TypeId - 1`.
    by_id: Box<[Option<U24Index>]>,

    /// Maps name -> index in `ordered`. Only contains Types and Aliases.
    by_name: HashTable<U24Index>,
    hasher: rapidhash::RapidBuildHasher,
}

impl Parse for Types {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let primary_names_count = u32::parse(cursor)?;
        cursor.set_types_count(primary_names_count);
        let ordered = Array::<Type>::parse(cursor)?;

        // Build indices
        let mut by_id = Vec::with_capacity(ordered.len());
        let hasher = rapidhash::RapidBuildHasher::default();
        let mut by_name = HashTable::with_capacity(ordered.len());

        for (index, item) in ordered.iter().enumerate() {
            let u24_idx: U24Index = index.try_into()?;
            if item.properties == TypeKind::Type || item.properties == TypeKind::Attribute {
                let id = item.id.as_u32() as usize;
                if id > by_id.len() {
                    by_id.resize(id, None);
                } else if by_id[id - 1].is_some() {
                    return Err(ParseError::DuplicateId { id: item.id.as_u32() });
                }
                by_id[id - 1] = Some(u24_idx);
            }
            if item.properties == TypeKind::Type || item.properties == TypeKind::Alias {
                let name = item.name.as_ref();
                let hash = hash_name(&hasher, name);
                let Entry::Vacant(entry) = by_name.entry(
                    hash,
                    |&idx| ordered[usize::from(idx)].name.as_ref() == name,
                    |&idx| hash_name(&hasher, ordered[usize::from(idx)].name.as_ref()),
                ) else {
                    return Err(ParseError::DuplicateName { name: name.into() });
                };
                entry.insert(u24_idx);
            }
        }

        by_id.shrink_to_fit();
        by_name.shrink_to_fit(|&idx| hash_name(&hasher, ordered[usize::from(idx)].name.as_ref()));

        Ok(Self { primary_names_count, ordered, by_id: by_id.into_boxed_slice(), by_name, hasher })
    }
}

impl Serialize for Types {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        self.primary_names_count.serialize(writer)?;
        self.ordered.serialize(writer)
    }
}

impl Validate for Types {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        self.ordered.validate(policy)
    }
}

impl Types {
    pub fn primary_names_count(&self) -> u32 {
        self.primary_names_count
    }

    pub fn get_by_id(&self, id: TypeId) -> Option<&Type> {
        let index = self.by_id.get((id.as_u32() - 1) as usize)?.as_ref()?;
        Some(&self.ordered[*index])
    }

    pub fn get_by_name(&self, name: &[u8]) -> Option<&Type> {
        let hash = hash_name(&self.hasher, name);
        let idx = self.by_name.find(hash, |&idx| self.ordered[idx].name.as_ref() == name)?;
        Some(&self.ordered[*idx])
    }

    pub fn is_empty(&self) -> bool {
        self.ordered.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Type> {
        self.ordered.iter()
    }
}

/// Type-to-attribute mappings for each primary type in the policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Validate)]
pub struct TypeAttributeMaps {
    maps: Box<[TypeSet]>,
}

impl TypeAttributeMaps {
    /// Returns the attribute [`TypeSet`] for the specified `type_id`.
    pub fn get(&self, type_id: TypeId) -> Option<&TypeSet> {
        self.maps.get((type_id.as_u32() - 1) as usize)
    }
}

impl Index<TypeId> for TypeAttributeMaps {
    type Output = TypeSet;

    fn index(&self, id: TypeId) -> &Self::Output {
        &self.maps[(id.as_u32() - 1) as usize]
    }
}

impl Parse for TypeAttributeMaps {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let count = cursor.types_count() as usize;
        let mut maps = Vec::with_capacity(count);
        for _ in 0..count {
            maps.push(TypeSet::parse(cursor)?);
        }
        Ok(Self { maps: maps.into_boxed_slice() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::metadata::PolicyVersion;

    #[test]
    fn test_types_lookup() {
        let mut bytes = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut bytes);
        2u32.serialize(&mut policy_writer).unwrap(); // primary_names_count
        2u32.serialize(&mut policy_writer).unwrap(); // ordered count = 2

        let t1 = Type {
            id: TypeId::new(NonZeroU16::new(1).unwrap()),
            name: Box::from(b"foo".as_slice()),
            properties: TypeKind::Type,
            bounds: None,
        };
        t1.serialize(&mut policy_writer).unwrap();

        let t2 = Type {
            id: TypeId::new(NonZeroU16::new(2).unwrap()),
            name: Box::from(b"bar".as_slice()),
            properties: TypeKind::Type,
            bounds: None,
        };
        t2.serialize(&mut policy_writer).unwrap();

        let mut cursor = PolicyCursor::new(&bytes);
        let types = Types::parse(&mut cursor).expect("parse types");

        assert_eq!(
            types.get_by_id(TypeId::new(NonZeroU16::new(1).unwrap())).map(|t| t.name()),
            Some(b"foo".as_slice())
        );
        assert_eq!(
            types.get_by_id(TypeId::new(NonZeroU16::new(2).unwrap())).map(|t| t.name()),
            Some(b"bar".as_slice())
        );
        assert!(types.get_by_id(TypeId::new(NonZeroU16::new(3).unwrap())).is_none());

        assert_eq!(
            types.get_by_name(b"foo").map(|t| t.id),
            Some(TypeId::new(NonZeroU16::new(1).unwrap()))
        );
        assert_eq!(
            types.get_by_name(b"bar").map(|t| t.id),
            Some(TypeId::new(NonZeroU16::new(2).unwrap()))
        );
        assert!(types.get_by_name(b"baz").is_none());
    }

    #[test]
    fn test_types_duplicate_id() {
        let mut bytes = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut bytes);
        2u32.serialize(&mut policy_writer).unwrap(); // primary_names_count
        2u32.serialize(&mut policy_writer).unwrap(); // ordered count = 2

        let t1 = Type {
            id: TypeId::new(NonZeroU16::new(1).unwrap()),
            name: Box::from(b"foo".as_slice()),
            properties: TypeKind::Type,
            bounds: None,
        };
        t1.serialize(&mut policy_writer).unwrap();

        let t2 = Type {
            id: TypeId::new(NonZeroU16::new(1).unwrap()),
            name: Box::from(b"bar".as_slice()),
            properties: TypeKind::Type,
            bounds: None,
        };
        t2.serialize(&mut policy_writer).unwrap();

        let mut cursor = PolicyCursor::new(&bytes);
        let result = Types::parse(&mut cursor);
        assert!(matches!(result, Err(ParseError::DuplicateId { id: 1 })));
    }

    #[test]
    fn test_types_duplicate_name() {
        let mut bytes = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut bytes);
        2u32.serialize(&mut policy_writer).unwrap(); // primary_names_count
        2u32.serialize(&mut policy_writer).unwrap(); // ordered count = 2

        let t1 = Type {
            id: TypeId::new(NonZeroU16::new(1).unwrap()),
            name: Box::from(b"foo".as_slice()),
            properties: TypeKind::Type,
            bounds: None,
        };
        t1.serialize(&mut policy_writer).unwrap();

        let t2 = Type {
            id: TypeId::new(NonZeroU16::new(2).unwrap()),
            name: Box::from(b"foo".as_slice()),
            properties: TypeKind::Type,
            bounds: None,
        };
        t2.serialize(&mut policy_writer).unwrap();

        let mut cursor = PolicyCursor::new(&bytes);
        let result = Types::parse(&mut cursor);
        assert!(matches!(result, Err(ParseError::DuplicateName { name }) if name == b"foo"));
    }

    #[test]
    fn test_types_duplicate_alias_name() {
        let mut bytes = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut bytes);
        1u32.serialize(&mut policy_writer).unwrap(); // primary_names_count
        2u32.serialize(&mut policy_writer).unwrap(); // ordered count = 2

        let t1 = Type {
            id: TypeId::new(NonZeroU16::new(1).unwrap()),
            name: Box::from(b"foo".as_slice()),
            properties: TypeKind::Type,
            bounds: None,
        };
        t1.serialize(&mut policy_writer).unwrap();

        let t2 = Type {
            id: TypeId::new(NonZeroU16::new(1).unwrap()),
            name: Box::from(b"foo".as_slice()),
            properties: TypeKind::Alias,
            bounds: None,
        };
        t2.serialize(&mut policy_writer).unwrap();

        let mut cursor = PolicyCursor::new(&bytes);
        let result = Types::parse(&mut cursor);
        assert!(matches!(result, Err(ParseError::DuplicateName { name }) if name == b"foo"));
    }
}
