// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZeroU16;

use selinux_policy_derive::{HasName, HasPolicyId, Parse, Serialize};

use super::NewPolicy;
use super::error::{ParseError, SerializeError, ValidateError};
use super::id_type::IdType;
use super::indexed::IdAndNameIndexed;
use super::parser::{PolicyCursor, PolicyWriter};
use super::permissions::Permission;
use super::traits::{Parse, PolicyId, Serialize, Validate};

/// Tag type for type safety of policy common symbol identifiers.
#[derive(Copy, Clone, Debug, Hash, Eq, Ord, PartialEq, PartialOrd)]
pub struct CommonSymbolTag;

/// Identifies a common symbol within a policy.
pub type CommonSymbolId = IdType<NonZeroU16, CommonSymbolTag>;

/// Parsed SELinux common symbol table entry (e.g. `common file { ... }`).
#[derive(Debug, HasName, HasPolicyId)]
pub struct CommonSymbol {
    id: CommonSymbolId,
    name: Box<[u8]>,
    /// Included in the policy to allow allocation of index structures to be optimized.
    primary_names_count: u32,
    permissions: IdAndNameIndexed<Box<[Permission]>>,
}

impl CommonSymbol {
    /// Returns the permissions associated with this common symbol.
    pub fn permissions(&self) -> &[Permission] {
        &self.permissions
    }
}

#[derive(Parse, Serialize)]
struct BinaryCommonSymbolHeader {
    name_len: u32,
    id: u32,
    /// Included in the policy to allow allocation of index structures to be optimized.
    primary_names_count: u32,
    permissions_count: u32,
}

impl Parse for CommonSymbol {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let header = BinaryCommonSymbolHeader::parse(cursor)?;

        let name = Box::from(cursor.read_bytes(header.name_len as usize)?);

        let mut permissions_vec = Vec::with_capacity(header.permissions_count as usize);
        for _ in 0..header.permissions_count {
            permissions_vec.push(Permission::parse(cursor)?);
        }
        let permissions = IdAndNameIndexed::new(permissions_vec.into_boxed_slice())?;

        let id = CommonSymbolId::from_u32(header.id)
            .ok_or(ParseError::InvalidId { value: header.id })?;

        Ok(Self { id, name, primary_names_count: header.primary_names_count, permissions })
    }
}

impl Serialize for CommonSymbol {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let header = BinaryCommonSymbolHeader {
            name_len: self.name.len() as u32,
            id: self.id.as_u32(),
            primary_names_count: self.primary_names_count,
            permissions_count: self.permissions.len() as u32,
        };
        header.serialize(writer)?;

        writer.write_bytes(&self.name);
        self.permissions.serialize(writer)?;
        Ok(())
    }
}

impl Validate for CommonSymbol {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        self.permissions.validate(policy)?;
        if self.primary_names_count > self.permissions.len() as u32 {
            return Err(ValidateError::InvalidPrimaryNamesCount {
                expected_at_most: self.permissions.len() as u32,
                found: self.primary_names_count,
            });
        }
        Ok(())
    }
}

impl Validate for CommonSymbolId {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        policy
            .common_symbols()
            .get_by_id(*self)
            .map(|_| ())
            .ok_or_else(|| ValidateError::UnknownId { kind: "common_symbol", id: self.as_u32() })
    }
}
