// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_FENCE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_FENCE_H_

#include <lib/async/cpp/wait.h>
#include <lib/async/dispatcher.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/event.h>
#include <zircon/syscalls/port.h>
#include <zircon/types.h>

#include <fbl/intrusive_single_list.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

#include "src/graphics/display/drivers/coordinator/id-map.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"

namespace display_coordinator {

class Fence;

// Interface between a Fence and the class that waits on it.
class FenceListener {
 public:
  FenceListener() = default;

  // FenceListener pointers must remain stable.
  FenceListener(const FenceListener&) = delete;
  FenceListener& operator=(const FenceListener&) = delete;

  // Called after a waited-for Fence's event was signaled.
  //
  // Must be called on the dispatcher used to access the Fence. The listener may
  // cause `fence` to be destroyed, so the caller must not assume that `fence`
  // is still valid after the method call.
  //
  // `fence` is no longer in the waited-for state at the time of the call. The
  // listener may call `Fence::Wait()` to bring the fence back in the waited-for
  // state.
  virtual void OnFenceSignaled(Fence& fence) = 0;

 protected:
  // FenceListener is not intended to be an owning pointer type.
  virtual ~FenceListener() = default;
};

// Manages an event imported by a Coordinator client.
//
// The class currently uses Vulkan terminology, rather than Fuchsia event
// terminology.
//
// Fences are reference-counted. When all the references are dropped, the any
// pending wait operation is canceled, and the event is released.
//
// Instances are not thread-safe and must be accessed on a single synchronized
// dispatcher.
class Fence : public fbl::RefCounted<Fence>,
              public IdMappable<fbl::RefPtr<Fence>, display::EventId>,
              public fbl::SinglyLinkedListable<fbl::RefPtr<Fence>> {
 public:
  // `listener` methods are called on `dispatcher`. `listener`
  // must not be null and must outlive the newly created instance.
  //
  // `dispatcher` must not be null and must outlive the newly created instance.
  // The instance must be accessed exclusively on the dispatcher.
  //
  // `id` and `event` must be valid.
  Fence(FenceListener* listener, fdf::UnownedSynchronizedDispatcher dispatcher, display::EventId id,
        zx::event event);

  Fence(const Fence&) = delete;
  Fence& operator=(const Fence&) = delete;

  ~Fence();

  // Brings the fence in the waited-for state.
  //
  // Signaling a waited-for fence's event causes a call to
  // `FenceListener::OnFenceSignaled()`.
  //
  // This method is idempotent. It makes no change if the fence is already
  // waited-for.
  //
  // The wait operation is automatically canceled when a waited-for fence is
  // destroyed. This can be avoided by holding a reference to the fence while
  // waiting for it to be signaled.
  zx::result<> Wait();

  // Signals the fence's underlying event.
  void Signal();

 private:
  // Called by `signal_waiter_`.
  void OnEventSignaled(async_dispatcher_t* dispatcher, async::WaitBase* self, zx_status_t status,
                       const zx_packet_signal_t* signal);

  FenceListener& listener_;
  const fdf::UnownedSynchronizedDispatcher dispatcher_;
  const zx::event event_;
  zx_koid_t koid_ = 0;

  // Pending when the fence is waited-for.
  async::WaitMethod<Fence, &Fence::OnEventSignaled> signal_waiter_;
};

// Manages the events (Fences) imported by a Coordinator client.
//
// Instances are not thread-safe and must be accessed on a single synchronized
// dispatcher.
class FenceCollection {
 public:
  // Creates an empty collection.
  //
  // `listener` methods are called on `dispatcher`. `listener`
  // must not be null and must outlive the newly created instance.
  //
  // `dispatcher` must not be null and must outlive the newly created instance.
  // The instance must be accessed exclusively on the dispatcher.
  FenceCollection(FenceListener* listener, fdf::UnownedSynchronizedDispatcher dispatcher);

  FenceCollection(const FenceCollection&) = delete;
  FenceCollection& operator=(const FenceCollection&) = delete;

  virtual ~FenceCollection() = default;

  // Releases the FenceCollection instance's references to all imported events.
  //
  // Any Fence that has no reference remaining will be destroyed.
  void Clear();

  // Adds an event to the set of imported events.
  //
  // `id` must be valid.
  //
  // Errors with ZX_ERR_ALREADY_EXISTS if `id` is already assigned to an
  // imported event. Errors with ZX_ERR_NO_MEMORY if a memory allocation fails.
  //
  // If successful, passing `id` to `GetFence()` will retrieve the Fence that
  // manages `event`.
  zx::result<> ImportEvent(zx::event event, display::EventId id);

  // Removes an event from the set of imported events.
  //
  // The method is idempotent. It makes no change if `id` is not assigned to
  // an imported event.
  //
  // The call releases the FenceCollection instance's reference to the Fence
  // managing the imported event. If there are no references remaining, the
  // Fence will be destroyed.
  void ReleaseEvent(display::EventId id);

  // Retrieves the fence managing an imported event.
  //
  // Returns nullptr if `id` is not assigned to an imported event.
  //
  // The returned reference can be used to ensure that the fence is not
  // destroyed (releasing the event and canceling any wait operation) when
  // `ReleaseEvent()` is called with `id`.
  fbl::RefPtr<Fence> GetFence(display::EventId id);

 private:
  Fence::Map imported_fences_;
  FenceListener& listener_;
  const fdf::UnownedSynchronizedDispatcher dispatcher_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_FENCE_H_
