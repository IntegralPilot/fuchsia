// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::socket_dispatcher::{SocketDispatcher, SocketDispatcherState};
use core::mem::MaybeUninit;
use zx_types::{zx_info_socket_t, zx_status_t};

unsafe extern "C" {
    pub(crate) fn cpp_socket_dispatcher_create(
        holder: *mut (),
        flags: u32,
        handle_out: *mut MaybeUninit<KernelHandle<SocketDispatcher>>,
    ) -> zx_status_t;
}

crate::object::dispatcher::impl_peered_dispatcher_state_init!(
    SocketDispatcher,
    SocketDispatcherState,
    flags: u32,
);

/// Returns the read threshold of the socket dispatcher.
///
/// # Safety
///
/// `disp` must be a valid reference to an initialized `SocketDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_socket_dispatcher_get_read_threshold(
    disp: &SocketDispatcher,
) -> usize {
    disp.get_read_threshold()
}

/// Sets the read threshold of the socket dispatcher.
///
/// # Safety
///
/// `disp` must be a valid reference to an initialized `SocketDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_socket_dispatcher_set_read_threshold(
    disp: &SocketDispatcher,
    value: usize,
) -> zx_status_t {
    disp.set_read_threshold(value).map_or_else(|s| s.into_raw(), |_| zx_types::ZX_OK)
}

/// Returns the write threshold of the socket dispatcher.
///
/// # Safety
///
/// `disp` must be a valid reference to an initialized `SocketDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_socket_dispatcher_get_write_threshold(
    disp: &SocketDispatcher,
) -> usize {
    disp.get_write_threshold()
}

/// Sets the write threshold of the socket dispatcher.
///
/// # Safety
///
/// `disp` must be a valid reference to an initialized `SocketDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_socket_dispatcher_set_write_threshold(
    disp: &SocketDispatcher,
    value: usize,
) -> zx_status_t {
    disp.set_write_threshold(value).map_or_else(|s| s.into_raw(), |_| zx_types::ZX_OK)
}

/// Returns socket buffer info for the socket dispatcher.
///
/// # Safety
///
/// `disp` must be a valid reference to an initialized `SocketDispatcher`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_socket_dispatcher_get_info(
    disp: &SocketDispatcher,
) -> zx_info_socket_t {
    disp.get_info()
}
