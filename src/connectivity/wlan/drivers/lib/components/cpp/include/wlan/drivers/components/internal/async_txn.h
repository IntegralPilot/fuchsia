// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_INCLUDE_WLAN_DRIVERS_COMPONENTS_INTERNAL_ASYNC_TXN_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_INCLUDE_WLAN_DRIVERS_COMPONENTS_INTERNAL_ASYNC_TXN_H_

#include <lib/fdf/arena.h>
#include <zircon/assert.h>

#include <utility>

template <class Completer, class... Args>
class AsyncTxn {
 public:
  explicit AsyncTxn(Completer& completer) : completer_(completer.ToAsync()) {}
  ~AsyncTxn() {
    if (completer_.is_reply_needed() && !replied_) {
      ZX_ASSERT_MSG(replied_, "Reply must be called on AsyncTxn");
    }
  }
  AsyncTxn(AsyncTxn&& other) noexcept = default;
  AsyncTxn& operator=(AsyncTxn&& other) = default;

  void Reply(Args&&... args) {
    replied_ = true;
    fdf::Arena arena('ASNC');
    completer_.buffer(arena).Reply(args...);
  }

 private:
  using AsyncCompleter = decltype(std::declval<Completer>().ToAsync());
  AsyncCompleter completer_;
  bool replied_ = false;
};

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_INCLUDE_WLAN_DRIVERS_COMPONENTS_INTERNAL_ASYNC_TXN_H_
