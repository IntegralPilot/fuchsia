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

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_TIMER_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_TIMER_H_

#include <lib/async/dispatcher.h>
#include <zircon/compiler.h>
#include <zircon/time.h>

#include <functional>
#include <memory>
#include <mutex>

class Timer {
 public:
  // A OneShot timer will only fire once for each call to Start. A Periodic timer will fire
  // regularly at the interval specified in the Start call.
  enum class Type { OneShot, Periodic };

  // Construct a timer that will run on |dispatcher| and call |callback| whenever the timer is
  // triggered. If |type| is Periodic the timer will restart itself after calling |callback|. To
  // prevent this, call Start with a duration of zero. This can be done inside |callback| to prevent
  // further timer callbacks. Caller must ensure that |dispatcher| remains running for the lifetime
  // of the Timer object or until Stop has been called on the timer.
  Timer(async_dispatcher_t* dispatcher, std::function<void()>&& callback, Type type);
  // Destroying the timer will synchronously Stop the timer before completing destruction.
  ~Timer();
  // Timers cannot be safely copied or moved.
  Timer(const Timer&) = delete;
  Timer& operator=(Timer&) = delete;
  // Start the timer with |interval| amount of time before the timer triggers. The same interval is
  // used when restarting the timer if it is periodic. Calling Start with an |interval| of zero on a
  // periodic timer will allow the timer to trigger at its next scheduled time but it will not
  // restart after that. Calling Start with an |interval| of zero on a one-shot timer has no effect.
  // Calling Start with |interval| greater than zero will adjust the timer interval to the new value
  // AND postpone any upcoming trigger to not be called until |interval| amount of time has passed.
  // For periodic timers this will also adjust the interval of all future timer triggers. This can
  // safely be done from the timer callback.
  // Returns |ZX_OK| on success.
  // Returns |ZX_ERR_BAD_STATE| if the dispatcher is shutting down.
  // Returns |ZX_ERR_NOT_SUPPORTED| if not supported by the dispatcher.
  zx_status_t Start(zx_duration_t interval);
  // Stop a running timer synchronously, blocking until the stop is complete. When Stop returns the
  // callback provided in the constructor is guaranteed to not run again until Start is called. If
  // the callback is currently running it will run to completion before Stop returns.
  void Stop();
  bool Stopped();

 private:
  // Called by Instance when the timer triggers. This in turn will call the user-provided callback.
  void TimerHandler();

  // A class that represents each internal instance of a Timer, started by a call to Start. See
  // implementation for more details.
  class Instance;

  std::shared_ptr<Instance> instance_ __TA_GUARDED(mutex_);
  std::mutex mutex_;

  async_dispatcher_t* const dispatcher_;
  const std::function<void()> callback_;
  const Type type_;
};

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_TIMER_H_
