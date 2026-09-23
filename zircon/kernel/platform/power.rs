// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformHaltAction {
    /// Spin forever.
    Halt = 0,

    /// Reset the CPU.
    Reboot = 1,

    /// Reboot into the bootloader.
    RebootBootloader = 2,

    /// Reboot into the recovery partition.
    RebootRecovery = 3,

    /// Shutdown and power off.
    Shutdown = 4,
}

/// TODO(https://fxbug.dev/534467534): For now, these values are copied from zircon/system/public/zircon/boot/crash-reason.h.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZirconCrashReason {
    /// 0 is reserved for "Invalid".  It will never be used by a functioning
    /// crash-logger.
    Invalid = 0,

    /// "Unknown" indicates that the system does not know the reason for a recent
    /// crash.  The primary use of this reason is to be something which can be left
    /// in the crashlog in case the system spontaneously reboots without a chance to
    /// gracefully finalize the log, perhaps because of something like a hardware
    /// watchdog timer.
    Unknown = 1,

    /// "No Crash" indicates that the system deliberately rebooted in an
    /// orderly fashion.  No crash occurred.
    NoCrash = 2,

    /// "OOM" indicates a crash triggered by the system because of an unrecoverable
    /// out-of-memory situation.
    Oom = 3,

    /// "Panic" indicates a crash triggered by the system because of an unrecoverable
    /// kernel panic situation.
    Panic = 4,

    /// "Software watchdog" indicates a crash triggered by a kernel level software
    /// watchdog construct.  Note that this is distinct from a hardware based WDT.
    /// If the system reboots because of a hardware watchdog, it will have no chance
    /// to record the reboot reason, and the crashlog will indicate "unknown".  The
    /// HW reboot reason may be known, but only if the bootloader reports it to us.
    SoftwareWatchdog = 5,

    /// "Userspace root job termination" indicates a crash triggered by the system
    /// because it detected the termination of the userspace root job, most likely
    /// because one of its critical processes crashed.
    UserspaceRootJobTermination = 6,
}

unsafe extern "C" {
    fn cpp_platform_halt(action: u32, reason: u32) -> !;
    fn cpp_platform_panic_start();
}

/// Halts the platform.
#[inline]
pub fn platform_halt(action: PlatformHaltAction, reason: ZirconCrashReason) -> ! {
    unsafe { cpp_platform_halt(action as u32, reason as u32) }
}

/// Prepares the platform to print a panic: stops the other CPUs and switches the
/// console to a mode that works without them.
#[inline]
pub fn platform_panic_start() {
    // SAFETY: no preconditions; safe to call from any CPU at any time.
    unsafe { cpp_platform_panic_start() }
}
