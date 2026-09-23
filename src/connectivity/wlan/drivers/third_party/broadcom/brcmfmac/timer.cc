// Copyright (c) 2019 The Fuchsia Authors
//
// Permission to use, copy, modify, and/or distribute this software for any purpose with or without
// fee is hereby granted, provided that the above copyright notice and this permission notice
// appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
// SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
// AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
// NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE
// OF THIS SOFTWARE.

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/timer.h"

#include <lib/async/task.h>
#include <lib/async/time.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <atomic>

// This Instance class exists so that it can be tied to the lifetime of each Timer invocation. Each
// call to Start by a user is an invocation in this context. A periodic timer re-arming itself does
// not create a new invocation. The reason that a separate object is needed is because timer handler
// tasks might still be called even after the timer has stopped or been destroyed. By using a
// shared_ptr to an Instance object and a captured weak_ptr in the timer handler the Instance can
// determine if the timer is active, stopped or destroyed by examining the weak_ptr.
class Timer::Instance : public std::enable_shared_from_this<Instance> {
 public:
  Instance(Timer& timer, async_dispatcher_t* dispatcher);
  ~Instance();
  zx_status_t Start(zx_duration_t interval);
  void Restart();
  void Stop();

 private:
  // Use the C interface for async_task, the C++ implementation does not support access from
  // multiple threads.
  struct AsyncTask : public async_task_t {
    std::weak_ptr<Instance> instance;
  };
  static void Handler(async_dispatcher_t*, async_task_t* task, zx_status_t status);
  zx_status_t PostTask();
  void CancelTask();

  Timer& timer_;
  async_dispatcher_t* const dispatcher_;
  // This has to be a recursive mutex to allow Stop calls from the timer handler's callback.
  // Unfortunately thread annotations are not supported for recursive mutexes, be careful.
  std::recursive_mutex mutex_;
  zx_duration_t interval_ = 0;
  bool active_ = false;
  std::atomic<AsyncTask*> async_task_ = nullptr;
  std::function<void()> handler_;
};

Timer::Timer(async_dispatcher_t* dispatcher, std::function<void()>&& callback, Type type)
    : dispatcher_(dispatcher), callback_(std::move(callback)), type_(type) {
  ZX_ASSERT_MSG(dispatcher_ != nullptr, "The dispatcher parameter cannot be null");
}

Timer::~Timer() { Stop(); }

zx_status_t Timer::Start(zx_duration_t interval) {
  if (interval == 0) {
    // This will allow the next periodic timer trigger to occur but no further triggers after that.
    // To accomplish this we cannot call Stop so this case needs special treatment.
    std::lock_guard lock(mutex_);
    if (instance_) {
      instance_->Start(interval);
    }
    return ZX_OK;
  }

  // Stop any previously started timer first.
  Stop();

  // Create a new instance for each invocation of Start. This way existing tasks can be gracefully
  // shut down, allowing them to observe the state associated with that instance even if a new timer
  // has started.
  std::lock_guard lock(mutex_);
  if (instance_) {
    // Another call to start was able to create and store a new instance between the call to Stop
    // and this point. Calling Start multiple times in parallel doesn't have to make any guarantees
    // about which Start call wins. Take the easy way out and let that other Start call win.
    return ZX_OK;
  }
  instance_ = std::make_shared<Instance>(*this, dispatcher_);
  return instance_->Start(interval);
}

void Timer::Stop() {
  // Stop the instance while making sure to NOT hold the mutex. Stop may synchronously wait for an
  // existing timer handler to complete. If that handler attempted to acquire the mutex this would
  // deadlock if the mutex was held here.
  std::shared_ptr<Instance> instance;
  {
    std::lock_guard lock(mutex_);
    // By moving the instance out of the member variable it will be destroyed after Stop has
    // completed and any in-flight timer handler has finished. It also leaves the member variable
    // empty which puts the Timer object in a stopped state.
    instance = std::move(instance_);
  }
  if (instance) {
    instance->Stop();
  }
}

bool Timer::Stopped() {
  std::lock_guard lock(mutex_);
  return !instance_;
}

void Timer::TimerHandler() {
  // Copy the instance here so that we can check if a new instance was created during the callback.
  // The instance_ member variable needs to stay in place, otherwise Stopped will return true even
  // though the timer is currently running.
  std::shared_ptr<Instance> invocation_instance;
  {
    std::lock_guard lock(mutex_);
    invocation_instance = instance_;
  }

  // Execute the user's callback.
  callback_();

  std::lock_guard lock(mutex_);
  if (instance_ && instance_ == invocation_instance) {
    // The instance is valid and remains the same. Either destroy it or restart it. If a new
    // instance was created it has already started and the instance for this invocation will be
    // destroyed when the invocation_instance copy goes out of scope.
    switch (type_) {
      case Type::OneShot:
        instance_.reset();
        break;
      case Type::Periodic:
        instance_->Restart();
        break;
    }
  }
}

Timer::Instance::Instance(Timer& timer, async_dispatcher_t* dispatcher)
    : timer_(timer), dispatcher_(dispatcher) {}

Timer::Instance::~Instance() { Stop(); }

zx_status_t Timer::Instance::Start(zx_duration_t interval) {
  std::lock_guard lock(mutex_);
  if (!handler_) {
    // shared_from_this doesn't work in the constructor of Instance since the shared_ptr hasn't
    // been created at that time. Set the task handler here where shared_from_this can be used.
    handler_ = [this, weak_instance = std::weak_ptr(shared_from_this())]() {
      std::shared_ptr shared_instance = weak_instance.lock();
      if (!shared_instance) {
        // If the weak pointer can't be locked the instance was destroyed. Do nothing.
        return;
      }
      std::lock_guard lock(shared_instance->mutex_);
      if (!shared_instance->active_) {
        // The instance is alive but the timer has been stopped, don't trigger the handler.
        return;
      }
      // Mark active as false to avoid a race condition if Stop() or Start() gets called in the
      // handler.
      shared_instance->active_ = false;
      timer_.TimerHandler();
    };
  }
  interval_ = interval;
  if (interval_ == 0) {
    // One way to stop periodic timer
    return ZX_OK;
  }
  active_ = true;

  return PostTask();
}

void Timer::Instance::Restart() { Start(interval_); }

void Timer::Instance::Stop() {
  std::lock_guard lock(mutex_);
  active_ = false;
  interval_ = 0;
  CancelTask();
}

void Timer::Instance::Handler(async_dispatcher_t*, async_task_t* task, zx_status_t status) {
  // Place the task in a unique_ptr, it has to be destroyed when the handler completes.
  std::unique_ptr<AsyncTask> async_task(static_cast<AsyncTask*>(task));
  if (status != ZX_OK) {
    return;
  }
  std::shared_ptr<Instance> shared_async_task = async_task->instance.lock();
  if (shared_async_task) {
    shared_async_task->handler_();
  }
}

zx_status_t Timer::Instance::PostTask() {
  // Each PostTask invocation gets its own AsyncTask object. This is the only safe way to deal with
  // trailing task calls after the timer or instance has been destroyed. This means carefully
  // managing the lifetime of the task object if posting fails or if the task is canceled. The
  // AsyncTask object holds a weak pointer to the Instance object. This way it can safely determine
  // if the instance object is still alive or not.
  AsyncTask* async_task = new AsyncTask{
      async_task_t{.handler = &Instance::Handler, .deadline = async_now(dispatcher_) + interval_},
      std::weak_ptr{shared_from_this()}};

  AsyncTask* old_async_task = async_task_.exchange(async_task);
  if (old_async_task) {
    // If there was an existing task, cancel it.
    const zx_status_t status = async_cancel_task(dispatcher_, old_async_task);
    if (status == ZX_OK) {
      // On success the handler won't be called and won't destroy the task. Manually do it.
      delete old_async_task;
    }
  }
  const zx_status_t status = async_post_task(dispatcher_, async_task);
  if (status != ZX_OK) {
    // If the post fails the handler won't be called and won't destroy the task. Manually do it.
    delete async_task;
    return status;
  }
  return ZX_OK;
}

void Timer::Instance::CancelTask() {
  AsyncTask* async_task = async_task_.exchange(nullptr);
  if (async_task == nullptr) {
    // There is not task to cancel, do nothing.
    return;
  }
  const zx_status_t status = async_cancel_task(dispatcher_, async_task);
  if (status == ZX_OK) {
    // On success the handler won't be called and won't destroy the task. Manually do it.
    delete async_task;
  }
}
