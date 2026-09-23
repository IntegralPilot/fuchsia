// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::capability::{capability_property, Tpm2Capability};
use crate::error::TpmError;
use crate::ffi_return_if_error;
use crate::hierarchy::{
    Tpm2Hierarchy, ENDORSEMENT_KEY_ECC_P256_TEMPLATE, TPM2_HIERARCHY_AUTH_SIZE,
};
use crate::tpm::AUTH_SIZE;
use crate::tss::heap::EsysHeapObj;
use crate::tss::tcti::TctiContext;
use std::ptr::null_mut;
use tpm2_tss_sys as tss_sys;

/// The number of session handles that the ESYS TPM2.0 can support for most
/// of its functions. This is used to simplify session management.
const NUM_SESSION_HANDLES: usize = 3;

/// This maximum size for TPM2_StirRandom is defined in section 16.2.1 of
/// TPM2.0 Specification Part 3: Commands.
const MAX_STIR_RANDOM_LEN: usize = 128;

/// Defined in `TPM2_MAX_SYM_DATA` but as a u32 which cannot be const cast.
const MAX_SENSITIVE_DATA_LEN: usize = 256;

/// Safe wrapper around the `ESYS_CONTEXT`. The Enhanced System API Context
/// hosts all the bookkeeping the TSS2 library. The `EsysContext` must own
/// the lifetime of the `TctiContext` however it is not responsible for
/// freeing the `TctiContext` which is the same behavior as in the C API.
/// TctiContext defines its own free mechanism so when the context goes out
/// of scope it will be freed after we finalize the `EsysContext`.
pub struct EsysContext {
    _tcti_context: TctiContext,
    esys_context: *mut tss_sys::ESYS_CONTEXT,
    session_handles: [u32; NUM_SESSION_HANDLES],
}

impl EsysContext {
    /// Attempts initialize the TSS2 Enhanced System API Context with the
    /// provided `tcti_context`. `EsysContext` takes ownership of the lifetime
    /// of the `TctiContext`.
    pub fn try_new(tcti_context: TctiContext) -> Result<Self, TpmError> {
        let mut esys_context = null_mut();
        ffi_return_if_error!(tss_sys::Esys_Initialize(
            &mut esys_context,
            tcti_context.as_mut_ptr(),
            null_mut()
        ));
        Ok(Self {
            _tcti_context: tcti_context,
            esys_context,
            session_handles: [tss_sys::ESYS_TR_NONE, tss_sys::ESYS_TR_NONE, tss_sys::ESYS_TR_NONE],
        })
    }

    /// Set all the session handles.
    #[allow(dead_code)]
    pub fn set_session_handles(
        &mut self,
        sessions: (tss_sys::ESYS_TR, tss_sys::ESYS_TR, tss_sys::ESYS_TR),
    ) {
        self.session_handles[0] = sessions.0;
        self.session_handles[1] = sessions.1;
        self.session_handles[2] = sessions.2;
    }

    /// Get all the active session handles.
    #[allow(dead_code)]
    pub fn session_handles(&self) -> (tss_sys::ESYS_TR, tss_sys::ESYS_TR, tss_sys::ESYS_TR) {
        (self.session_handles[0], self.session_handles[1], self.session_handles[2])
    }

    /// Returns the first session handle intended for slot one.
    fn session_handle_one(&self) -> u32 {
        self.session_handles[0]
    }

    /// Returns the first session handle intended for slot two.
    fn session_handle_two(&self) -> u32 {
        self.session_handles[1]
    }

    /// Returns the first session handle intended for slot three.
    fn session_handle_three(&self) -> u32 {
        self.session_handles[2]
    }

    /// This function invokes TPM2_Startup with the SU_CLEAR param.
    /// See: Trusted Platform Module Library, Part 3: Commands,
    ///      Subsection 9.3.1.
    pub fn startup(&self, clear: bool) -> Result<(), TpmError> {
        if clear {
            ffi_return_if_error!(tss_sys::Esys_Startup(self.esys_context, tss_sys::TPM2_SU_CLEAR));
        } else {
            ffi_return_if_error!(tss_sys::Esys_Startup(self.esys_context, tss_sys::TPM2_SU_STATE));
        }
        Ok(())
    }

    /// This function invokes TPM2_Clear which attempts to clear the TPM.
    /// See: Trusted Platform Module Library, Part 3: Commands,
    ///      Subsection 24.6.1
    pub fn clear(&self) -> Result<(), TpmError> {
        ffi_return_if_error!(tss_sys::Esys_Clear(
            self.esys_context,
            tss_sys::ESYS_TR_RH_LOCKOUT,
            tss_sys::ESYS_TR_PASSWORD,
            tss_sys::ESYS_TR_NONE,
            tss_sys::ESYS_TR_NONE,
        ));
        Ok(())
    }

    /// This function invokes TPM2_ClearControl which sets the
    /// TPMA_PERMANENT.disableClear attribute to require lockout
    /// authorization for TPM2_Clear. `disable` yes if the disableOwnerClear
    /// flag is to be set, no, if the flag is to be clear.
    pub fn clear_control(&self, disable: bool) -> Result<(), TpmError> {
        let yes_no: tss_sys::TPMI_YES_NO = match disable {
            true => tss_sys::TPM2_YES.try_into().unwrap(),
            false => tss_sys::TPM2_NO.try_into().unwrap(),
        };
        ffi_return_if_error!(tss_sys::Esys_ClearControl(
            self.esys_context,
            tss_sys::TPM2_RH_LOCKOUT,
            self.session_handle_one(),
            self.session_handle_two(),
            self.session_handle_three(),
            yes_no,
        ));
        Ok(())
    }

    /// Calls `Esys_TR_SetAuth` which sets the authorization associated with
    /// the `ESYS_TR` Tpm Resource.
    pub fn set_auth(
        &self,
        handle: tss_sys::ESYS_TR,
        auth: &tss_sys::TPM2B_AUTH,
    ) -> Result<(), TpmError> {
        ffi_return_if_error!(tss_sys::Esys_TR_SetAuth(self.esys_context, handle, auth));
        Ok(())
    }

    /// Calls `TPM2_HierarchyChangeAuth` to set the authorization value on
    /// a particular hierarchy.
    pub fn hierarchy_change_auth(
        &self,
        hierarchy: Tpm2Hierarchy,
        auth_value: &[u8],
    ) -> Result<(), TpmError> {
        // The TCG Provisioning Guidelines have strict requirements on the size
        // of this buffer.
        if auth_value.len() != TPM2_HIERARCHY_AUTH_SIZE {
            if auth_value.len() < TPM2_HIERARCHY_AUTH_SIZE {
                return Err(TpmError::BufferTooSmall {
                    expected: TPM2_HIERARCHY_AUTH_SIZE,
                    found: auth_value.len(),
                });
            } else {
                return Err(TpmError::BufferTooLarge {
                    maximum: TPM2_HIERARCHY_AUTH_SIZE,
                    found: auth_value.len(),
                });
            }
        }
        let mut auth_buffer = [0; AUTH_SIZE];
        for (src, dst) in auth_value.iter().zip(auth_buffer.iter_mut()) {
            *dst = *src;
        }
        let auth = tss_sys::TPM2B_AUTH {
            size: TPM2_HIERARCHY_AUTH_SIZE.try_into().unwrap(),
            buffer: auth_buffer,
        };
        let hierarchy_handle: tss_sys::ESYS_TR = hierarchy.into();
        ffi_return_if_error!(tss_sys::Esys_HierarchyChangeAuth(
            self.esys_context,
            hierarchy_handle,
            tss_sys::ESYS_TR_PASSWORD,
            tss_sys::ESYS_TR_NONE,
            tss_sys::ESYS_TR_NONE,
            &auth,
        ));
        self.set_auth(hierarchy_handle, &auth)
    }

    /// Calls `TPM2_StirRandom`. Returns an error if the provided input size is
    /// too large.
    pub fn stir_random(&self, buffer: Vec<u8>) -> Result<(), TpmError> {
        if buffer.len() == 0 {
            return Ok(());
        }
        if buffer.len() > MAX_STIR_RANDOM_LEN {
            return Err(TpmError::BufferTooLarge {
                maximum: MAX_STIR_RANDOM_LEN,
                found: buffer.len(),
            });
        }

        let mut sensitive_data = tss_sys::TPM2B_SENSITIVE_DATA {
            size: buffer.len().try_into().expect("buffer should always be bounded by u16"),
            buffer: [0; MAX_SENSITIVE_DATA_LEN],
        };
        for (i, e) in buffer.iter().enumerate() {
            sensitive_data.buffer[i] = *e;
        }
        ffi_return_if_error!(tss_sys::Esys_StirRandom(
            self.esys_context,
            self.session_handle_one(),
            self.session_handle_two(),
            self.session_handle_three(),
            &sensitive_data,
        ));
        Ok(())
    }

    /// Calls `TPM2_GetRandom` returning `bytes_requested` of random bytes from
    /// the TPM. Sizes larger than the max payload of `TPM2_GetRandom` will be
    /// batched and extended onto each other.
    /// See: Trusted Platform Module Library, Part 3: Commands,
    ///      Subsection: 16.1.1
    pub fn get_random(&self, bytes_requested: usize) -> Result<Vec<u8>, TpmError> {
        if bytes_requested == 0 {
            return Ok(vec![]);
        }
        // For requests greater than the max payload batch them.
        let mut random_bytes: Vec<u8> = Vec::new();
        let mut remaining_bytes = bytes_requested;
        let max_payload = std::mem::size_of::<tss_sys::TPMU_HA>();
        while remaining_bytes > max_payload {
            // This frees the buffer on each iteration of the loop. Which is
            // required as each call performs an allocation.
            let mut digest = EsysHeapObj::<tss_sys::TPM2B_DIGEST>::null();
            ffi_return_if_error!(tss_sys::Esys_GetRandom(
                self.esys_context,
                self.session_handle_one(),
                self.session_handle_two(),
                self.session_handle_three(),
                bytes_requested.try_into().expect("this should always be bounded by u16"),
                &mut digest.inner,
            ));
            // Extend the buffer by the number of random bytes received. Note
            // the total size of the buffer may be larger than the size of the
            // bytes written.
            let mut bytes = Vec::from(digest.as_ref().buffer);
            bytes.truncate(digest.as_ref().size.into());
            random_bytes.extend(bytes);
            remaining_bytes -= max_payload;
        }
        let mut remaining_digest = EsysHeapObj::<tss_sys::TPM2B_DIGEST>::null();
        ffi_return_if_error!(tss_sys::Esys_GetRandom(
            self.esys_context,
            self.session_handle_one(),
            self.session_handle_two(),
            self.session_handle_three(),
            remaining_bytes.try_into().expect("this should always be bounded by u16"),
            &mut remaining_digest.inner,
        ));
        // Add any remaining bytes that do not fit in a full size batch.
        let mut bytes = Vec::from(remaining_digest.as_ref().buffer);
        bytes.truncate(remaining_digest.as_ref().size.into());
        random_bytes.extend(bytes);
        Ok(random_bytes)
    }

    /// Calls `TPM2_GetCapability` attempting to retrieve the `capability` in
    /// the quantity specified by the `property_count`.
    /// See: Trusted Platform Module Library, Part 3: Commands,
    ///      Subsection: 30.2.1
    pub fn get_capability(
        &self,
        capability: &Tpm2Capability,
        property_count: u32,
    ) -> Result<tss_sys::TPMS_CAPABILITY_DATA, TpmError> {
        let mut more_data: tss_sys::TPMI_YES_NO = tss_sys::TPM2_NO as tss_sys::TPMI_YES_NO;
        let mut capability_data = EsysHeapObj::<tss_sys::TPMS_CAPABILITY_DATA>::null();
        ffi_return_if_error!(tss_sys::Esys_GetCapability(
            self.esys_context,
            self.session_handle_one(),
            self.session_handle_two(),
            self.session_handle_three(),
            capability.into(),
            capability_property(&capability),
            property_count,
            &mut more_data,
            &mut capability_data.inner,
        ));
        // Clone the data out of the C heap allocated structure to simplify
        // memory management for the callee.
        let capability_clone = capability_data.as_ref().clone();
        Ok(capability_clone)
    }

    /// Calls `TPM2_CreatePrimary` which is used to create a Primary Object
    /// under one of the Primary Seeds. This command specifies a pre-defined
    /// template that meets the requirements specified in the TPM TCG
    /// Provisioning Guidance.
    pub fn create_primary(&self, hierarchy: Tpm2Hierarchy) -> Result<(), TpmError> {
        // Set the SRK sensitive data.
        let sensitive = tss_sys::TPM2B_SENSITIVE_CREATE {
            size: std::mem::size_of::<tss_sys::TPMS_SENSITIVE_CREATE>() as u16,
            sensitive: tss_sys::TPMS_SENSITIVE_CREATE {
                userAuth: tss_sys::TPM2B_AUTH { size: 0, buffer: [0; 64] },
                data: unsafe { std::mem::zeroed() },
            },
        };
        // Do not bind the SRK to any PCR selection.
        let pcr_selection = tss_sys::TPML_PCR_SELECTION {
            count: 0,
            pcrSelections: [unsafe { std::mem::zeroed() }; 16],
        };
        let outside_info = tss_sys::TPM2B_DATA { size: 0, buffer: [0; 64] };
        // Construct all of the out parameters for the command.
        let mut handle: tss_sys::ESYS_TR = 0;
        let mut public_data = EsysHeapObj::<tss_sys::TPM2B_PUBLIC>::null();
        let mut creation_data = EsysHeapObj::<tss_sys::TPM2B_CREATION_DATA>::null();
        let mut creation_hash = EsysHeapObj::<tss_sys::TPM2B_DIGEST>::null();
        let mut creation_ticket = EsysHeapObj::<tss_sys::TPMT_TK_CREATION>::null();
        ffi_return_if_error!(tss_sys::Esys_CreatePrimary(
            self.esys_context,
            hierarchy.into(),
            tss_sys::ESYS_TR_PASSWORD,
            tss_sys::ESYS_TR_NONE,
            tss_sys::ESYS_TR_NONE,
            &sensitive,
            &ENDORSEMENT_KEY_ECC_P256_TEMPLATE,
            &outside_info,
            &pcr_selection,
            &mut handle,
            &mut public_data.inner,
            &mut creation_data.inner,
            &mut creation_hash.inner,
            &mut creation_ticket.inner,
        ));
        Ok(())
    }
}

/// Finalizes and frees the memory associated with the internal `esys_context`.
impl Drop for EsysContext {
    fn drop(&mut self) {
        unsafe {
            tss_sys::Esys_Finalize(&mut self.esys_context);
        };
    }
}
