// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon_counter_semaphore.h"

#include <lib/magma/platform/platform_object.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/utils.h>
#include <lib/zx/time.h>

#include <chrono>

#include "zircon_platform_port.h"

namespace magma {

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

bool ZirconCounterSemaphore::duplicate_handle(uint32_t* handle_out) const {
  zx::handle new_handle;
  if (!duplicate_handle(&new_handle))
    return false;
  *handle_out = new_handle.release();
  return true;
}

bool ZirconCounterSemaphore::duplicate_handle(zx::handle* handle_out) const {
  zx::counter duplicate;
  zx_status_t status = counter_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate);
  if (status < 0)
    return DRETF(false, "zx_handle_duplicate failed: %d", status);
  *handle_out = std::move(duplicate);
  return true;
}

magma::Status ZirconCounterSemaphore::WaitNoReset(uint64_t timeout_ms) {
  TRACE_DURATION("magma:sync", "semaphore wait", "id", koid_);
  zx_status_t status = counter_.wait_one(
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

magma::Status ZirconCounterSemaphore::Wait(uint64_t timeout_ms) {
  magma::Status status = WaitNoReset(timeout_ms);
  if (status.ok()) {
    Reset();
  }
  return status;
}

bool ZirconCounterSemaphore::WaitAsync(PlatformPort* port, uint64_t key) {
  TRACE_DURATION("magma:sync", "semaphore wait async", "id", koid_);
  TRACE_FLOW_BEGIN("magma:sync", "semaphore wait async", koid_);

  auto zircon_port = static_cast<ZirconPlatformPort*>(port);

  zx_status_t status = counter_.wait_async(zircon_port->zx_port(), key, GetZxSignal(), 0);
  if (status != ZX_OK)
    return DRETF(false, "wait_async failed: %d", status);

  return true;
}

void ZirconCounterSemaphore::Signal() {
  // The connects with clients waiting on this semaphore.
  // For more info see https://fuchsia.dev/fuchsia-src/development/graphics/magma/concepts/tracing
  TRACE_FLOW_BEGIN("gfx", "event_signal", koid_);

  TRACE_DURATION("magma:sync", "semaphore signal", "id", koid_);
  TRACE_FLOW_BEGIN("magma:sync", "semaphore signal", koid_);
  {
    // Write the timestamp immediately before we signal.  If we get descheduled in between these
    // two operations the timestamp could be early.
    uint64_t timestamp_ns =
        std::chrono::time_point_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now())
            .time_since_epoch()
            .count();

    WriteTimestamp(timestamp_ns);
  }

  zx_status_t status = counter_.signal(0u, GetZxSignal());
  DASSERT(status == ZX_OK);
}

void ZirconCounterSemaphore::Reset() {
  TRACE_DURATION("magma:sync", "semaphore reset", "id", koid_, "oneshot", is_one_shot());
  TRACE_FLOW_END("magma:sync", "semaphore signal", koid_);
  TRACE_FLOW_END("magma:sync", "semaphore wait async", koid_);
  if (is_one_shot()) {
    return;
  }

  WriteTimestamp(0);

  zx_status_t status = counter_.signal(GetZxSignal(), 0);
  DASSERT(status == ZX_OK);
}

void ZirconCounterSemaphore::WriteTimestamp(uint64_t timestamp_ns) {
  zx_status_t status = counter_.write(std::bit_cast<int64_t>(timestamp_ns));
  DASSERT(status == ZX_OK);
}

bool ZirconCounterSemaphore::GetTimestamp(uint64_t* timestamp_ns_out) {
  int64_t timestamp_ns = 0;

  magma_status_t status = counter_.read(&timestamp_ns);
  MAGMA_DASSERT(status == ZX_OK);

  *timestamp_ns_out = std::bit_cast<uint64_t>(timestamp_ns);
  return true;
}

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

}  // namespace magma
