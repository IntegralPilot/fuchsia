// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/ffi.h>
#include <object/event_dispatcher.h>

extern "C" {

zx_status_t cpp_event_dispatcher_create(
    uint32_t options, ffi::Uninitialized<KernelHandle<EventDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) EventDispatcher(options));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_event_dispatcher_get_mem_pressure_event(
    uint32_t kind, fbl::RefPtr<EventDispatcher>* out_event) {
  *out_event = GetMemPressureEvent(kind);
}

zx_status_t cpp_memory_stall_event_dispatcher_create(
    uint32_t kind, zx_duration_mono_t threshold, zx_duration_mono_t window,
    ffi::Uninitialized<KernelHandle<EventDispatcher>>* out_handle,
    ffi::Uninitialized<zx_rights_t>* out_rights) {
  KernelHandle<EventDispatcher> handle;
  zx_rights_t rights;
  zx_status_t status = MemoryStallEventDispatcher::Create(
      static_cast<zx_system_memory_stall_type_t>(kind), threshold, window, &handle, &rights);
  if (status != ZX_OK) {
    return status;
  }
  out_handle->Initialize(ktl::move(handle));
  out_rights->Initialize(rights);
  return ZX_OK;
}

}  // extern "C"
