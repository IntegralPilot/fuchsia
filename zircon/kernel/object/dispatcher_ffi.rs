// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::dispatcher::Dispatcher;

unsafe extern "C" {
    pub(crate) fn cpp_dispatcher_on_zero_handles(dispatcher: *const Dispatcher);
    pub(crate) fn cpp_dispatcher_clear_signals(
        dispatcher: *const Dispatcher,
        signals: zx_types::zx_signals_t,
    );
    pub(crate) fn cpp_dispatcher_update_state(
        dispatcher: *const Dispatcher,
        clear_mask: u32,
        set_mask: u32,
        strobe_mask: u32,
    );
    pub(crate) fn cpp_dispatcher_update_state_locked(
        dispatcher: *const Dispatcher,
        clear_mask: u32,
        set_mask: u32,
        strobe_mask: u32,
    );
    pub(crate) fn cpp_dispatcher_signals_state_locked(
        dispatcher: *const Dispatcher,
    ) -> zx_types::zx_signals_t;
    pub(crate) fn cpp_dispatcher_get_ref_counted(
        dispatcher: *const Dispatcher,
    ) -> *mut core::ffi::c_void;
    pub(crate) fn cpp_dispatcher_get_type(dispatcher: *const Dispatcher)
    -> zx_types::zx_obj_type_t;
    pub(crate) fn cpp_dispatcher_get_koid(dispatcher: *const Dispatcher) -> zx_types::zx_koid_t;
    pub(crate) fn cpp_dispatcher_recycle(dispatcher: *const Dispatcher);
    pub(crate) fn cpp_dispatcher_get_related_koid(
        dispatcher: *const Dispatcher,
    ) -> zx_types::zx_koid_t;
    pub(crate) fn cpp_dispatcher_add_observer(
        dispatcher: *const Dispatcher,
        observer: *mut core::ffi::c_void,
        handle: *const core::ffi::c_void,
        signals: zx_types::zx_signals_t,
    ) -> zx_types::zx_status_t;
    pub(crate) fn cpp_dispatcher_remove_observer(
        dispatcher: *const Dispatcher,
        observer: *mut core::ffi::c_void,
        out_signals: *mut zx_types::zx_signals_t,
    ) -> bool;
    pub(crate) fn cpp_dispatcher_get_name(
        dispatcher: *const Dispatcher,
        out_name: *mut core::ffi::c_char,
    ) -> zx_types::zx_status_t;
    pub(crate) fn cpp_dispatcher_set_name(
        dispatcher: *const Dispatcher,
        name: *const core::ffi::c_char,
        len: usize,
    ) -> zx_types::zx_status_t;
    pub(crate) fn cpp_dispatcher_current_handle_count(dispatcher: *const Dispatcher) -> u32;
}
