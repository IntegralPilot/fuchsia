// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/lib/codec_impl/dispatcher.h"

#include <lib/fdf/cpp/dispatcher.h>
#include <lib/media/codec_impl/log.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/assert.h>

namespace codec_impl {

class DispatcherViaFdfDispatcher : public Dispatcher {
 public:
  explicit DispatcherViaFdfDispatcher(const char* name, std::string_view scheduler_role) {
    auto dispatcher_result = fdf::SynchronizedDispatcher::Create(
        fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, name,
        [this](fdf_dispatcher_t* dispatcher) {
          ZX_DEBUG_ASSERT(dispatcher_.has_value());
          ZX_DEBUG_ASSERT(fdf::SynchronizedDispatcher::GetCurrent()->get() == dispatcher_->get());
          shutdown_completion_.Signal();
          // don't touch "this" after Signal()
        },
        scheduler_role);
    if (!dispatcher_result.is_ok()) {
      LOG(WARN, "fdf::SynchronizedDispatcher::Create failed: %s",
          dispatcher_result.status_string());
      ZX_DEBUG_ASSERT(!dispatcher_.has_value());
      shutdown_completion_.Signal();
      return;
    }
    ZX_DEBUG_ASSERT(!shutdown_completion_.signaled());
    dispatcher_.emplace(std::move(*dispatcher_result));
  }
  // Only called while still single-threaded.
  bool IsStarted() { return dispatcher_.has_value(); }
  ~DispatcherViaFdfDispatcher() {
    ZX_DEBUG_ASSERT(shutdown_completion_.signaled());
    // ~dispatcher_ is delayed until here intentionally to simplify reasoning for the caller re.
    // what methods are safe to call when
  }
  bool IsCurrent() override {
    if (!dispatcher_.has_value()) {
      return false;
    }
    return fdf::SynchronizedDispatcher::GetCurrent()->get() == dispatcher_->get();
  }
  async_dispatcher_t* dispatcher() override {
    ZX_DEBUG_ASSERT(dispatcher_.has_value());
    return dispatcher_->async_dispatcher();
  }
  void QuitAsync() override {
    ZX_DEBUG_ASSERT(dispatcher_.has_value());
    dispatcher_->ShutdownAsync();
  }
  void Join() override { shutdown_completion_.Wait(); }
  std::optional<thrd_t> maybe_thrd() override { return std::nullopt; }

 private:
  std::optional<fdf::SynchronizedDispatcher> dispatcher_;
  libsync::Completion shutdown_completion_;
};

// static
//
// name is for naming the async::Loop thread or fdf::Dispatcher
std::unique_ptr<Dispatcher> DispatcherFactory::Create(const char* name,
                                                      std::string_view scheduler_role) {
  auto result = std::unique_ptr<DispatcherViaFdfDispatcher>(
      new DispatcherViaFdfDispatcher(name, scheduler_role));
  if (!result->IsStarted()) {
    // ~result
    return nullptr;
  }
  return result;
}

}  // namespace codec_impl
