// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_memory_stacktrack_process::RegistrySynchronousProxy;
use std::cell::RefCell;

use std::sync::LazyLock;

mod profiler;
use profiler::{PerThreadData, Profiler};
mod recursion_guard;
use recursion_guard::{with_hard_recursion_guard, with_soft_recursion_guard};
mod unwind;

static PROFILER: LazyLock<Profiler> = LazyLock::new(|| Profiler::default());

thread_local! {
    pub static THREAD_DATA: RefCell<PerThreadData> = const { RefCell::new(PerThreadData::new()) };
}

/// Calls `f`, giving it access to the Profiler and the current thread's PerThreadData.
fn with_profiler(f: impl FnOnce(&Profiler, &mut PerThreadData)) {
    let profiler = &*PROFILER;
    THREAD_DATA.with(|thread_data| {
        f(profiler, &mut thread_data.borrow_mut());
    })
}

/// Initializes the stacktrack library, allocates the VMO, and binds to the registry.
///
/// `channel` must be a valid handle to a `fuchsia.memory.stacktrack.process.Registry` channel.
#[unsafe(no_mangle)]
pub extern "C" fn stacktrack_bind_with_channel(channel: zx::sys::zx_handle_t) {
    with_hard_recursion_guard(|| {
        let nullable = unsafe { zx::NullableHandle::from_raw(channel) };
        let channel = zx::Channel::from(nullable);

        let registry = RegistrySynchronousProxy::new(channel);
        with_profiler(|profiler, _thread_data| {
            let Ok(vmo_for_registry) = profiler.get_vmo() else { return };

            let Ok(process) =
                fuchsia_runtime::process_self().duplicate_handle(zx::Rights::SAME_RIGHTS)
            else {
                return;
            };

            let _ = registry.register_v1(process, vmo_for_registry);
        });
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn stacktrack_update_current_thread() {
    with_soft_recursion_guard(|| {
        with_profiler(|profiler, thread_data| {
            profiler.update_thread(thread_data);
        })
    });
}

/// Tears down the current thread's state, removing it from the VMO.
#[unsafe(no_mangle)]
pub extern "C" fn stacktrack_remove_current_thread() {
    with_hard_recursion_guard(|| {
        with_profiler(|profiler, thread_data| {
            profiler.remove_thread(thread_data);
        });
    })
}
