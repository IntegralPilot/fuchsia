// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/media/codec_impl/log.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/threads.h>

#include "src/media/lib/codec_impl/dispatcher.h"

namespace codec_impl {

class DispatcherViaAsyncLoop : public Dispatcher {
 public:
  explicit DispatcherViaAsyncLoop(const char* name, std::string_view scheduler_role)
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    thrd_t thrd{};
    zx_status_t start_thread_result = loop_.StartThread(name, &thrd);
    if (start_thread_result != ZX_OK) {
      LOG(WARN, "loop_.StartThread() failed: %s", zx_status_get_string(start_thread_result));
      ZX_DEBUG_ASSERT(thrd_ == thrd_t{});
      return;
    }
    thrd_.store(thrd, std::memory_order_relaxed);
    ZX_DEBUG_ASSERT(thrd_ != thrd_t{});
    if (!scheduler_role.empty()) {
      // probably YAGNI; DFv1 drivers can just keep using CoreCodecSetStreamControlProfile.
      //
      // If needed, would involve plumbing zx_device_t device and:
      //
      // zx::thread zx_thread(thrd_get_zx_handle(thrd_));
      //
      // device_set_profile_by_role(device, zx_thread, scheduler_role.data(),
      // scheduler_role.size());
      //
      // or possibly equivalent/newer/better way(s).
      LOG(WARN, "DispatcherViaAsyncLoop doesn't support scheduler_role so far");
      // intentionally keep going
    }
  }
  bool IsStarted() {
    // Constructor already complained to the log.
    return thrd_.load(std::memory_order_relaxed) != thrd_t{};
  }
  ~DispatcherViaAsyncLoop() {
    // caller must Join() first (unless !IsStarted() immediately after construction)
    ZX_ASSERT(!IsStarted());
  }
  bool IsCurrent() override { return thrd_current() == thrd_.load(std::memory_order_relaxed); }
  async_dispatcher_t* dispatcher() override {
    ZX_DEBUG_ASSERT(IsStarted());
    return loop_.dispatcher();
  }
  void QuitAsync() override {
    ZX_DEBUG_ASSERT(IsStarted());
    loop_.Quit();
  }
  void Join() override {
    ZX_DEBUG_ASSERT(IsStarted());
    loop_.Shutdown();
    loop_.JoinThreads();
    thrd_ = thrd_t{};
  }
  std::optional<thrd_t> maybe_thrd() override { return thrd_; }

 private:
  async::Loop loop_;
  std::atomic<thrd_t> thrd_;
};

// static
//
// name is for naming the async::Loop thread or fdf::Dispatcher
std::unique_ptr<Dispatcher> DispatcherFactory::Create(const char* name,
                                                      std::string_view scheduler_role) {
  auto result =
      std::unique_ptr<DispatcherViaAsyncLoop>(new DispatcherViaAsyncLoop(name, scheduler_role));
  if (!result->IsStarted()) {
    // ~result
    return nullptr;
  }
  return result;
}

}  // namespace codec_impl
