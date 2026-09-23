// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_START_COMPLETER_H_
#define LIB_DRIVER_COMPONENT_CPP_START_COMPLETER_H_

#include <lib/fit/function.h>
#include <lib/zx/result.h>

namespace fdf {

// This class is a wrapper for a callback type that must be called into exactly once
// before destruction. It is a move only type.
class Completer {
 public:
  explicit Completer(fit::callback<void(zx::result<>)> callback) : callback_(std::move(callback)) {}

  Completer(Completer&& other) noexcept : callback_(std::move(other.callback_)) {
    other.callback_ = std::nullopt;
  }

  Completer(const Completer&) = delete;
  Completer& operator=(const Completer&) = delete;

  ~Completer();

  // Calls the wrapped callback function.
  // This method should not be invoked more than once.
  void operator()(zx::result<> result);

 private:
  std::optional<fit::callback<void(zx::result<>)>> callback_;
};

// This is the completer for the Start operation in |DriverBase|.
class StartCompleter final : public Completer {
 public:
  using Completer::Completer;
  using Completer::operator();
};

}  // namespace fdf

#endif  // LIB_DRIVER_COMPONENT_CPP_START_COMPLETER_H_
