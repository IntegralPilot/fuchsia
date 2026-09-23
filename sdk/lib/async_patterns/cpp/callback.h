// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_CPP_CALLBACK_H_
#define LIB_ASYNC_PATTERNS_CPP_CALLBACK_H_

#include <lib/async_patterns/cpp/internal/tag.h>
#include <lib/async_patterns/cpp/internal/task_queue.h>
#include <lib/async_patterns/cpp/pending_call.h>
#include <lib/async_patterns/cpp/sendable.h>
#include <lib/fit/function.h>
#include <zircon/assert.h>

namespace async_patterns {

template <typename Owner>
class Receiver;

template <typename F>
class Callback;

/// An asynchronous |Callback| that will always execute on the async dispatcher
/// associated with a |Receiver|. Invoking this callback translates to posting a
/// task to the destination dispatcher. It will not block the caller.
///
/// The receiver may not necessarily receive the callback. The callback will be
/// a no-op if:
/// - The |Receiver| object goes out of scope.
/// - The async dispatcher of the |Receiver| shuts down.
///
/// A callback can only be invoked once. It is akin to a one-shot,
/// uni-directional channel. Calls posted to the same |Receiver| will be
/// processed in the order they are made, regardless which |Function|s and
/// |Callback|s they are made from.
template <typename ReturnType, typename... Args>
class Callback<ReturnType(Args...)> {
 public:
  /// Schedules the callback to be asynchronously run on the receiver's
  /// dispatcher.
  ///
  /// See |async_patterns::BindForSending| for detailed requirements on |args|.
  ///
  /// This operator returns a pending call. You may either:
  ///
  /// - Make a fire-and-forget call, by discarding the returned object, or
  /// - Get a promise carrying the return value of the function by calling
  ///   `promise()` on the object, yielding a |fpromise::promise<ReturnType>|, or
  /// - Call `Then()` on the object and pass a |Callback<void(ReturnType)>|
  ///
  /// See |async_patterns::PendingCall| for details.
  ///
  /// Example:
  ///
  ///     async_patterns::Callback<int(std::string)> parse = ...;
  ///
  ///     // Ignore the returned integer.
  ///     parse(std::string("abc"));
  ///
  ///     // Get a promise that will resolve when the function is asynchronously
  ///     // executed on the receiver's async dispatcher.
  ///     fpromise::promise<int> promise = parse(std::string("abc")).promise();
  ///
  auto operator()(Args... args) {
    ZX_ASSERT(task_queue_handle_.has_value());
    return PendingCall{BindForSending(std::move(callback_), std::forward<Args>(args)...),
                       internal::SubmitWithTaskQueueHandle{std::move(task_queue_handle_)},
                       internal::Tag<ReturnType>{}};
  }

  /// Returns a functor that performs the same actions as this |Callback|, but returns
  /// void, instead of potentially a promise object. This is useful when converting the
  /// |Callback| into a |fit::callback<void(ReturnType)|.
  auto ignore_result() && {
    return [callback = std::move(*this)](Args... args) mutable {
      callback(std::forward<Args>(args)...);
    };
  }

 private:
  // The worst case scenario is a pointer-to-member (2 words) and a weak pointer (2 words).
  // We can improve this further using custom weak pointer types if necessary.
  using CallbackType = fit::inline_callback<ReturnType(Args...), sizeof(void*) * 4>;

  template <typename Owner>
  friend class ::async_patterns::Receiver;

  explicit Callback(internal::TaskQueueHandle handle, CallbackType callback)
      : task_queue_handle_(std::move(handle)), callback_(std::move(callback)) {}

  internal::TaskQueueHandle task_queue_handle_;
  CallbackType callback_;
};

}  // namespace async_patterns

#endif  // LIB_ASYNC_PATTERNS_CPP_CALLBACK_H_
