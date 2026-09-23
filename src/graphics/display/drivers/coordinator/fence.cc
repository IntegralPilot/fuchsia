// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/fence.h"

#include <lib/async/cpp/wait.h>
#include <lib/async/dispatcher.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/trace/event.h>
#include <lib/zx/event.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/intrusive_single_list.h>
#include <fbl/ref_ptr.h>

#include "src/graphics/display/lib/api-types/cpp/event-id.h"

namespace display_coordinator {

void Fence::Signal() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());
  event_.signal(/*clear_mask=*/0, /*set_mask=*/ZX_EVENT_SIGNALED);
}

zx::result<> Fence::Wait() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());

  if (signal_waiter_.is_pending()) {
    return zx::ok();
  }

  zx_status_t wait_status = signal_waiter_.Begin(dispatcher_->async_dispatcher());
  if (wait_status != ZX_OK) {
    return zx::error(wait_status);
  }
  ZX_DEBUG_ASSERT(signal_waiter_.is_pending());
  return zx::ok();
}

void Fence::OnEventSignaled(async_dispatcher_t* dispatcher, async::WaitBase* self,
                            zx_status_t status, const zx_packet_signal_t* signal) {
  ZX_DEBUG_ASSERT_MSG(status == ZX_OK, "Fence::OnReady failed: %s", zx_status_get_string(status));
  ZX_DEBUG_ASSERT(dispatcher != nullptr);
  ZX_DEBUG_ASSERT((signal->observed & ZX_EVENT_SIGNALED) != 0);
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());
  ZX_DEBUG_ASSERT(dispatcher == dispatcher_->async_dispatcher());
  TRACE_DURATION("gfx", "Display::Fence::OnReady");
  TRACE_FLOW_END("gfx", "event_signal", koid_);

  ZX_DEBUG_ASSERT(!signal_waiter_.is_pending());
  event_.signal(/*clear_mask=*/ZX_EVENT_SIGNALED, /*set_mask=*/0);
  listener_.OnFenceSignaled(*this);
}

Fence::Fence(FenceListener* listener, fdf::UnownedSynchronizedDispatcher dispatcher,
             display::EventId fence_id, zx::event event)
    : IdMappable(fence_id),
      listener_(*listener),
      dispatcher_(std::move(dispatcher)),
      event_(std::move(event)),
      signal_waiter_(this, event_.get(), /*trigger=*/ZX_EVENT_SIGNALED, /*options=*/0) {
  ZX_DEBUG_ASSERT(listener != nullptr);
  ZX_DEBUG_ASSERT(dispatcher_->get() != nullptr);
  ZX_DEBUG_ASSERT(fence_id != display::kInvalidEventId);
  ZX_DEBUG_ASSERT(event_.is_valid());
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());

  zx_info_handle_basic_t info;
  zx_status_t status = event_.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ZX_DEBUG_ASSERT(status == ZX_OK);
  koid_ = info.koid;
}

Fence::~Fence() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());
}

FenceCollection::FenceCollection(FenceListener* listener,
                                 fdf::UnownedSynchronizedDispatcher dispatcher)
    : listener_(*listener), dispatcher_(std::move(dispatcher)) {
  ZX_DEBUG_ASSERT(listener != nullptr);
  ZX_DEBUG_ASSERT(dispatcher_->get() != nullptr);
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());
}

void FenceCollection::Clear() {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());
  imported_fences_.clear();
}

zx::result<> FenceCollection::ImportEvent(zx::event event, display::EventId id) {
  ZX_DEBUG_ASSERT(event.is_valid());
  ZX_DEBUG_ASSERT(id != display::kInvalidEventId);
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());

  auto imported_fences_it = imported_fences_.find(id);
  if (imported_fences_it.IsValid()) {
    fdf::error("Refused to import an event with existing ID: {}", id.value());
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  fbl::AllocChecker alloc_checker;
  fbl::RefPtr<Fence> fence = fbl::AdoptRef(
      new (&alloc_checker) Fence(&listener_, dispatcher_->borrow(), id, std::move(event)));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate Fence for event ID: {}", id.value());
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  bool successfully_inserted = imported_fences_.insert_or_find(std::move(fence));
  ZX_DEBUG_ASSERT(successfully_inserted);
  return zx::ok();
}

void FenceCollection::ReleaseEvent(display::EventId id) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());

  auto imported_fences_it = imported_fences_.find(id);
  if (!imported_fences_it.IsValid()) {
    return;
  }
  imported_fences_.erase(imported_fences_it);
}

fbl::RefPtr<Fence> FenceCollection::GetFence(display::EventId id) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->async_dispatcher() ==
                  dispatcher_->async_dispatcher());

  if (id == display::kInvalidEventId) {
    return nullptr;
  }
  auto imported_fences_it = imported_fences_.find(id);
  return imported_fences_it.IsValid() ? imported_fences_it.CopyPointer() : nullptr;
}

}  // namespace display_coordinator
