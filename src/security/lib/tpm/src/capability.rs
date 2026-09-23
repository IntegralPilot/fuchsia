// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::util::flag_set;
use std::convert::From;
use std::fmt;
use tpm2_tss_sys as tss_sys;

/// `TPM_PT` defining the properties associated with a TPM_CAP_TPM_PROPERTIES
/// capability type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tpm2Property {
    /// `TPM2_PT_MANUFACTURER`, a string describing the manufacturer.
    Manufacturer,
    /// `TPM2_PT_PERMANENT`, a property tag to retrieve the TPMA_PERMANENT
    /// structure see TPM 2.0 Part 2 Structures Section 8.6
    Permanent,
    /// `TPM2_PT_STARTUP_CLEAR` defines which hierarchies are enabled.
    /// See https://trustedcomputinggroup.org/wp-content/uploads/TCG_TPM2_r1p59_Part1_Architecture_pub.pdf
    /// for more information on hierarchies.
    StartupClear,
}

/// Converts a `Tpm2Property` into its underlying `tss_sys::TPM2_PT` type.
impl From<&Tpm2Property> for tss_sys::TPM2_PT {
    fn from(item: &Tpm2Property) -> tss_sys::TPM2_PT {
        match item {
            Tpm2Property::Manufacturer => tss_sys::TPM2_PT_MANUFACTURER,
            Tpm2Property::Permanent => tss_sys::TPM2_PT_PERMANENT,
            Tpm2Property::StartupClear => tss_sys::TPM2_PT_STARTUP_CLEAR,
        }
    }
}

/// `TPM2_CAP` enum of the available TPM2_CAP variants, codified as a real type
/// instead of just an arbitrary integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tpm2Capability {
    /// TPM_CAP_TPM_PROPERTIES returns a list of tagged properties.
    Tpm2Properties(Tpm2Property),
}

/// Converts a `Tpm2Capability` into its underlying `tss_sys::TPM2_CAP` type.
impl From<&Tpm2Capability> for tss_sys::TPM2_CAP {
    fn from(item: &Tpm2Capability) -> tss_sys::TPM2_CAP {
        match item {
            Tpm2Capability::Tpm2Properties(_) => tss_sys::TPM2_CAP_TPM_PROPERTIES,
        }
    }
}

/// Returns the property associtated with the capability as an underlying u32
/// so it can interop with the tss_sys API.
pub(crate) fn capability_property(capability: &Tpm2Capability) -> u32 {
    match capability {
        Tpm2Capability::Tpm2Properties(property) => property.into(),
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Tpm2ManufacturerProperty(pub u32);

impl Tpm2ManufacturerProperty {
    pub fn manufacturer(&self) -> u32 {
        self.0.clone()
    }
}

impl fmt::Debug for Tpm2ManufacturerProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut binding = f.debug_struct("Tpm2ManufacturerProperty");
        let mut debug = binding.field("value", &self.0);
        let bytes = self.0.to_be_bytes();
        if let Ok(value_str) = std::str::from_utf8(&bytes) {
            debug = debug.field("value_as_str", &value_str);
        }
        debug.finish()
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Tpm2PermanentProperty(pub u32);

impl Tpm2PermanentProperty {
    /// Returns the ownership status of the TPM defined in the tagged
    /// `TPM2_PT_PERMANENT` with the bitmask `TPMA_PERMANENT_OWNERAUTHSET`.
    pub fn is_owner_auth_set(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_PERMANENT_OWNERAUTHSET)
    }

    /// Returns the ownership status of the TPM defined in the tagged
    /// `TPM2_PT_PERMANENT` with the bitmask `TPMA_PERMANENT_ENDORSEMENTAUTHSET`.
    pub fn is_endorsement_auth_set(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_PERMANENT_ENDORSEMENTAUTHSET)
    }

    /// Returns the ownership status of the TPM defined in the tagged
    /// `TPM2_PT_PERMANENT` with the bitmask `TPMA_PERMANENT_LOCKOUTAUTH`.
    pub fn is_lockout_auth_set(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_PERMANENT_LOCKOUTAUTHSET)
    }

    /// Returns the ownership lockout of the TPM defined in the tagged
    /// `TPM2_PT_PERMANENT` with the bitmask `TPMA_PERMANENT_INLOCKOUT`.
    pub fn is_in_lockout(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_PERMANENT_INLOCKOUT)
    }

    /// Returns whether TPM2_ClearControl has disabled clearing the TPM
    /// with `TPM2_PT_OWNER`.
    pub fn is_disable_clear_set(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_PERMANENT_DISABLECLEAR)
    }
}
impl fmt::Debug for Tpm2PermanentProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tpm2PermanentProperty")
            .field("value", &self.0)
            .field("owner_auth_set", &self.is_owner_auth_set())
            .field("endorsement_auth_set", &self.is_endorsement_auth_set())
            .field("lockout_auth_set", &self.is_lockout_auth_set())
            .field("in_lockout", &self.is_in_lockout())
            .field("disable_clear_set", &self.is_disable_clear_set())
            .finish()
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Tpm2StartupClearProperty(pub u32);

impl Tpm2StartupClearProperty {
    /// Returns true if the platform hierarchy is enabled.
    /// See 8.7 of the TPM 2.0 Specification Part 2: Structures
    pub fn is_ph_enabled(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_STARTUP_CLEAR_PHENABLE)
    }

    /// Returns true if the storage hierarchy is enabled.
    /// See 8.7 of the TPM 2.0 Specification Part 2: Structures
    pub fn is_sh_enabled(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_STARTUP_CLEAR_SHENABLE)
    }

    /// Returns true if the endorsment hierarchy is enabled.
    /// See 8.7 of the TPM 2.0 Specification Part 2: Structures
    pub fn is_eh_enabled(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_STARTUP_CLEAR_EHENABLE)
    }

    /// Returns true if the TPMA_NV_PLATFORMCREATE may be read or written.
    /// See 8.7 of the TPM 2.0 Specification Part 2: Structures
    pub fn is_ph_nv_enabled(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_STARTUP_CLEAR_PHENABLENV)
    }

    /// Returns true if the TPM2 received a matching shutdown and startup.
    /// See 8.7 of the TPM 2.0 Specification Part 2: Structures
    pub fn is_startup_orderly(&self) -> bool {
        flag_set(self.0, tpm2_tss_sys::TPMA_STARTUP_CLEAR_ORDERLY)
    }
}

impl fmt::Debug for Tpm2StartupClearProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tpm2StartupClearProperty")
            .field("value", &self.0)
            .field("ph_enabled", &self.is_ph_enabled())
            .field("sh_enabled", &self.is_sh_enabled())
            .field("eh_enabled", &self.is_eh_enabled())
            .field("ph_nv_enabled", &self.is_ph_nv_enabled())
            .field("startup_orderly", &self.is_startup_orderly())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tpm2_permanent_property() {
        let property = Tpm2PermanentProperty(0);
        assert!(!property.is_owner_auth_set());
        assert!(!property.is_endorsement_auth_set());
        assert!(!property.is_lockout_auth_set());
        assert!(!property.is_in_lockout());
        assert!(!property.is_disable_clear_set());
    }

    #[test]
    fn test_tpm2_startup_clear_property() {
        let property = Tpm2StartupClearProperty(0);
        assert!(!property.is_ph_enabled());
        assert!(!property.is_sh_enabled());
        assert!(!property.is_eh_enabled());
        assert!(!property.is_ph_nv_enabled());
        assert!(!property.is_startup_orderly());
    }
}
