// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt;
use {hex, tpm2_tss_sys as tss_sys};

/// Taken from TCG TPM v2.0 Provisioning Guidance. This gives 256 bits of
/// random data per auth value.
pub const TPM2_HIERARCHY_AUTH_SIZE: usize = 32;

/// Contains the values the Owner,Endorsement and Lockout Authorizations
/// were set to.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Tpm2HierarchyPasswords {
    pub owner: [u8; TPM2_HIERARCHY_AUTH_SIZE],
    pub endorsement: [u8; TPM2_HIERARCHY_AUTH_SIZE],
    pub lockout: [u8; TPM2_HIERARCHY_AUTH_SIZE],
}

impl fmt::Debug for Tpm2HierarchyPasswords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tpm2HierarchyPasswords")
            .field("owner", &hex::encode(&self.owner))
            .field("endorsement", &hex::encode(&self.endorsement))
            .field("lockout", &hex::encode(&self.lockout))
            .finish()
    }
}

/// Typed representation of the `TPM2_TR_RH` type. This represents the
/// different TPM2 Hierarchy types that appera in the specification.
pub enum Tpm2Hierarchy {
    /// Represents the `TPM2_TR_RH_ENDORSEMENT` handle.
    Endorsement,
    /// Represents the `TPM2_TR_RH_OWNER` handle.
    Owner,
    /// Represents the `TPM2_TR_RH_PLATFORM` handle.
    Platform,
    /// Represents the `TPM2_TR_RH_LOCKOUT` handle.
    Lockout,
    /// Represents the `TPM2_TR_RH_NULL` handle.
    Null,
}

/// Converts the `TPM2_RH` type into the underlying TSS type.
impl From<Tpm2Hierarchy> for tss_sys::ESYS_TR {
    fn from(item: Tpm2Hierarchy) -> tss_sys::ESYS_TR {
        match item {
            Tpm2Hierarchy::Endorsement => tss_sys::ESYS_TR_RH_ENDORSEMENT,
            Tpm2Hierarchy::Owner => tss_sys::ESYS_TR_RH_OWNER,
            Tpm2Hierarchy::Platform => tss_sys::ESYS_TR_RH_PLATFORM,
            Tpm2Hierarchy::Lockout => tss_sys::ESYS_TR_RH_LOCKOUT,
            Tpm2Hierarchy::Null => tss_sys::ESYS_TR_RH_NULL,
        }
    }
}

/// Defined as part of TCG EK Credential Profile
/// We are utilising ECC over RSA here due to its speed and size.
/// Section B.3.4.
/// https://trustedcomputinggroup.org/wp-content/uploads/TCG_IWG_Credential_Profile_EK_V2.1_R13.pdf
pub static ENDORSEMENT_KEY_ECC_P256_TEMPLATE: tss_sys::TPM2B_PUBLIC = tss_sys::TPM2B_PUBLIC {
    size: std::mem::size_of::<tss_sys::TPMT_PUBLIC>() as u16,
    publicArea: tss_sys::TPMT_PUBLIC {
        type_: tss_sys::TPM2_ALG_ECC,
        nameAlg: tss_sys::TPM2_ALG_SHA256,
        objectAttributes: tss_sys::TPMA_OBJECT_FIXEDTPM
            | tss_sys::TPMA_OBJECT_FIXEDPARENT
            | tss_sys::TPMA_OBJECT_SENSITIVEDATAORIGIN
            | tss_sys::TPMA_OBJECT_ADMINWITHPOLICY
            | tss_sys::TPMA_OBJECT_RESTRICTED
            | tss_sys::TPMA_OBJECT_DECRYPT,
        authPolicy: tss_sys::TPM2B_DIGEST {
            size: 32,
            buffer: [
                0x83, 0x71, 0x97, 0x67, 0x44, 0x84, 0xB3, 0xF8, 0x1A, 0x90, 0xCC, 0x8D, 0x46, 0xA5,
                0xD7, 0x24, 0xFD, 0x52, 0xD7, 0x6E, 0x06, 0x52, 0x0B, 0x64, 0xF2, 0xA1, 0xDA, 0x1B,
                0x33, 0x14, 0x69, 0xAA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ],
        },
        parameters: tss_sys::TPMU_PUBLIC_PARMS {
            eccDetail: tss_sys::TPMS_ECC_PARMS {
                symmetric: tss_sys::TPMT_SYM_DEF_OBJECT {
                    algorithm: tss_sys::TPM2_ALG_AES,
                    keyBits: tss_sys::TPMU_SYM_KEY_BITS { aes: 128 },
                    mode: tss_sys::TPMU_SYM_MODE { aes: tss_sys::TPM2_ALG_CFB },
                },
                scheme: tss_sys::TPMT_ECC_SCHEME {
                    scheme: tss_sys::TPM2_ALG_NULL,
                    // This is defined as NULL in the specification.
                    details: tss_sys::TPMU_ASYM_SCHEME {
                        anySig: tss_sys::TPMS_SCHEME_HASH { hashAlg: tss_sys::TPM2_ALG_NULL },
                    },
                },
                curveID: tss_sys::TPM2_ECC_NIST_P256,
                kdf: tss_sys::TPMT_KDF_SCHEME {
                    scheme: tss_sys::TPM2_ALG_NULL,
                    // This is defined as NULL in the specification.
                    details: tss_sys::TPMU_KDF_SCHEME {
                        mgf1: tss_sys::TPMS_SCHEME_HASH { hashAlg: tss_sys::TPM2_ALG_NULL },
                    },
                },
            },
        },
        unique: tss_sys::TPMU_PUBLIC_ID {
            ecc: tss_sys::TPMS_ECC_POINT {
                x: tss_sys::TPM2B_ECC_PARAMETER { size: 32, buffer: [0; 128] },
                y: tss_sys::TPM2B_ECC_PARAMETER { size: 32, buffer: [0; 128] },
            },
        },
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hierarchy_from() {
        let endorsement: tss_sys::ESYS_TR = Tpm2Hierarchy::Endorsement.into();
        assert_eq!(endorsement, tss_sys::ESYS_TR_RH_ENDORSEMENT);
        let owner: tss_sys::ESYS_TR = Tpm2Hierarchy::Owner.into();
        assert_eq!(owner, tss_sys::ESYS_TR_RH_OWNER);
        let platform: tss_sys::ESYS_TR = Tpm2Hierarchy::Platform.into();
        assert_eq!(platform, tss_sys::ESYS_TR_RH_PLATFORM);
        let lockout: tss_sys::ESYS_TR = Tpm2Hierarchy::Lockout.into();
        assert_eq!(lockout, tss_sys::ESYS_TR_RH_LOCKOUT);
        let null: tss_sys::ESYS_TR = Tpm2Hierarchy::Null.into();
        assert_eq!(null, tss_sys::ESYS_TR_RH_NULL);
    }
}
