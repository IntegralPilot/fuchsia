// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_COUNTER_SEMAPHORE_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_COUNTER_SEMAPHORE_H_

#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma/platform/platform_trace.h>
#include <lib/magma/util/short_macros.h>
#include <lib/zx/counter.h>

namespace magma {

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

// Counter semaphores support timestamps.
// They aren't created by default since they're less memory efficient than the event-based
// ZirconPlatformSemaphore, but they can be imported given a counter handle.
class ZirconCounterSemaphore : public PlatformSemaphore {
 public:
  ZirconCounterSemaphore(zx::counter counter, uint64_t koid, uint64_t flags)
      : PlatformSemaphore(flags), counter_(std::move(counter)), koid_(koid) {}

  void set_local_id(uint64_t id) override {
    DASSERT(id);
    DASSERT(!local_id_);
    local_id_ = id;
  }

  uint64_t id() const override { return local_id_ ? local_id_ : koid_; }
  uint64_t global_id() const override { return koid_; }

  bool duplicate_handle(uint32_t* handle_out) const override;
  bool duplicate_handle(zx::handle* handle_out) const override;

  void Reset() override;
  void Signal() override;

  magma::Status WaitNoReset(uint64_t timeout_ms) override;
  magma::Status Wait(uint64_t timeout_ms) override;

  bool WaitAsync(PlatformPort* port, uint64_t key) override;

  zx_signals_t GetZxSignal() const override { return ZX_COUNTER_SIGNALED; }

  bool GetTimestamp(uint64_t* timestamp_ns_out) override;

 private:
  void WriteTimestamp(uint64_t timestamp_ns);

  zx::counter counter_;
  uint64_t koid_;
  uint64_t local_id_ = 0;
};

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_COUNTER_SEMAPHORE_H_
