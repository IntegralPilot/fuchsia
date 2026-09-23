// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;
use anyhow::format_err;
use num::FromPrimitive;
use std::os::raw::c_char;

// This mirrors the behavior from ot-br-posix
// https://github.com/openthread/ot-br-posix/blob/main/src/border_agent/border_agent.cpp
const EPSKC_RANDOM_GEN_LEN: usize = 8;

/// Represents the thread joiner state.
///
/// Functional equivalent of [`otsys::otJoinerState`](crate::otsys::otJoinerState).
#[derive(
    Debug,
    Copy,
    Clone,
    Eq,
    Ord,
    PartialOrd,
    PartialEq,
    num_derive::FromPrimitive,
    num_derive::ToPrimitive,
)]
pub enum BorderAgentEphemeralKeyState {
    /// Functional equivalent of [`otsys::OT_BORDER_AGENT_STATE_DISABLED`](crate::otsys::OT_BORDER_AGENT_STATE_DISABLED).
    Disabled = OT_BORDER_AGENT_STATE_DISABLED as isize,

    /// Functional equivalent of [`otsys::OT_BORDER_AGENT_STATE_STOPPED`](crate::otsys::OT_BORDER_AGENT_STATE_STOPPED).
    Stopped = OT_BORDER_AGENT_STATE_STOPPED as isize,

    /// Functional equivalent of [`otsys::OT_BORDER_AGENT_STATE_STARTED`](crate::otsys::OT_BORDER_AGENT_STATE_STARTED).
    Started = OT_BORDER_AGENT_STATE_STARTED as isize,

    /// Functional equivalent of [`otsys::OT_BORDER_AGENT_STATE_CONNECTED`](crate::otsys::OT_BORDER_AGENT_STATE_CONNECTED).
    Connected = OT_BORDER_AGENT_STATE_CONNECTED as isize,

    /// Functional equivalent of [`otsys::OT_BORDER_AGENT_STATE_ACCEPTED`](crate::otsys::OT_BORDER_AGENT_STATE_ACCEPTED).
    Accepted = OT_BORDER_AGENT_STATE_ACCEPTED as isize,
}

impl From<otBorderAgentEphemeralKeyState> for BorderAgentEphemeralKeyState {
    fn from(x: otBorderAgentEphemeralKeyState) -> Self {
        Self::from_u32(x)
            .unwrap_or_else(|| panic!("Unknown otBorderAgentEphemeralKeyState value: {x}"))
    }
}

impl From<BorderAgentEphemeralKeyState> for otBorderAgentEphemeralKeyState {
    fn from(x: BorderAgentEphemeralKeyState) -> Self {
        x as otBorderAgentEphemeralKeyState
    }
}

#[derive(Debug, PartialEq)]
#[allow(missing_docs)]
pub struct BorderAgentCounters {
    pub epskc_activations: u32,
    pub epskc_deactivation_clears: u32,
    pub epskc_deactivation_timeouts: u32,
    pub epskc_deactivation_max_attempts: u32,
    pub epskc_deactivation_disconnects: u32,
    pub epskc_invalid_ba_state_errors: u32,
    pub epskc_invalid_args_errors: u32,
    pub epskc_start_secure_session_errors: u32,
    pub epskc_secure_session_successes: u32,
    pub epskc_secure_session_failures: u32,
    pub epskc_commissioner_petitions: u32,
    pub pskc_secure_session_successes: u32,
    pub pskc_secure_session_failures: u32,
    pub pskc_commissioner_petitions: u32,
    pub mgmt_active_gets: u32,
    pub mgmt_pending_gets: u32,
}

impl BorderAgentCounters {
    unsafe fn from_ot_counters(
        counters: *const otBorderAgentCounters,
    ) -> Option<BorderAgentCounters> {
        unsafe { counters.as_ref() }.map(|&counters| BorderAgentCounters {
            epskc_activations: counters.mEpskcActivations,
            epskc_deactivation_clears: counters.mEpskcDeactivationClears,
            epskc_deactivation_timeouts: counters.mEpskcDeactivationTimeouts,
            epskc_deactivation_max_attempts: counters.mEpskcDeactivationMaxAttempts,
            epskc_deactivation_disconnects: counters.mEpskcDeactivationDisconnects,
            epskc_invalid_ba_state_errors: counters.mEpskcInvalidBaStateErrors,
            epskc_invalid_args_errors: counters.mEpskcInvalidArgsErrors,
            epskc_start_secure_session_errors: counters.mEpskcStartSecureSessionErrors,
            epskc_secure_session_successes: counters.mEpskcSecureSessionSuccesses,
            epskc_secure_session_failures: counters.mEpskcSecureSessionFailures,
            epskc_commissioner_petitions: counters.mEpskcCommissionerPetitions,
            pskc_secure_session_successes: counters.mPskcSecureSessionSuccesses,
            pskc_secure_session_failures: counters.mPskcSecureSessionFailures,
            pskc_commissioner_petitions: counters.mPskcCommissionerPetitions,
            mgmt_active_gets: counters.mMgmtActiveGets,
            mgmt_pending_gets: counters.mMgmtPendingGets,
        })
    }
}

/// Methods from the [OpenThread "Border Agent" Module][1].
///
/// [1]: https://openthread.io/reference/group/api-border-agent
pub trait BorderAgent {
    /// Functional equivalent of
    /// [`otsys::otBorderAgentIsActive`](crate::otsys::otBorderAgentIsActive).
    fn border_agent_is_active(&self) -> bool;

    /// Functional equivalent of
    /// [`otsys::otBorderAgentUdpPort`](crate::otsys::otBorderAgentGetUdpPort).
    fn border_agent_get_udp_port(&self) -> u16;

    /// Functional equivalent of
    /// [`otsys::otBorderAgentGetMeshCoPServiceTxtData`](crate::otsys::otBorderAgentGetMeshCoPServiceTxtData).
    fn border_agent_get_meshcop_service_txt_data(&self) -> Result<Vec<u8>>;

    /// Functional equivalent of
    /// [`otsys::otBorderAgentEphemeralKeyGetState`](crate::otsys::otBorderAgentEphemeralKeyGetState).
    fn border_agent_ephemeral_key_get_state(&self) -> BorderAgentEphemeralKeyState;

    /// Functional equivalent of
    /// [`otsys::otBorderAgentEphemeralKeySetEnabled`](crate::otsys::otBorderAgentEphemeralKeySetEnabled).
    fn border_agent_ephemeral_key_set_enabled(&self, enabled: bool);

    /// Functional equivalent of
    /// [`otsys::otBorderAgentEphemeralKeyStart`](crate::otsys::otBorderAgentEphemeralKeyStart).
    fn border_agent_ephemeral_key_start(
        &self,
        key_string: &CStr,
        timeout: u32,
        port: u16,
    ) -> Result;

    /// Functional equivalent of
    /// [`otsys::otBorderAgentEphemeralKeyStop`](crate::otsys::otBorderAgentEphemeralKeyStop).
    fn border_agent_ephemeral_key_stop(&self);

    /// Functional equivalent of
    /// [`otsys::otBorderAgentEphemeralKeyGetUdpPort`](crate::otsys::otBorderAgentEphemeralKeyGetUdpPort).
    fn border_agent_ephemeral_key_get_udp_port(&self) -> u16;

    /// Functional equivalent of
    /// [`otsys::otBorderAgentEphemeralKeySetCallback`](crate::otsys::otBorderAgentEphemeralKeySetCallback).
    fn border_agent_set_ephemeral_key_callback<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut() + 'a;

    /// Functional equivalent of
    /// [`otsys::otBorderAgentGetCounters`](crate::otsys::otBorderAgentGetCounters).
    fn border_agent_get_counters(&self) -> Option<BorderAgentCounters>;

    /// [`otsys::otBorderAgentSetMeshCoPServiceChangedCallback`](crate::otsys::otBorderAgentSetMeshCoPServiceChangedCallback).
    fn border_agent_set_meshcop_service_changed_fn<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut() + 'a;
}

impl<T: BorderAgent + Boxable> BorderAgent for ot::Box<T> {
    fn border_agent_is_active(&self) -> bool {
        self.as_ref().border_agent_is_active()
    }

    fn border_agent_get_udp_port(&self) -> u16 {
        self.as_ref().border_agent_get_udp_port()
    }

    fn border_agent_get_meshcop_service_txt_data(&self) -> Result<Vec<u8>> {
        self.as_ref().border_agent_get_meshcop_service_txt_data()
    }

    fn border_agent_ephemeral_key_get_state(&self) -> BorderAgentEphemeralKeyState {
        self.as_ref().border_agent_ephemeral_key_get_state()
    }

    fn border_agent_ephemeral_key_set_enabled(&self, enabled: bool) {
        self.as_ref().border_agent_ephemeral_key_set_enabled(enabled)
    }

    fn border_agent_ephemeral_key_start(&self, key: &CStr, timeout: u32, port: u16) -> Result {
        self.as_ref().border_agent_ephemeral_key_start(key, timeout, port)
    }

    fn border_agent_ephemeral_key_stop(&self) {
        self.as_ref().border_agent_ephemeral_key_stop()
    }

    fn border_agent_ephemeral_key_get_udp_port(&self) -> u16 {
        self.as_ref().border_agent_ephemeral_key_get_udp_port()
    }

    fn border_agent_set_ephemeral_key_callback<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut() + 'a,
    {
        self.as_ref().border_agent_set_ephemeral_key_callback(f)
    }

    fn border_agent_get_counters(&self) -> Option<BorderAgentCounters> {
        self.as_ref().border_agent_get_counters()
    }

    fn border_agent_set_meshcop_service_changed_fn<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut() + 'a,
    {
        self.as_ref().border_agent_set_meshcop_service_changed_fn(f)
    }
}

impl BorderAgent for Instance {
    fn border_agent_is_active(&self) -> bool {
        unsafe { otBorderAgentIsActive(self.as_ot_ptr()) }
    }

    fn border_agent_get_udp_port(&self) -> u16 {
        unsafe { otBorderAgentGetUdpPort(self.as_ot_ptr()) }
    }

    fn border_agent_get_meshcop_service_txt_data(&self) -> Result<Vec<u8>> {
        let mut txt_data = otBorderAgentMeshCoPServiceTxtData::default();
        let result: Result = Error::from(unsafe {
            otBorderAgentGetMeshCoPServiceTxtData(self.as_ot_ptr(), &mut txt_data)
        })
        .into();
        result?;
        Ok(txt_data.mData[..txt_data.mLength as usize].to_vec())
    }

    fn border_agent_ephemeral_key_get_state(&self) -> BorderAgentEphemeralKeyState {
        unsafe { otBorderAgentEphemeralKeyGetState(self.as_ot_ptr()).into() }
    }

    fn border_agent_ephemeral_key_set_enabled(&self, enabled: bool) {
        unsafe { otBorderAgentEphemeralKeySetEnabled(self.as_ot_ptr(), enabled) }
    }

    fn border_agent_ephemeral_key_start(&self, key: &CStr, timeout: u32, port: u16) -> Result {
        unsafe {
            Error::from(otBorderAgentEphemeralKeyStart(
                self.as_ot_ptr(),
                key.as_ptr(),
                timeout,
                port,
            ))
            .into()
        }
    }

    fn border_agent_ephemeral_key_stop(&self) {
        unsafe { otBorderAgentEphemeralKeyStop(self.as_ot_ptr()) }
    }

    fn border_agent_ephemeral_key_get_udp_port(&self) -> u16 {
        unsafe { otBorderAgentEphemeralKeyGetUdpPort(self.as_ot_ptr()) }
    }

    fn border_agent_set_ephemeral_key_callback<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut() + 'a,
    {
        unsafe extern "C" fn _border_agent_set_ephemeral_key_callback<'a, F: FnMut() + 'a>(
            context: *mut ::std::os::raw::c_void,
        ) {
            trace!("_border_agent_set_ephemeral_key_callback");

            // Reconstitute a reference to our closure.
            let sender = unsafe { &mut *(context as *mut F) };

            sender()
        }

        let (fn_ptr, fn_box, cb): (_, _, otBorderAgentEphemeralKeyCallback) = if let Some(f) = f {
            let mut x = Box::new(f);

            (
                x.as_mut() as *mut F as *mut ::std::os::raw::c_void,
                Some(x as Box<dyn FnMut() + 'a>),
                Some(_border_agent_set_ephemeral_key_callback::<F>),
            )
        } else {
            (std::ptr::null_mut() as *mut ::std::os::raw::c_void, None, None)
        };

        unsafe {
            otBorderAgentEphemeralKeySetCallback(self.as_ot_ptr(), cb, fn_ptr);

            // Make sure our object eventually gets cleaned up.
            // Here we must also transmute our closure to have a 'static lifetime.
            // We need to do this because the borrow checker cannot infer the
            // proper lifetime for the singleton instance backing, but
            // this is guaranteed by the API.
            self.borrow_backing().ephemeral_key_callback.set(std::mem::transmute::<
                Option<Box<dyn FnMut() + 'a>>,
                Option<Box<dyn FnMut() + 'static>>,
            >(fn_box));
        }
    }

    fn border_agent_get_counters(&self) -> Option<BorderAgentCounters> {
        unsafe { BorderAgentCounters::from_ot_counters(otBorderAgentGetCounters(self.as_ot_ptr())) }
    }

    fn border_agent_set_meshcop_service_changed_fn<'a, F>(&'a self, f: Option<F>)
    where
        F: FnMut() + 'a,
    {
        unsafe extern "C" fn _border_agent_set_meshcop_service_changed_callback<
            'a,
            F: FnMut() + 'a,
        >(
            context: *mut ::std::os::raw::c_void,
        ) {
            trace!("_border_agent_set_meshcop_service_changed_callback");

            // Reconstitute a reference to our closure.
            let sender = unsafe { &mut *(context as *mut F) };

            sender()
        }

        let (fn_ptr, fn_box, cb): (_, _, otBorderAgentMeshCoPServiceChangedCallback) =
            if let Some(f) = f {
                let mut x = Box::new(f);

                (
                    x.as_mut() as *mut F as *mut ::std::os::raw::c_void,
                    Some(x as Box<dyn FnMut() + 'a>),
                    Some(_border_agent_set_meshcop_service_changed_callback::<F>),
                )
            } else {
                (std::ptr::null_mut() as *mut ::std::os::raw::c_void, None, None)
            };

        unsafe {
            otBorderAgentSetMeshCoPServiceChangedCallback(self.as_ot_ptr(), cb, fn_ptr);

            // Make sure our object eventually gets cleaned up.
            // Here we must also transmute our closure to have a 'static lifetime.
            // We need to do this because the borrow checker cannot infer the
            // proper lifetime for the singleton instance backing, but
            // this is guaranteed by the API.
            self.borrow_backing().meshcop_service_changed_callback.set(std::mem::transmute::<
                Option<Box<dyn FnMut() + 'a>>,
                Option<Box<dyn FnMut() + 'static>>,
            >(fn_box));
        }
    }
}

/// Constructs a random key for use with ePSKc utilizing the algorithm from ot-br-posix.
///
/// [1]: https://github.com/openthread/ot-br-posix/blob/main/src/border_agent/border_agent.cpp
pub fn create_ephemeral_key() -> Result<CString, anyhow::Error> {
    let mut key: Vec<u8> = Vec::new();

    // Generate a sequence of integers from 0-9 with equal probability.
    for _ in 0..EPSKC_RANDOM_GEN_LEN {
        loop {
            let mut new_value: u8 = 0;
            let rand_result = unsafe { otRandomCryptoFillBuffer(&mut new_value as *mut u8, 1) };

            ot::Error::from(rand_result)
                .into_result()
                .map_err(|e| format_err!("Random number generation failed: {}", e))?;

            if new_value < 250 {
                key.push(b'0' + new_value % 10);
                break;
            }
        }
    }

    // Convert the decimal digits to a string.
    let key_string: String = key.iter().map(|digit| digit.to_string()).collect();

    // The final element in the key is a checksum.
    let mut checksum_char: c_char = 0;
    let decimal_string = CString::new(key_string)
        .map_err(|e| format_err!("Ephemeral key string contains a null byte:: {}", e))?;
    let checksum_result = unsafe {
        otVerhoeffChecksumCalculate(decimal_string.as_ptr(), &mut checksum_char as *mut c_char)
    };
    ot::Error::from(checksum_result)
        .into_result()
        .map_err(|e| format_err!("Verhoeff checksum calculation failed: {}", e))?;

    key.push(checksum_char as u8);
    CString::new(key).map_err(|e| format_err!("Ephemeral key is not a valid string: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_conversion_succeeds() {
        let epskc_activations = 0;
        let epskc_deactivation_clears = 1;
        let epskc_deactivation_timeouts = 2;
        let epskc_deactivation_max_attempts = 3;
        let epskc_deactivation_disconnects = 4;
        let epskc_invalid_ba_state_errors = 5;
        let epskc_invalid_args_errors = 6;
        let epskc_start_secure_session_errors = 7;
        let epskc_secure_session_successes = 8;
        let epskc_secure_session_failures = 9;
        let epskc_commissioner_petitions = 10;
        let pskc_secure_session_successes = 11;
        let pskc_secure_session_failures = 12;
        let pskc_commissioner_petitions = 13;
        let mgmt_active_gets = 14;
        let mgmt_pending_gets = 15;

        let ot_counters = otBorderAgentCounters {
            mEpskcActivations: epskc_activations,
            mEpskcDeactivationClears: epskc_deactivation_clears,
            mEpskcDeactivationTimeouts: epskc_deactivation_timeouts,
            mEpskcDeactivationMaxAttempts: epskc_deactivation_max_attempts,
            mEpskcDeactivationDisconnects: epskc_deactivation_disconnects,
            mEpskcInvalidBaStateErrors: epskc_invalid_ba_state_errors,
            mEpskcInvalidArgsErrors: epskc_invalid_args_errors,
            mEpskcStartSecureSessionErrors: epskc_start_secure_session_errors,
            mEpskcSecureSessionSuccesses: epskc_secure_session_successes,
            mEpskcSecureSessionFailures: epskc_secure_session_failures,
            mEpskcCommissionerPetitions: epskc_commissioner_petitions,
            mPskcSecureSessionSuccesses: pskc_secure_session_successes,
            mPskcSecureSessionFailures: pskc_secure_session_failures,
            mPskcCommissionerPetitions: pskc_commissioner_petitions,
            mMgmtActiveGets: mgmt_active_gets,
            mMgmtPendingGets: mgmt_pending_gets,
        };

        let ot_counters_ptr: *const otBorderAgentCounters = &ot_counters;

        let converted_counters = unsafe { BorderAgentCounters::from_ot_counters(ot_counters_ptr) }
            .expect("Failed to convert OT Border Agent counters");

        assert_eq!(
            converted_counters,
            BorderAgentCounters {
                epskc_activations,
                epskc_deactivation_clears,
                epskc_deactivation_timeouts,
                epskc_deactivation_max_attempts,
                epskc_deactivation_disconnects,
                epskc_invalid_ba_state_errors,
                epskc_invalid_args_errors,
                epskc_start_secure_session_errors,
                epskc_secure_session_successes,
                epskc_secure_session_failures,
                epskc_commissioner_petitions,
                pskc_secure_session_successes,
                pskc_secure_session_failures,
                pskc_commissioner_petitions,
                mgmt_active_gets,
                mgmt_pending_gets,
            }
        );
    }

    #[test]
    fn test_counter_conversion_fails() {
        let null_ptr: *const otBorderAgentCounters = std::ptr::null();
        assert!(unsafe { BorderAgentCounters::from_ot_counters(null_ptr) }.is_none());
    }
}
