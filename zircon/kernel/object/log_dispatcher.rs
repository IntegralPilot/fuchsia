// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use crate::debuglog_rs::{DlogReaderStorage, dlog_record_t, dlog_write};
use fbl::Canary;
use ksync::{KMutex, RawCriticalMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_LOG_FLAG_READABLE, ZX_LOG_READABLE, ZX_OBJ_TYPE_DEBUGLOG, ZX_RIGHT_DUPLICATE,
    ZX_RIGHT_INSPECT, ZX_RIGHT_READ, ZX_RIGHT_SIGNAL, ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT,
    ZX_RIGHT_WRITE, zx_rights_t,
};

use super::log_dispatcher_ffi::{cpp_log_dispatcher_create, rust_log_dispatcher_notify};
use super::{DispatcherOps, KernelHandle};

use object_constants_rs as object_constants;

const ZX_DEFAULT_LOG_READ_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_READ
    | ZX_RIGHT_WRITE
    | ZX_RIGHT_SIGNAL
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT;

const ZX_DEFAULT_LOG_WRITE_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_WRITE
    | ZX_RIGHT_SIGNAL
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT;

zr::static_assert_size_and_align!(
    LogDispatcherState,
    object_constants::kLogDispatcherStateSize,
    object_constants::kLogDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_LOG_CREATE_COUNT, "dispatcher.log.create", Sum);
define_kcounter!(DISPATCHER_LOG_DESTROY_COUNT, "dispatcher.log.destroy", Sum);

#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct LogDispatcherState {
    canary: Canary<{ fbl::magic(b"LOGD") }>,

    #[pin]
    #[guarded_by(lock)]
    pub reader: DlogReaderStorage,

    flags: u32,

    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl LogDispatcherState {
    pub fn init(
        dispatcher: *const LogDispatcher,
        flags: u32,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_LOG_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            flags,
            lock <- KMutex::init(),
            reader <- ksync::kcell_init(
                // SAFETY: `dispatcher` points to the valid `LogDispatcher` enclosing this state.
                unsafe {
                    DlogReaderStorage::init(
                        *flags,
                        rust_log_dispatcher_notify,
                        dispatcher.cast_mut().cast(),
                    )
                }
            ),
        })
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }
}

#[pinned_drop]
impl PinnedDrop for LogDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        DISPATCHER_LOG_DESTROY_COUNT.add(1);
        let this = self.project();
        if (*this.flags & ZX_LOG_FLAG_READABLE) != 0 {
            let reader_pin = this.reader.get_pinned_mut();
            reader_pin.disconnect();
        }
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    pub struct LogDispatcher,
    LogDispatcherState,
    ZX_OBJ_TYPE_DEBUGLOG,
    object_constants::kLogDispatcherStateOffset
);

impl LogDispatcher {
    /// Returns default rights for a LogDispatcher given creation `flags`.
    pub fn default_rights(flags: u32) -> zx_rights_t {
        if (flags & ZX_LOG_FLAG_READABLE) != 0 {
            ZX_DEFAULT_LOG_READ_RIGHTS
        } else {
            ZX_DEFAULT_LOG_WRITE_RIGHTS
        }
    }

    /// Creates a new LogDispatcher and returns its kernel handle and rights.
    pub fn create(flags: u32) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let rights = Self::default_rights(flags);
        // SAFETY: `cpp_log_dispatcher_create` initializes `handle` on success.
        let handle =
            unsafe { KernelHandle::create(|out| cpp_log_dispatcher_create(flags, rights, out)) }?;
        Ok((handle, rights))
    }

    /// Writes `bytes` to debuglog with given options/flags.
    pub fn write(&self, severity: u32, flags: u32, bytes: &[u8]) -> Result<(), Status> {
        dlog_write(severity, self.state().flags() | flags, bytes)
    }

    /// Reads a record from debuglog.
    pub fn read(
        &self,
        flags: u32,
        record: &mut dlog_record_t,
        actual: &mut usize,
    ) -> Result<(), Status> {
        if (self.state().flags() & ZX_LOG_FLAG_READABLE) == 0 {
            return Err(Status::BAD_STATE);
        }

        ksync::lock!(let mut guard = self.state().lock_lock());
        let mut fields = guard.as_mut().fields_mut();
        let res = fields.reader.as_mut().read(flags, record, actual);
        if res == Err(Status::SHOULD_WAIT) {
            self.update_state_locked(guard.token(), ZX_LOG_READABLE, 0);
        }
        res
    }
}
