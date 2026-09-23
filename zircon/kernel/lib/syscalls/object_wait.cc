// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/ktrace.h>
#include <lib/syscalls/forward.h>
#include <lib/user_copy/user_ptr.h>
#include <platform.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/inline_array.h>
#include <fbl/ref_ptr.h>
#include <kernel/event.h>
#include <kernel/lockdep.h>
#include <kernel/thread.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/port_dispatcher.h>
#include <object/process_dispatcher.h>
#include <object/wait_signal_observer.h>

#define LOCAL_TRACE 0

// zx_status_t zx_object_wait_async
zx_status_t sys_object_wait_async(zx_handle_t handle_value, zx_handle_t port_handle_value,
                                  uint64_t key, zx_signals_t signals, uint32_t options) {
  LTRACEF("handle %x\n", handle_value);

  if ((options & ~(ZX_WAIT_ASYNC_TIMESTAMP | ZX_WAIT_ASYNC_BOOT_TIMESTAMP | ZX_WAIT_ASYNC_EDGE)) !=
      0u) {
    return ZX_ERR_INVALID_ARGS;
  }

  // It is invalid to request both a monotonic and boot timestamp.
  if ((options & ZX_WAIT_ASYNC_TIMESTAMP) != 0 && (options & ZX_WAIT_ASYNC_BOOT_TIMESTAMP) != 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto up = ProcessDispatcher::GetCurrent();

  {
    Guard<BrwLockPi, BrwLockPi::Reader> guard{up->handle_table().get_lock()};

    // Note, we're doing this all while holding the handle table lock for two reasons.
    //
    // First, this thread may be racing with another thread that's closing the last handle to
    // the port. By holding the lock we can ensure that this syscall behaves as if the port was
    // closed just *before* the syscall started or closed just *after* it has completed.
    //
    // Second, MakeObserver takes a Handle. By holding the lock we ensure the Handle isn't
    // destroyed out from under it.

    Handle* port_handle = up->handle_table().GetHandleLocked(*up, port_handle_value);
    if (!port_handle) {
      return ZX_ERR_BAD_HANDLE;
    }
    fbl::RefPtr<Dispatcher> port_dispatcher = port_handle->dispatcher();
    fbl::RefPtr<PortDispatcher> port = DownCastDispatcher<PortDispatcher>(&port_dispatcher);
    if (!port) {
      return ZX_ERR_WRONG_TYPE;
    }
    if (!port_handle->HasRights(ZX_RIGHT_WRITE)) {
      return ZX_ERR_ACCESS_DENIED;
    }

    Handle* handle = up->handle_table().GetHandleLocked(*up, handle_value);
    if (!handle) {
      return ZX_ERR_BAD_HANDLE;
    }
    if (!handle->HasRights(ZX_RIGHT_WAIT)) {
      return ZX_ERR_ACCESS_DENIED;
    }

    fbl::RefPtr<Dispatcher> dispatcher = handle->dispatcher();

    return dispatcher->MakePortObserver(options, handle, signals, key, port.get());
  }
}
