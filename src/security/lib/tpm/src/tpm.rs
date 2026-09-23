// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::capability::{
    Tpm2Capability, Tpm2ManufacturerProperty, Tpm2PermanentProperty, Tpm2Property,
    Tpm2StartupClearProperty,
};
use crate::error::TpmError;
use crate::hierarchy::{Tpm2Hierarchy, Tpm2HierarchyPasswords, TPM2_HIERARCHY_AUTH_SIZE};
use crate::tss::{EsysContext, TctiContext};

/// Defines a safe constant for the expected authorization size. This matches
/// the max size of `TPM2B_DIGEST`.
pub const AUTH_SIZE: usize = 64;

/// The `Tpm` is the interface to the `Trusted Platform Module 2.0`. The
/// intention of this interface is to provide a small surface area of
/// methods limited to just the Fuchsia projects requirements that importantly
/// are not 1:1 mappings to TPM commands but are instead mapped around
/// features such as provisioning, deprovisioning etc. This reduces the chance
/// of misuse and abstracts the underlying C implementation and the memory
/// allocation complications of that from the consumer.
pub struct Tpm {
    /// The Enhanced System API (esys) context contains all the bookkeeping
    /// about the current TPM device or agent we are connected to.
    esys_context: EsysContext,
}

impl Tpm {
    /// Attempts to connect to the TPM by establishing a connection with the
    /// TPM Command Transmission Interface (TCTI). It then attempts to use
    /// that TCTI connection to construct an Enhanced System API interface
    /// (ESYS) which is used to service commands.
    pub fn try_new() -> Result<Self, TpmError> {
        let tcti_context = TctiContext::try_new()?;
        let esys_context = EsysContext::try_new(tcti_context)?;
        Ok(Self { esys_context })
    }

    /// Calls TPM2_StirRandom. Returns nothing unless an error occurs.
    /// The buffer must not be greater than 128 bytes as defined in
    /// section 16.2.1 of the Commands specification.
    pub fn stir_random(&self, buffer: Vec<u8>) -> Result<(), TpmError> {
        self.esys_context.stir_random(buffer)
    }

    /// Calls TPM2_GetRandom Returns `bytes_requested` random bytes from the
    /// TPM.
    pub fn get_random(&self, bytes_requested: usize) -> Result<Vec<u8>, TpmError> {
        self.esys_context.get_random(bytes_requested)
    }

    /// Calls TPM2_Startup passing in `TPM2_SU_CLEAR` if `clear` is set
    /// otherwise passing through `TPM2_SU_STATE`.
    pub fn startup(&self, clear: bool) -> Result<(), TpmError> {
        self.esys_context.startup(clear)
    }

    /// Calls TPM2_Clear. This assumes the command has not been disabled by
    /// TPM2_ClearControl and that the platform auth is default.
    pub fn clear(&self) -> Result<(), TpmError> {
        self.esys_context.clear()
    }

    /// Disables the ability for the `TPM_RH_OWNER` to clear the TPM; instead
    /// requiring `TPM_RH_LOCKOUT` authorization.
    pub fn disable_owner_clear(&self) -> Result<(), TpmError> {
        self.esys_context.clear_control(/*disable=*/ true)
    }

    /// Enables the ability for the `TPM_RH_OWNER` to clear the TPM.
    pub fn enable_owner_clear(&self) -> Result<(), TpmError> {
        self.esys_context.clear_control(/*disable=*/ false)
    }

    /// Returns whethere the TPM is currently owned.
    pub fn is_owned(&self) -> Result<bool, TpmError> {
        let property = self.permanent_property()?;
        Ok(property.is_owner_auth_set())
    }

    /// Takes ownership of the TPM. This is based on the information outlined
    /// in the TCG TPM v2.0 Provisioning Guidance Document.
    pub fn take_ownership(&self) -> Result<Tpm2HierarchyPasswords, TpmError> {
        // Clear the TPM
        self.clear()?;
        // Generate 256 bit random values for each authorization.
        let owner_auth = self.get_random(TPM2_HIERARCHY_AUTH_SIZE)?;
        let endorsement_auth = self.get_random(TPM2_HIERARCHY_AUTH_SIZE)?;
        let lockout_auth = self.get_random(TPM2_HIERARCHY_AUTH_SIZE)?;
        // Set Authorization on each of the three hierarchy.
        self.set_owner_auth(owner_auth.as_slice())?;
        self.set_endorsement_auth(endorsement_auth.as_slice())?;
        self.set_lockout_auth(lockout_auth.as_slice())?;
        // Generate the storage root key with the well-known password. The
        // reason for this is that any application on the OS should be able
        // to encrypt/decrypt things with the SRK. It is simply a way to bind
        // some wrapped key to the device.
        // Create the well known primary root key.
        // Return the authorization incase they need to be stored on
        // engineering builds.
        Ok(Tpm2HierarchyPasswords {
            owner: owner_auth.try_into().unwrap(),
            endorsement: endorsement_auth.try_into().unwrap(),
            lockout: lockout_auth.try_into().unwrap(),
        })
    }

    /// Creates the primary storage root key assigning it the well-known
    /// password.
    #[allow(dead_code)]
    fn create_primary_srk(&self) -> Result<(), TpmError> {
        self.esys_context.create_primary(Tpm2Hierarchy::Owner)
    }

    /// Sets the Owner Authorization password to the provided `owner_auth_value`.
    /// Calls TPM2_HierarchyChangeAuth
    fn set_owner_auth(&self, owner_auth: &[u8]) -> Result<(), TpmError> {
        self.esys_context.hierarchy_change_auth(Tpm2Hierarchy::Owner, owner_auth)
    }

    /// Sets the Endorsment Authorization password to the provided `endorsment_auth_value`.
    /// Calls TPM2_HierarchyChangeAuth
    fn set_endorsement_auth(&self, endorsement_auth: &[u8]) -> Result<(), TpmError> {
        self.esys_context.hierarchy_change_auth(Tpm2Hierarchy::Endorsement, endorsement_auth)
    }

    /// Sets the Lockout Authorization password to the provided `lockout_auth_value`.
    /// Calls TPM2_HierarchyChangeAuth
    fn set_lockout_auth(&self, lockout_auth: &[u8]) -> Result<(), TpmError> {
        self.esys_context.hierarchy_change_auth(Tpm2Hierarchy::Lockout, lockout_auth)
    }

    /// Returns a single u32 representing a property from the capability type
    /// TPM_CAP_TPM_PROPERTIES. These are simple u32s that contain flags set
    /// for different properties of the chip.
    fn get_property(&self, property: Tpm2Property) -> Result<u32, TpmError> {
        let capability =
            self.esys_context.get_capability(&Tpm2Capability::Tpm2Properties(property), 1)?;
        unsafe {
            if capability.data.tpmProperties.count != 1 {
                return Err(TpmError::UnexpectedPropertyCount {
                    expected: 1,
                    found: capability.data.tpmProperties.count,
                });
            }
            // See TPM 2.0 Part 2: Structures Subsection 8.2 for the masks.
            Ok(capability.data.tpmProperties.tpmProperty[0].value)
        }
    }

    /// Returns `TPM2_PT_PERMANENT` flags which defines ownership.
    pub fn permanent_property(&self) -> Result<Tpm2PermanentProperty, TpmError> {
        Ok(Tpm2PermanentProperty(self.get_property(Tpm2Property::Permanent)?))
    }

    /// Returns `TPM2_PT_STARTUP_CLEAR` defines which hierarchies are enabled.
    pub fn startup_clear_property(&self) -> Result<Tpm2StartupClearProperty, TpmError> {
        Ok(Tpm2StartupClearProperty(self.get_property(Tpm2Property::StartupClear)?))
    }

    /// Returns the manufacturer string defined by `TPM2_PT_MANUFACTURER` in
    /// the specification.
    pub fn manufacturer_property(&self) -> Result<Tpm2ManufacturerProperty, TpmError> {
        Ok(Tpm2ManufacturerProperty(self.get_property(Tpm2Property::Manufacturer)?))
    }
}
