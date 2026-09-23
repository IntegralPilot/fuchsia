// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Fuchsia-specific `libc` functionality.
//!
//! This crate is a minimal extension on top of the `fuchsia-zircon` crate,
//! which provides bindings to the Zircon kernel's syscalls, but does not
//! depend on functionality from `libc`.

// AKA `libc`-granted ambient-authority crate ;)

#![deny(missing_docs)]

use num_derive::FromPrimitive;
use num_traits::cast::FromPrimitive;
use thiserror::Error;
use zx::sys::{ZX_HANDLE_INVALID, zx_handle_t, zx_status_t};
use zx::{
    BootTimeline, Clock, ClockDetails, ClockTransformation, ClockUpdate, Duration, Instant, Job,
    NullableHandle, Process, Rights, Status, Thread, Timeline, Unowned, Vmar,
};

// TODO(https://fxbug.dev/42139436): Document these.
#[allow(missing_docs)]
unsafe extern "C" {
    pub fn dl_clone_loader_service(out: *mut zx_handle_t) -> zx_status_t;
    pub fn zx_take_startup_handle(hnd_info: u32) -> zx_handle_t;
    pub fn zx_thread_self() -> zx_handle_t;
    pub fn zx_process_self() -> zx_handle_t;
    pub fn thrd_get_zx_process() -> zx_handle_t;
    pub fn zx_vmar_root_self() -> zx_handle_t;
    pub fn zx_job_default() -> zx_handle_t;
}

/// Handle types as defined by the processargs protocol.
///
/// See [//zircon/system/public/zircon/processargs.h][processargs.h] for canonical definitions.
///
/// Short descriptions of each handle type are given, but more complete documentation may be found
/// in the [processargs.h] header.
///
/// [processargs.h]: https://fuchsia.googlesource.com/fuchsia/+/HEAD/zircon/system/public/zircon/processargs.h
#[repr(u8)]
#[derive(FromPrimitive, Copy, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HandleType {
    /// Handle to our own process.
    ///
    /// Equivalent to PA_PROC_SELF.
    ProcessSelf = 0x01,

    /// Handle to the initial thread of our own process.
    ///
    /// Equivalent to PA_THREAD_SELF.
    ThreadSelf = 0x02,

    /// Handle to a job object which can be used to make child processes.
    ///
    /// The job can be the same as the one used to create this process or it can
    /// be different.
    ///
    /// Equivalent to PA_JOB_DEFAULT.
    DefaultJob = 0x03,

    /// Handle to the root of our address space.
    ///
    /// Equivalent to PA_VMAR_ROOT.
    RootVmar = 0x04,

    /// Handle to the VMAR used to load the initial program image.
    ///
    /// Equivalent to PA_VMAR_LOADED.
    LoadedVmar = 0x05,

    /// Service for loading shared libraries.
    ///
    /// See `fuchsia.ldsvc.Loader` for the interface definition.
    ///
    /// Equivalent to PA_LDSVC_LOADER.
    LdsvcLoader = 0x10,

    /// Handle to the VMO containing the vDSO ELF image.
    ///
    /// Equivalent to PA_VMO_VDSO.
    VdsoVmo = 0x11,

    /// Handle to the VMO used to map the initial thread's stack.
    ///
    /// Equivalent to PA_VMO_STACK.
    StackVmo = 0x13,

    /// Handle to the VMO for the main executable file.
    ///
    /// Equivalent to PA_VMO_EXECUTABLE.
    ExecutableVmo = 0x14,

    /// Used by kernel and userboot during startup.
    ///
    /// Equivalent to PA_VMO_BOOTDATA.
    BootdataVmo = 0x1A,

    /// Used by kernel and userboot during startup.
    ///
    /// Equivalent to PA_VMO_BOOTFS.
    BootfsVmo = 0x1B,

    /// Used by the kernel to export debug information as a file in bootfs.
    ///
    /// Equivalent to PA_VMO_KERNEL_FILE.
    KernelFileVmo = 0x1C,

    /// A Handle to a component's process' configuration VMO.
    ///
    /// Equivalent to PA_VMO_COMPONENT_CONFIG.
    ComponentConfigVmo = 0x1D,

    /// A handle to a fuchsia.io.Directory service to be used as a directory in the process's
    /// namespace. Corresponds to a path in the processargs bootstrap message's namespace table
    /// based on the argument of a HandleInfo of this type.
    ///
    /// Equivalent to PA_NS_DIR.
    NamespaceDirectory = 0x20,

    /// A handle which will be used as a file descriptor.
    ///
    /// Equivalent to PA_FD.
    FileDescriptor = 0x30,

    /// A Handle to a channel on which the process may serve the
    /// the |fuchsia.process.Lifecycle| protocol.
    ///
    /// Equivalent to PA_LIFECYCLE.
    Lifecycle = 0x3A,

    /// Server endpoint for handling connections to a process's outgoing directory.
    ///
    /// Equivalent to PA_DIRECTORY_REQUEST.
    DirectoryRequest = 0x3B,

    /// A |fuchsia.component.sandbox/Dictionary| client endpoint where the process
    /// may find a dictionary that it has stowed away earlier via
    /// |fuchsia.process.Lifecycle/OnEscrow|.
    ///
    /// Equivalent to PA_ESCROWED_DICTIONARY.
    EscrowedDictionary = 0x3C,

    /// A `fuchsia.logger.LogSink` channel handle.
    ///
    /// Equivalent to PA_LOG_SINK.
    LogSink = 0x3D,

    /// A Handle to a resource object. Used by devcoordinator and devhosts.
    ///
    /// Equivalent to PA_RESOURCE.
    Resource = 0x3F,

    /// A Handle to a clock object representing UTC.  Used by runtimes to gain
    /// access to UTC time.
    ///
    /// Equivalent to PA_CLOCK_UTC.
    ClockUtc = 0x40,

    /// A Handle to an MMIO resource object.
    ///
    /// Equivalent to PA_MMIO_RESOURCE.
    MmioResource = 0x50,

    /// A Handle to an IRQ resource object.
    ///
    /// Equivalent to PA_IRQ_RESOURCE.
    IrqResource = 0x51,

    /// A Handle to an IO Port resource object.
    ///
    /// Equivalent to PA_IOPORT_RESOURCE.
    IoportResource = 0x52,

    /// A Handle to an SMC resource object.
    ///
    /// Equivalent to PA_SMC_RESOURCE.
    SmcResource = 0x53,

    /// A Handle to the System resource object.
    ///
    /// Equivalent to PA_SYSTEM_RESOURCE.
    SystemResource = 0x54,

    /// A handle type with user-defined meaning.
    ///
    /// Equivalent to PA_USER0.
    User0 = 0xF0,

    /// A handle type with user-defined meaning.
    ///
    /// Equivalent to PA_USER1.
    User1 = 0xF1,

    /// A handle type with user-defined meaning.
    ///
    /// Equivalent to PA_USER2.
    User2 = 0xF2,
}

/// Metadata information for a handle in a processargs message. Contains a handle type and an
/// unsigned 16-bit value, whose meaning is handle type dependent.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct HandleInfo {
    htype: HandleType,
    arg: u16,
}

impl HandleInfo {
    /// Create a handle info struct from a handle type and an argument.
    ///
    /// For example, a `HandleInfo::new(HandleType::FileDescriptor, 32)` identifies
    /// the respective handle as file descriptor 32.
    ///
    /// Corresponds to PA_HND in processargs.h.
    pub const fn new(htype: HandleType, arg: u16) -> Self {
        HandleInfo { htype, arg }
    }

    /// Returns the handle type for this handle info struct.
    #[inline(always)]
    pub fn handle_type(&self) -> HandleType {
        self.htype
    }

    /// Returns the argument for this handle info struct.
    #[inline(always)]
    pub fn arg(&self) -> u16 {
        self.arg
    }

    /// Convert the handle info into a raw u32 value for FFI purposes.
    pub const fn as_raw(&self) -> u32 {
        ((self.htype as u32) & 0xFF) | (self.arg as u32) << 16
    }
}

/// An implementation of the From trait to create a [HandleInfo] from a [HandleType] with an argument
/// of 0.
impl From<HandleType> for HandleInfo {
    fn from(ty: HandleType) -> Self {
        Self::new(ty, 0)
    }
}

/// Possible errors when converting a raw u32 to a HandleInfo with the  TryFrom<u32> impl on
/// HandleInfo.
#[derive(Error, Debug)]
pub enum HandleInfoError {
    /// Unknown handle type.
    #[error("Unknown handle type for HandleInfo: {:#x?}", _0)]
    UnknownHandleType(u32),

    /// Otherwise invalid raw value, like reserved bytes being non-zero.
    #[error("Invalid value for HandleInfo: {:#x?}", _0)]
    InvalidHandleInfo(u32),
}

impl TryFrom<u32> for HandleInfo {
    type Error = HandleInfoError;

    /// Attempt to convert a u32 to a handle ID value. Can fail if the value represents an
    /// unknown handle type or is otherwise invalid.
    ///
    /// Useful to convert existing handle info values received through FIDL APIs, e.g. from a
    /// client that creates them using the PA_HND macro in processargs.h from C/C++.
    fn try_from(value: u32) -> Result<HandleInfo, HandleInfoError> {
        // 2nd byte should be zero, it is currently unused.
        if value & 0xFF00 != 0 {
            return Err(HandleInfoError::InvalidHandleInfo(value));
        }

        let htype = HandleType::from_u8((value & 0xFF) as u8)
            .ok_or(HandleInfoError::UnknownHandleType(value))?;
        Ok(HandleInfo::new(htype, (value >> 16) as u16))
    }
}

/// Removes the handle of type `HandleType` from the list of handles received at startup.
///
/// This function will return `Some` at-most once per handle type.
/// This function will return `None` if the requested type was not received at
/// startup or if the handle with the provided type was already taken.
pub fn take_startup_handle(info: HandleInfo) -> Option<NullableHandle> {
    unsafe {
        let raw = zx_take_startup_handle(info.as_raw());
        if raw == ZX_HANDLE_INVALID { None } else { Some(NullableHandle::from_raw(raw)) }
    }
}

/// Provides temporary scoped access to the current thread's handle.
///
/// Callers who need to store a thread handle or to send it to another thread should duplicate the
/// handle to ensure it can outlive the current thread.
pub fn with_thread_self<F, R>(f: F) -> R
where
    F: for<'a> FnOnce(&Thread) -> R,
{
    // SAFETY: The handle returned by zx_thread_self() is guaranteed to be valid for the duration of
    // the calling thread's execution and by creating this on our stack we prevent it from outliving
    // this thread.
    let thread = unsafe { Unowned::from_raw_handle(zx_thread_self()) };
    f(&thread)
}

/// Get a reference to the handle of the current process.
pub fn process_self() -> Unowned<'static, Process> {
    unsafe {
        // zx_process_self() doesn't work correctly in jobs where multiple processes share
        // the portion of their address space for global variables. Use thrd_get_zx_process() to
        // return the correct value in that context. See https://fxbug.dev/42083701 for background.
        let handle = thrd_get_zx_process();
        Unowned::from_raw_handle(handle)
    }
}

/// Get a reference to the handle of the current address space.
pub fn vmar_root_self() -> Unowned<'static, Vmar> {
    unsafe {
        let handle = zx_vmar_root_self();
        Unowned::from_raw_handle(handle)
    }
}

/// Get a reference to the fuchsia.ldsvc.Loader channel.
pub fn loader_svc() -> Result<NullableHandle, Status> {
    unsafe {
        let mut handle: zx_handle_t = 0;
        let status = dl_clone_loader_service(&mut handle);
        Status::ok(status)?;
        Ok(NullableHandle::from_raw(handle))
    }
}

/// Get a reference to the default `Job` provided to the process on startup.
///
/// This typically refers to the `Job` that is the immediate parent of the current
/// process.
///
/// If the current process was launched as a Fuchsia Component, this `Job`
/// will begin with no child processes other than the current process.
pub fn job_default() -> Unowned<'static, Job> {
    unsafe {
        let handle = zx_job_default();
        Unowned::from_raw_handle(handle)
    }
}

/// Marker type for the UTC timeline.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct UtcTimeline;
impl Timeline for UtcTimeline {}

/// A UTC timestamp, measured in nanoseconds since Jan 1 1970.
pub type UtcInstant = Instant<UtcTimeline>;

/// A duration in the UTC timeline.
pub type UtcDuration = Duration<UtcTimeline>;

/// Special methods that are only valid for UtcDuration.
pub trait UtcDurationExt {
    /// Converts the reading of a duration on the UTC timeline to a reading on the boot timeline.
    ///
    /// This is "lossy" because durations are usually differences of
    /// two Instants, and Boot time and UTC time mostly tick at the same rate.
    /// However, they aren't required to tick in lock-step.
    fn to_boot_lossy(&self) -> zx::Duration<BootTimeline>;
}

impl UtcDurationExt for UtcDuration {
    fn to_boot_lossy(&self) -> zx::Duration<BootTimeline> {
        zx::Duration::from_nanos(self.into_nanos())
    }
}

/// Special methods that are only valid for zx::BootDuration.
pub trait BootDurationExt {
    /// Converts the reading of a duration on the boot timeline to a reading on the UTC timeline.
    ///
    /// See notes alongside [UtcDurationExt::to_boot_lossy].
    fn to_utc_lossy(&self) -> UtcDuration;
}

impl BootDurationExt for zx::BootDuration {
    fn to_utc_lossy(&self) -> UtcDuration {
        zx::Duration::from_nanos(self.into_nanos())
    }
}

/// A clock that will return UTC timestamps.
pub type UtcClock = Clock<BootTimeline, UtcTimeline>;

/// Details of a UTC clock.
pub type UtcClockDetails = ClockDetails<BootTimeline, UtcTimeline>;

/// A transformation for the UTC clock.
pub type UtcClockTransform = ClockTransformation<BootTimeline, UtcTimeline>;

/// An update for the UTC clock.
pub type UtcClockUpdate = ClockUpdate<BootTimeline, UtcTimeline>;

fn utc_clock() -> Unowned<'static, UtcClock> {
    zx_libc::utc::reference_get()
}

/// Duplicate the UTC `Clock` registered with the runtime.
pub fn duplicate_utc_clock_handle(rights: Rights) -> Result<UtcClock, Status> {
    utc_clock().duplicate_handle(rights)
}

/// Swaps the current process-global UTC clock with `new_clock`, returning
/// the old clock on success.
/// If `new_clock` is a valid handle but does not have the ZX_RIGHT_READ right,
/// an error is returned and `new_clock` is dropped.
pub fn swap_utc_clock_handle(new_clock: UtcClock) -> Result<UtcClock, Status> {
    zx_libc::utc::reference_swap(new_clock)
}

/// SUBTLE, read the detailed documentation before using this function.
///
/// Reads time from the UTC `Clock` registered with the runtime.
///
/// # Subtleties
///
/// If you intend to use this timestamp in any user-facing capacity, you must
/// ensure that a sensible thing is displayed to the user. Specifically:
///
/// * If you want to display the value, make sure to first check whether the
///   UTC clock is even started. If not started, Fuchsia's UTC will [continuously
///   indicate backstop time][bt]. This is a Fuchsia-only behavior, disregarding
///   this behavior may present a confusing outcome to your user.
/// * If you want to use this value in arithmetic calculations, never roll your
///   own, especially if you are combining values from different timelines.
/// * If you must use this value in arithmetic calculations and must combine
///   values from different timelines, doublecheck that you are using the correct
///   other timeline.
/// * If in the end, you want to format the value as user readable date-time,
///   never roll your own. Use the ICU library or equivalent chronological
///   library of your choice.
/// * **In testing only.** Due to above peculiarities, we often provide fake
///   UTC clocks to test fixtures. This does not happen automatically, you must
///   configure your test fixture to do this. When reading UTC time in a test
///   especially if you want to compare across components, doublecheck that all
///   components are using the same UTC clock by koid.
///
/// # Checking whether the clock is started
///
/// You can check this property in a few ways:
///
/// * You can call `utc_clock().get_details()`, which will return a `UtcClockDetails`.
///   You can check that the clock rates reported are not zero. Requires
///   `zx::Rights::INSPECT` on the clock handle for `get_details()`.
/// * You can call `utc_clock().read()`, which will return an `UtcInstant` value.
///   If the clock is not started, this will return a value that is exactly the
///   value of UTC backstop time. This is more straightforward but requires you
///   to read the clock and its details, which is more work than you may want.
///
/// # Panics
///
/// Panics if there is no UTC clock registered with the runtime or the registered handle does not
/// have the required rights.
///
/// [bt]: https://fuchsia.dev/fuchsia-src/concepts/kernel/time/utc/behavior
#[inline]
pub fn utc_time() -> UtcInstant {
    utc_clock().read().expect("Failed to read UTC clock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_id() {
        let mut randbuf = [0; 2];
        for type_val in 0..0xFF {
            if let Some(htype) = HandleType::from_u8(type_val as u8) {
                zx::cprng_draw(&mut randbuf);
                let arg = u16::from_le_bytes(randbuf);

                let info = HandleInfo::new(htype, arg);
                assert_eq!(info.handle_type(), htype);
                assert_eq!(info.arg(), arg);

                let info = HandleInfo::from(htype);
                assert_eq!(info.handle_type(), htype);
                assert_eq!(info.arg(), 0);
            }
        }
    }

    #[test]
    fn handle_id_raw() {
        assert_eq!(HandleInfo::new(HandleType::ProcessSelf, 0).as_raw(), 0x00000001);
        assert_eq!(HandleInfo::new(HandleType::DirectoryRequest, 0).as_raw(), 0x0000003B);
        assert_eq!(HandleInfo::new(HandleType::LdsvcLoader, 0xABCD).as_raw(), 0xABCD0010);
        assert_eq!(HandleInfo::new(HandleType::User0, 0x1).as_raw(), 0x000100F0);
        assert_eq!(HandleInfo::new(HandleType::User1, 0xABCD).as_raw(), 0xABCD00F1);
        assert_eq!(HandleInfo::new(HandleType::User2, 0xFFFF).as_raw(), 0xFFFF00F2);
    }

    #[test]
    fn handle_id_from_u32() {
        assert_eq!(
            HandleInfo::try_from(0x00000002).unwrap(),
            HandleInfo::new(HandleType::ThreadSelf, 0)
        );
        assert_eq!(
            HandleInfo::try_from(0x00040030).unwrap(),
            HandleInfo::new(HandleType::FileDescriptor, 4)
        );
        assert_eq!(
            HandleInfo::try_from(0x501C00F2).unwrap(),
            HandleInfo::new(HandleType::User2, 0x501C)
        );

        // Non-zero unused byte
        assert!(HandleInfo::try_from(0x00001100).is_err());
        assert!(HandleInfo::try_from(0x00010101).is_err());
        // Unknown handle type
        assert!(HandleInfo::try_from(0x00000000).is_err());
        assert!(HandleInfo::try_from(0x00000006).is_err());
    }

    #[test]
    fn read_utc_time() {
        utc_time();
    }

    #[test]
    fn get_loader_svc() {
        loader_svc().unwrap();
    }
}
