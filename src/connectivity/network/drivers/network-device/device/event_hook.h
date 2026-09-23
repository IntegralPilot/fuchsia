// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_EVENT_HOOK_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_EVENT_HOOK_H_

#include <atomic>
#include <utility>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>

namespace network::internal {

// An event hook class used to install event hooks for testing purposes. A hook is represented by a
// fit::function (or any other callable that allows sharing). For example:
//
//   EventHook<fit::function<void (SomeParam*)>> some_param_event_;
//
// To install a hook, do:
//
//   some_param_event_.Set([](SomeParam* param) { InspectParam(param); });
//
// And the code that triggers the event would look like this:
//
//   some_param_event_.Trigger(&param);
//
// The call to Trigger will efficiently (without locks) return early if no event hook has been
// installed.
template <typename Function>
class EventHook {
 public:
  void Set(Function&& function) {
    fbl::AutoLock lock(&lock_);
    function_ = std::move(function);
    is_set_ = static_cast<bool>(function_);
  }

  // The event hook is currently limited in that return values from the function will be ignored.
  // This is because the call to Trigger has to be able to return if the handler is no longer valid.
  // There is currently no need for return types so to make things easy Trigger just returns void.
  template <typename... Args>
  void Trigger(Args... args) {
    if (!is_set_.load(std::memory_order_relaxed)) {
      return;
    }
    Function function;
    {
      fbl::AutoLock lock(&lock_);
      // Check again, because we didn't hold the lock when checking is_set_ the function could have
      // disappeared by now.
      if (!function_) {
        return;
      }
      // Ensure that the function is kept alive for the entire call in case the function re-sets or
      // clears its own event hook. This also allows the function to be called without holding the
      // lock.
      function = function_.share();
    }
    // At this point function has to be valid. The check in the lock scope should return otherwise.
    function(std::forward<Args>(args)...);
  }

 private:
  Function function_ __TA_GUARDED(lock_);
  // is_set_ exists to allow for IsSet checks without having to acquire the lock.
  std::atomic<bool> is_set_ = false;
  fbl::Mutex lock_;
};

}  // namespace network::internal

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_EVENT_HOOK_H_
