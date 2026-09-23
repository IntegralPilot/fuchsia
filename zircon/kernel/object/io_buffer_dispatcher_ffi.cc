// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <object/handle.h>

#include "object/io_buffer_dispatcher.h"

extern "C" {

zx_status_t cpp_io_buffer_dispatcher_create(
    void* holder, size_t endpoint_id, void* shared_state,
    ffi::Uninitialized<KernelHandle<IoBufferDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(
      new (&ac) IoBufferDispatcher(holder, static_cast<IobEndpointId>(endpoint_id), shared_state));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}

// IoBufferDispatcher inherits from both Dispatcher and VmObjectChildObserver via multiple
// inheritance. In C++, casting IoBufferDispatcher* to VmObjectChildObserver* adjusts the base
// pointer by an offset to point to the VmObjectChildObserver vtable subobject. This FFI helper
// performs that C++ upcast so that Rust callers pass the correct adjusted pointer to
// VmObject::SetChildObserver.
VmObjectChildObserver* cpp_io_buffer_dispatcher_as_child_observer(IoBufferDispatcher* disp) {
  return disp;
}

}  // extern "C"
