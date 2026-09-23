// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V 64 timer management (rdtime / SSTC / SBI timer).

use debug::dprintf;
use zx_status::Status;

use super::arch::{
    RISCV64_CSR_SIE, RISCV64_CSR_SIE_STIE, RISCV64_CSR_STIMECMP, RISCV64_CSR_TIME,
    arch_ints_disabled, riscv64_csr_clear, riscv64_csr_read, riscv64_csr_set, riscv64_csr_write,
};
use super::{feature, sbi};

/// Driver configuration passed from the ZBI.
pub use zbi::DcfgRiscvGenericTimerDriver;

const _: () = assert!(core::mem::size_of::<DcfgRiscvGenericTimerDriver>() == 8);
const _: () = assert!(core::mem::align_of::<DcfgRiscvGenericTimerDriver>() == 4);

unsafe extern "C" {
    fn cpp_timer_tick();
    fn cpp_timer_set_conversion_and_register(freq_hz: u32, initial_ticks: u64);
}

/// Read the current architectural timer ticks from the `time` CSR.
#[inline(always)]
pub fn riscv_sbi_current_ticks() -> i64 {
    // SAFETY: Reading the time CSR has no side effects and returns the hardware timebase counter.
    (unsafe { riscv64_csr_read::<RISCV64_CSR_TIME>() }) as i64
}

/// Program the next oneshot timer deadline in ticks.
pub fn riscv_sbi_set_oneshot_timer(mut deadline: i64) -> Result<(), Status> {
    debug_assert!(arch_ints_disabled());

    if deadline < 0 {
        deadline = 0;
    }

    // If the sstc feature is present, directly set the compare register instead of
    // making a call to SBI.
    if feature::has_sstc() {
        // SAFETY: Programming supervisor timer compare register for the local CPU.
        unsafe { riscv64_csr_write::<RISCV64_CSR_STIMECMP>(deadline as u64) };
    } else {
        sbi::sbi_set_timer(deadline as u64);
    }

    // Enable the timer interrupt.
    // SAFETY: Enabling supervisor timer interrupt in SIE CSR.
    unsafe { riscv64_csr_set::<RISCV64_CSR_SIE>(RISCV64_CSR_SIE_STIE) };

    Ok(())
}

/// Stop the hardware timer interrupt on the current CPU.
pub fn riscv_sbi_timer_stop() -> Result<(), Status> {
    // SAFETY: Disabling supervisor timer interrupt in SIE CSR.
    unsafe { riscv64_csr_clear::<RISCV64_CSR_SIE>(RISCV64_CSR_SIE_STIE) };
    Ok(())
}

/// Shutdown the hardware timer on the current CPU.
pub fn riscv_sbi_timer_shutdown() -> Result<(), Status> {
    debug_assert!(arch_ints_disabled());
    // SAFETY: Disabling supervisor timer interrupt in SIE CSR.
    unsafe { riscv64_csr_clear::<RISCV64_CSR_SIE>(RISCV64_CSR_SIE_STIE) };
    Ok(())
}

/// Interrupt handler called when a supervisor timer exception occurs.
#[unsafe(no_mangle)]
pub extern "C" fn riscv64_timer_exception() {
    // SAFETY: Acknowledging timer interrupt by masking STIE in SIE CSR.
    unsafe { riscv64_csr_clear::<RISCV64_CSR_SIE>(RISCV64_CSR_SIE_STIE) };
    // SAFETY: takes no arguments; runs the generic timer tick handler for this CPU.
    unsafe { cpp_timer_tick() };
}

static TIMER_INITIALIZED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Returns true if the timer driver has completed early initialization.
#[inline(always)]
pub fn timer_is_initialized() -> bool {
    TIMER_INITIALIZED.load(core::sync::atomic::Ordering::Relaxed)
}

/// Early initialization of the RISC-V generic timer driver.
///
/// # Safety
/// `config` must point at a live `DcfgRiscvGenericTimerDriver` for the duration
/// of the call. It comes from the driver configuration handed over by physboot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn riscv_generic_timer_init_early(
    config: *const DcfgRiscvGenericTimerDriver,
) {
    // SAFETY: the caller guarantees `config` points at a live driver config.
    let config = unsafe { &*config };
    let initial_ticks = riscv_sbi_current_ticks() as u64;
    dprintf!(INFO, "TIMER: registering SBI timer\n");
    TIMER_INITIALIZED.store(true, core::sync::atomic::Ordering::Release);
    // SAFETY: both arguments are plain integers; the callee registers the tick
    // conversion ratio and takes no pointers from this side.
    unsafe {
        cpp_timer_set_conversion_and_register(config.freq_hz, initial_ticks);
    }
}

// C FFI exports

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv_sbi_current_ticks() -> i64 {
    riscv_sbi_current_ticks()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv_sbi_set_oneshot_timer(deadline: i64) -> Result<(), Status> {
    riscv_sbi_set_oneshot_timer(deadline)
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv_sbi_timer_stop() -> Result<(), Status> {
    riscv_sbi_timer_stop()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv_sbi_timer_shutdown() -> Result<(), Status> {
    riscv_sbi_timer_shutdown()
}
