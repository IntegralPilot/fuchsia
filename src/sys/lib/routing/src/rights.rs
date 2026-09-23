// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::{RightsRoutingError, RoutingError};
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_io as fio;
use moniker::ExtendedMoniker;
use router_error::RouterError;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize, de::Deserializer, ser::Serializer};
use std::fmt;

/// Opaque rights type to define new traits like PartialOrd on.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Rights(fio::Operations);

impl Rights {
    /// Ensures the next walk state of rights satisfies a monotonic increasing sequence. Used to
    /// verify the expectation that no right requested from a use, offer, or expose is missing as
    /// capability routing walks from the capability's consumer to its provider.
    pub fn validate_next(
        &self,
        next_rights: &Self,
        moniker: ExtendedMoniker,
    ) -> Result<(), RightsRoutingError> {
        if next_rights.0.contains(self.0) {
            Ok(())
        } else {
            Err(RightsRoutingError::Invalid { moniker, requested: *self, provided: *next_rights })
        }
    }
}

/// Allows creating rights from fio::Operations.
impl From<fio::Operations> for Rights {
    fn from(rights: fio::Operations) -> Self {
        Rights(rights)
    }
}

impl From<Rights> for fio::Flags {
    fn from(rights: Rights) -> Self {
        fio::Flags::from_bits_retain(rights.0.bits())
    }
}

impl Into<u64> for Rights {
    fn into(self) -> u64 {
        self.0.bits()
    }
}

impl Into<fio::Operations> for Rights {
    fn into(self) -> fio::Operations {
        let Self(ops) = self;
        ops
    }
}

impl From<fio::Flags> for Rights {
    fn from(flags: fio::Flags) -> Self {
        Self(
            fio::Operations::from_bits(flags.bits())
                .expect("operations is bit-compatible with flags"),
        )
    }
}

impl fmt::Display for Rights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(rights) = &self;
        match *rights {
            fio::R_STAR_DIR => write!(f, "r*"),
            fio::W_STAR_DIR => write!(f, "w*"),
            fio::X_STAR_DIR => write!(f, "x*"),
            fio::RW_STAR_DIR => write!(f, "rw*"),
            fio::RX_STAR_DIR => write!(f, "rx*"),
            ops => write!(f, "{:?}", ops),
        }
    }
}

#[cfg(feature = "serde")]
impl Serialize for Rights {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let Self(rights) = self;
        rights.bits().serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for Rights {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bits: u64 = Deserialize::deserialize(deserializer)?;
        let rights = fio::Operations::from_bits(bits)
            .ok_or_else(|| serde::de::Error::custom("invalid value for fuchsia.io/Operations"))?;
        Ok(Self(rights))
    }
}

pub fn validate_rights(
    moniker: ExtendedMoniker,
    allowed_rights: fio::Operations,
    request: &mut RouteRequest,
) -> Result<(), RouterError> {
    let allowed_rights = Rights::from(allowed_rights);
    let inherit = request.inherit_rights.ok_or_else(|| RoutingError::RouteRequestMissingField {
        moniker: moniker.clone(),
        missing_field: "inherit_rights".to_string(),
    })?;
    let request_rights: Rights = match request.directory_rights {
        Some(request_rights) => request_rights.into(),
        None => {
            if inherit {
                request.directory_rights = Some(fio::Flags::from(allowed_rights));
                allowed_rights
            } else {
                Err(RoutingError::RouteRequestMissingField {
                    moniker: moniker.clone(),
                    missing_field: "directory_rights".to_string(),
                })?
            }
        }
    };
    request_rights.validate_next(&allowed_rights, moniker.clone()).map_err(RoutingError::from)?;
    if let Some(intermediate_rights) = request.directory_intermediate_rights {
        Rights::from(intermediate_rights)
            .validate_next(&allowed_rights, moniker)
            .map_err(|e| router_error::RouterError::from(RoutingError::from(e)))?;
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn validate_next() {
        assert_matches!(
            Rights(fio::Operations::empty())
                .validate_next(&Rights(fio::R_STAR_DIR,), ExtendedMoniker::ComponentManager),
            Ok(())
        );
        assert_matches!(
            Rights(fio::Operations::READ_BYTES | fio::Operations::GET_ATTRIBUTES,)
                .validate_next(&Rights(fio::R_STAR_DIR), ExtendedMoniker::ComponentManager),
            Ok(())
        );
        let provided = fio::Operations::READ_BYTES | fio::Operations::GET_ATTRIBUTES;
        assert_eq!(
            Rights(fio::R_STAR_DIR)
                .validate_next(&Rights(provided), ExtendedMoniker::ComponentManager),
            Err(RightsRoutingError::Invalid {
                moniker: ExtendedMoniker::ComponentManager,
                requested: Rights::from(fio::R_STAR_DIR),
                provided: Rights::from(provided),
            })
        );
        let provided = fio::Operations::READ_BYTES | fio::Operations::GET_ATTRIBUTES;
        assert_eq!(
            Rights(fio::Operations::WRITE_BYTES)
                .validate_next(&Rights(provided), ExtendedMoniker::ComponentManager),
            Err(RightsRoutingError::Invalid {
                moniker: ExtendedMoniker::ComponentManager,
                requested: Rights::from(fio::Operations::WRITE_BYTES),
                provided: Rights::from(provided),
            })
        );
    }
}
