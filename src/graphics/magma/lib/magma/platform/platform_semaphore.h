// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_PLATFORM_SEMAPHORE_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_PLATFORM_SEMAPHORE_H_

#if defined(__Fuchsia__)
#include <lib/zx/handle.h>
#endif

#include <lib/magma/platform/platform_object.h>
#include <lib/magma/util/macros.h>
#include <lib/magma/util/status.h>

#include <memory>

namespace magma {

class PlatformPort;

// Semantics of PlatformSemaphore match Vulkan semaphores. From:
//
// https://www.khronos.org/registry/vulkan/specs/1.0/html/vkspec.html#synchronization-semaphores
//
// "Semaphores are a synchronization primitive that can be used to insert a dependency between
// batches submitted to queues. Semaphores have two states - signaled and unsignaled. The state
// of a semaphore can be signaled after execution of a batch of commands is completed. A batch
// can wait for a semaphore to become signaled before it begins execution, and the semaphore is
// also unsignaled before the batch begins execution."
//
// "Unlike fences or events, the act of waiting for a semaphore also unsignals that semaphore.
// If two operations are separately specified to wait for the same semaphore, and there are no
// other execution dependencies between those operations, behaviour is undefined. An execution
// dependency must be present that guarantees that the semaphore unsignal operation for the first
// of those waits, happens-before the semaphore is signalled again, and before the second unsignal
// operation. Semaphore waits and signals should thus occur in discrete 1:1 pairs."
//
class PlatformSemaphore : public PlatformObject {
 public:
  static std::unique_ptr<PlatformSemaphore> Create();

  explicit PlatformSemaphore(uint64_t flags = 0) : flags_(flags) {}

  // Imports and takes ownership of |handle|.
  static std::unique_ptr<PlatformSemaphore> Import(uint32_t handle, uint64_t flags);
#if defined(__Fuchsia__)
  static std::unique_ptr<PlatformSemaphore> Import(zx::handle handle, uint64_t flags);
#endif

  virtual ~PlatformSemaphore() {}

  bool is_one_shot() const { return flags_ & MAGMA_IMPORT_SEMAPHORE_ONE_SHOT; }

#if defined(__Fuchsia__)
  virtual zx_signals_t GetZxSignal() const = 0;
#endif

  std::unique_ptr<PlatformSemaphore> Clone() {
    uint32_t handle;
    if (!duplicate_handle(&handle))
      return MAGMA_DRETP(nullptr, "failed to duplicate handle");
    return Import(handle, flags_);
  }

  // Signal the semaphore.  State must be unsignalled.
  // Called only by the driver device thread.
  virtual void Signal() = 0;

  // Resets the state to unsignalled. State may be signalled or unsignalled.
  // Called by the client (apps thread) and by the driver device thread.
  virtual void Reset() = 0;

  // Returns MAGMA_STATUS_OK if the event is signaled before the timeout expires.
  virtual magma::Status WaitNoReset(uint64_t timeout_ms) = 0;

  // If the event is signaled before the timeout expires, resets the state to
  // unsignalled (if not one shot) and returns MAGMA_STATUS_OK.  Only one thread
  // should ever wait on a given semaphore.
  virtual magma::Status Wait(uint64_t timeout_ms) = 0;

  magma::Status Wait() { return Wait(UINT64_MAX); }

  // Registers an async wait delivered on the given port when this semaphore is signalled.
  // Note that a port wait completion will not autoreset the semaphore.
  // On success returns true.
  virtual bool WaitAsync(PlatformPort* port, uint64_t key) = 0;

  // Returns true if timestamps are supported; sets |timestamp_ns_out| to the time of the
  // last status change.
  virtual bool GetTimestamp(uint64_t* timestamp_ns_out) { return false; }

 private:
  uint64_t flags_;
};

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_PLATFORM_SEMAPHORE_H_
