// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon_platform_semaphore.h"

#include <lib/magma/platform/platform_object.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/utils.h>
#include <lib/zx/time.h>

#include "zircon_counter_semaphore.h"
#include "zircon_platform_port.h"

namespace magma {

bool ZirconPlatformSemaphore::duplicate_handle(uint32_t* handle_out) const {
  zx::handle new_handle;
  if (!duplicate_handle(&new_handle))
    return false;
  *handle_out = new_handle.release();
  return true;
}

bool ZirconPlatformSemaphore::duplicate_handle(zx::handle* handle_out) const {
  zx::event duplicate;
  zx_status_t status = event_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate);
  if (status < 0)
    return DRETF(false, "zx_handle_duplicate failed: %d", status);
  *handle_out = std::move(duplicate);
  return true;
}

magma::Status ZirconPlatformSemaphore::WaitNoReset(uint64_t timeout_ms) {
  TRACE_DURATION("magma:sync", "semaphore wait", "id", koid_);
  zx_status_t status = event_.wait_one(
      GetZxSignal(), zx::deadline_after(zx::duration(magma::ms_to_signed_ns(timeout_ms))), nullptr);
  switch (status) {
    case ZX_OK:
      return MAGMA_STATUS_OK;
    case ZX_ERR_TIMED_OUT:
      return MAGMA_STATUS_TIMED_OUT;
    case ZX_ERR_CANCELED:
      return MAGMA_STATUS_CONNECTION_LOST;
    default:
      return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Unexpected wait() status: %d", status);
  }
}

magma::Status ZirconPlatformSemaphore::Wait(uint64_t timeout_ms) {
  magma::Status status = WaitNoReset(timeout_ms);
  if (status.ok()) {
    Reset();
  }
  return status;
}

bool ZirconPlatformSemaphore::WaitAsync(PlatformPort* port, uint64_t key) {
  TRACE_DURATION("magma:sync", "semaphore wait async", "id", koid_);
  TRACE_FLOW_BEGIN("magma:sync", "semaphore wait async", koid_);

  auto zircon_port = static_cast<ZirconPlatformPort*>(port);

  zx_status_t status = event_.wait_async(zircon_port->zx_port(), key, GetZxSignal(), 0);
  if (status != ZX_OK)
    return DRETF(false, "wait_async failed: %d", status);

  return true;
}

//////////////////////////////////////////////////////////////////////////////////////////////////

std::unique_ptr<PlatformSemaphore> PlatformSemaphore::Create() {
  zx::event event;
  zx_status_t status = zx::event::create(0, &event);
  if (status != ZX_OK)
    return DRETP(nullptr, "event::create failed: %d", status);

  uint64_t koid;
  if (!PlatformObject::IdFromHandle(event.get(), &koid))
    return DRETP(nullptr, "couldn't get koid from handle");

  return std::make_unique<ZirconPlatformSemaphore>(std::move(event), koid, /*flags=*/0);
}

std::unique_ptr<PlatformSemaphore> PlatformSemaphore::Import(uint32_t handle, uint64_t flags) {
  return Import(zx::handle(handle), flags);
}

std::unique_ptr<PlatformSemaphore> PlatformSemaphore::Import(zx::handle handle, uint64_t flags) {
  zx_info_handle_basic_t info;
  zx_status_t status = handle.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK)
    return DRETP(nullptr, "couldn't get handle info %d", status);

  if (flags & ~MAGMA_IMPORT_SEMAPHORE_ONE_SHOT)
    return DRETP(nullptr, "unhandled flags 0x%lx", flags);

  switch (info.type) {
    case ZX_OBJ_TYPE_EVENT:
      return std::make_unique<ZirconPlatformSemaphore>(zx::event(std::move(handle)), info.koid,
                                                       flags);
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    case ZX_OBJ_TYPE_COUNTER:
      return std::make_unique<ZirconCounterSemaphore>(zx::counter(std::move(handle)), info.koid,
                                                      flags);
#endif
    default:
      return DRETP(nullptr, "unexpected object type: %d", info.type);
  }
}

}  // namespace magma
