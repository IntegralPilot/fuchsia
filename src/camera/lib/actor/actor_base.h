// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_LIB_ACTOR_ACTOR_BASE_H_
#define SRC_CAMERA_LIB_ACTOR_ACTOR_BASE_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/wait.h>
#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>

namespace camera::actor {

class ActorBase {
 public:
  using WaitHandler = fit::function<void(zx_status_t status, const zx_packet_signal_t* signal)>;

  explicit ActorBase(async_dispatcher_t* dispatcher, fpromise::scope& scope)
      : executor_(dispatcher), scope_(scope) {}

 protected:
  template <typename Ret, typename Err>
  void Schedule(fpromise::promise<Ret, Err> promise) {
    executor_.schedule_task(fpromise::pending_task(promise.wrap_with(scope_)));
  }

  template <typename Lambda>
  void Schedule(Lambda lambda) {
    executor_.schedule_task(
        fpromise::pending_task(fpromise::make_promise(std::move(lambda)).wrap_with(scope_)));
  }

  template <typename Ret, typename Err>
  void ScheduleAfterDelay(zx::duration duration, fpromise::promise<Ret, Err> promise) {
    Schedule(executor_.MakeDelayedPromise(duration).and_then(std::move(promise)));
  }

  template <typename Lambda>
  void ScheduleAfterDelay(zx::duration duration, Lambda lambda) {
    Schedule(
        executor_.MakeDelayedPromise(duration).and_then(fpromise::make_promise(std::move(lambda))));
  }

  template <typename Ret, typename Err>
  void ScheduleAtTime(zx::time time, fpromise::promise<Ret, Err> promise) {
    Schedule(executor_.MakePromiseForTime(time).and_then(std::move(promise)));
  }

  template <typename Lambda>
  void ScheduleAtTime(zx::time time, Lambda lambda) {
    Schedule(
        executor_.MakePromiseForTime(time).and_then(fpromise::make_promise(std::move(lambda))));
  }

  void WaitOnce(zx_handle_t object, zx_signals_t trigger, uint32_t options, WaitHandler handler) {
    auto wait = std::make_shared<async::WaitOnce>(object, trigger, options);
    wait->Begin(executor_.dispatcher(),
                [wait = wait, handler = std::move(handler)](
                    async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                    const zx_packet_signal_t* signal) { handler(status, signal); });
  }

  void WaitOnce(zx_handle_t object, zx_signals_t trigger, WaitHandler handler) {
    WaitOnce(object, trigger, 0, std::move(handler));
  }

 private:
  async::Executor executor_;
  fpromise::scope& scope_;
};

}  // namespace camera::actor

#endif  // SRC_CAMERA_LIB_ACTOR_ACTOR_BASE_H_
