// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::HandleValue;
use crate::user_copy::{UserInPtr, UserOutPtr};
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{zx_smc_parameters_t, zx_smc_result_t};

const ARM_SMC_SERVICE_CALL_NUM_MASK: u32 = 0x3F;
const ARM_SMC_SERVICE_CALL_NUM_SHIFT: u32 = 24;

/// Extracts the service call number from an ARM SMC function ID.
pub const fn arm_smc_get_service_call_num_from_func_id(func_id: u32) -> u32 {
    (func_id >> ARM_SMC_SERVICE_CALL_NUM_SHIFT) & ARM_SMC_SERVICE_CALL_NUM_MASK
}

// Fast Call: ARM SMCCC w0 encodes the function ID.
// ARM SMCCC w0[31] is 1 for Fast Calls.
#[inline]
pub const fn is_smccc_fast_call(function_id: u32) -> bool {
    (function_id & (1 << 31)) != 0
}

#[cfg(target_arch = "aarch64")]
mod arch {
    use super::*;
    use crate::counters::define_kcounter;
    use boot_options::BootOptions;

    define_kcounter!(ARM_SMCCC_FAST_CALLS, "arm_smccc.fast_calls", Sum);
    define_kcounter!(ARM_SMCCC_YIELDING_CALLS, "arm_smccc.yielding_calls", Sum);
    define_kcounter!(ARM_SMCCC_QCOM_INTERRUPTED, "arm_smccc.qcom.interrupted", Sum);

    /// Low-level result structure from ARM SMCCC calls matching `arm_smccc_result_t`.
    #[repr(C)]
    #[derive(Debug, Copy, Clone, Default, Eq, PartialEq)]
    pub struct ArmSmcccResult {
        pub x0: u64,
        pub x1: u64,
        pub x2: u64,
        pub x3: u64,
        pub x6: u64,
    }

    zr::static_assert!(core::mem::size_of::<ArmSmcccResult>() == 40);
    zr::static_assert!(core::mem::align_of::<ArmSmcccResult>() == 8);

    unsafe extern "C" {
        /// Calls the low-level SMC assembly routine (`arm_smccc_smc_internal`).
        ///
        /// # Safety
        ///
        /// The caller must ensure that the function ID and argument registers conform to
        /// the ARM SMCCC specification and are safe to execute at the secure monitor level.
        pub fn arm_smccc_smc_internal(
            w0: u32,
            x1: u64,
            x2: u64,
            x3: u64,
            x4: u64,
            x5: u64,
            x6: u64,
            w7: u32,
        ) -> ArmSmcccResult;
    }

    /// Calls the low-level SMC function.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn arm_smccc_smc(
        w0: u32,
        x1: u64,
        x2: u64,
        x3: u64,
        x4: u64,
        x5: u64,
        x6: u64,
        w7: u32,
    ) -> ArmSmcccResult {
        // SAFETY: Direct call to assembly SMC routine. The calling convention preserves
        // required registers and returns results in x0-x3, x6.
        unsafe { arm_smccc_smc_internal(w0, x1, x2, x3, x4, x5, x6, w7) }
    }

    ksync::declare_singleton_mutex!(QcomSmcLock);

    pub fn smc_call(params: &zx_smc_parameters_t) -> Result<zx_smc_result_t, Status> {
        let client_and_secure_os_id =
            ((params.secure_os_id as u32) << 16) | (params.client_id as u32);
        let arm_result: ArmSmcccResult;

        if is_smccc_fast_call(params.func_id) {
            ARM_SMCCC_FAST_CALLS.add(1);
            // TODO(74553): Detect when SMC calls take too long
            arm_result = arm_smccc_smc(
                params.func_id,
                params.arg1,
                params.arg2,
                params.arg3,
                params.arg4,
                params.arg5,
                params.arg6,
                client_and_secure_os_id,
            );
        } else {
            ARM_SMCCC_YIELDING_CALLS.add(1);
            if BootOptions::get().arm64_smccc_qcom {
                ksync::lock!(QcomSmcLock::Get().lock());
                let mut current_result = arm_smccc_smc(
                    params.func_id,
                    params.arg1,
                    params.arg2,
                    params.arg3,
                    params.arg4,
                    params.arg5,
                    params.arg6,
                    client_and_secure_os_id,
                );
                const INTERRUPTED: u64 = 0x1;
                // When interrupted:
                //  * `arm_result.x0` will be `kInterrupted`.
                //  * `arm_result.x6` will contain a session ID.
                //
                // In order to resume, a SMC must be issued with:
                //  * `w0`(function ID) must be `kInterrupted`.
                //  * `x6` must reuse the session ID returned earlier.
                while current_result.x0 == INTERRUPTED {
                    ARM_SMCCC_QCOM_INTERRUPTED.add(1);
                    current_result = arm_smccc_smc(
                        INTERRUPTED as u32,
                        params.arg1,
                        params.arg2,
                        params.arg3,
                        params.arg4,
                        params.arg5,
                        current_result.x6,
                        client_and_secure_os_id,
                    );
                }
                arm_result = current_result;
            } else {
                arm_result = arm_smccc_smc(
                    params.func_id,
                    params.arg1,
                    params.arg2,
                    params.arg3,
                    params.arg4,
                    params.arg5,
                    params.arg6,
                    client_and_secure_os_id,
                );
            }
        }

        Ok(zx_smc_result_t {
            arg0: arm_result.x0,
            arg1: arm_result.x1,
            arg2: arm_result.x2,
            arg3: arm_result.x3,
            arg6: arm_result.x6,
        })
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod arch {
    use super::*;

    pub fn smc_call(_params: &zx_smc_parameters_t) -> Result<zx_smc_result_t, Status> {
        Err(Status::NOT_SUPPORTED)
    }
}

#[syscall]
pub fn sys_smc_call(
    handle: HandleValue,
    parameters: UserInPtr<zx_smc_parameters_t>,
    out_smc_result: UserOutPtr<zx_smc_result_t>,
) -> Result<(), Status> {
    if parameters.is_null() || out_smc_result.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    let mut uninit_params = core::mem::MaybeUninit::uninit();
    let params = parameters.copy_from_user(&mut uninit_params)?;

    let service_call_num = arm_smc_get_service_call_num_from_func_id(params.func_id);
    crate::object::validate_ranged_resource(
        handle,
        zx_types::ZX_RSRC_KIND_SMC,
        service_call_num as u64,
        1,
    )?;

    let result = arch::smc_call(params)?;
    out_smc_result.write(result)?;
    Ok(())
}

#[cfg(ktest)]
#[unittest::suite(name = "smc_rust")]
/// Unit tests for SMC syscall and ARM SMCCC helpers.
mod tests {
    use super::{arm_smc_get_service_call_num_from_func_id, is_smccc_fast_call};
    use unittest::{assert_eq, expect_true};

    /// Tests bitmask extraction for SMC service call number.
    #[test]
    fn test_service_call_num_extraction() {
        assert_eq!(arm_smc_get_service_call_num_from_func_id(0x0200_0000), 0x02);
        assert_eq!(arm_smc_get_service_call_num_from_func_id(0x3200_0000), 0x32);
        assert_eq!(arm_smc_get_service_call_num_from_func_id(0x3F00_0000), 0x3F);
        assert_eq!(arm_smc_get_service_call_num_from_func_id(0xFF00_0000), 0x3F);
        assert_eq!(arm_smc_get_service_call_num_from_func_id(0x0000_0000), 0x00);
    }

    /// Tests identification of SMCCC fast calls via function ID bit 31.
    #[test]
    fn test_is_smccc_fast_call() {
        expect_true!(is_smccc_fast_call(0x8000_0000));
        expect_true!(!is_smccc_fast_call(0x0000_0000));
    }
}
