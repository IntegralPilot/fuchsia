// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_CPP_PENDING_CALL_H_
#define LIB_ASYNC_PATTERNS_CPP_PENDING_CALL_H_

#include <lib/async_patterns/cpp/internal/tag.h>
#include <lib/fpromise/bridge.h>
#include <zircon/assert.h>

#include <utility>

namespace async_patterns {

template <typename F>
class Callback;

/// This type is usually returned from a |DispatcherBound::AsyncCall| or calling
/// |Callback<ReturnType(Args...)>|.
///
/// Let |Call| be a callable that takes zero arguments and returns |ReturnType|.
/// |PendingCall| represents a call that is yet to be run, and offers a variety
/// of ways to monitor its return value:
///
/// - The caller may discard the |PendingCall|, at which point the call will be
///   submitted for execution but its return value will be ignored.
///
/// - The caller may call `promise()` to get a `fpromise::promise<ReturnType>`
///   that will resolve if the call runs to completion, or be abandoned if the
///   call is dropped.
///
/// - The caller may call `Then` to specify an `async_patterns::Callback<void(R)>`
///   that will be called when the call runs to completion.
///
template <typename ReturnType, typename Call, typename Submit>
class PendingCall {
 public:
  /// Make the call if not already, and ignore the result.
  ///
  /// This leads to "fire-and-forget" behavior:
  ///
  ///     async_patterns::DispatcherBound<MyType> object;
  ///
  ///     // This returns a |PendingCall|. If we do nothing with the return
  ///     // value, that means making the call and we don't care about its result.
  ///     object.AsyncCall(&MyType::SomeMethod);
  ///
  ~PendingCall() {
    if (submit_.has_value()) {
      submit([call = std::move(call_)]() mutable { (void)call(); });
    }
  }

  /// Make the call and return a promise representing the return value of the call.
  ///
  /// The promise will resolve if the call runs to completion.
  ///
  /// The promise will be abandoned if the call is dropped, such as if the target object
  /// that is supposed to respond to this async call is already destroyed.
  ///
  /// Example:
  ///
  ///     async_patterns::Callback<std::string(int)> callback = ...;
  ///     fpromise::promise<int> promise = callback(42).promise();
  ///
  ///     // Now you can do something with the promise..
  ///     executor.schedule_task(
  ///         promise.and_then([] (int& value) { ... }));
  ///
  fpromise::promise<ReturnType> promise() && {
    fpromise::bridge<ReturnType> bridge;
    submit([call = std::move(call_), completer = bridge.completer.bind()]() mutable {
      using CallReturnType = decltype(call());
      static_assert(std::is_convertible_v<CallReturnType, ReturnType>);
      if constexpr (std::is_void_v<ReturnType>) {
        call();
        completer();
      } else {
        completer(call());
      }
    });
    return bridge.consumer.promise();
  }

  /// Arranges |on_result| to be called with the result of the async call.
  /// |on_result| is an |async_patterns::Callback<void(R)>|. |R| could be
  /// identical to |ReturnType| or be some other compatible type such as |const
  /// ReturnType&|. Typically, the owner will declare a |Receiver| to mint these
  /// callbacks.
  ///
  /// This allows two thread-unsafe objects living on different dispatchers to
  /// exchange messages in a ping-pong fashion. Example:
  ///
  ///     class Owner {
  ///      public:
  ///       Owner(async_dispatcher_t* owner_dispatcher)
  ///         : receiver_{this, owner_dispatcher},
  ///           background_loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
  ///           background_{background_loop_.dispatcher(), std::in_place} {}
  ///
  ///       void StartDoingStuff() {
  ///         // Make a call on |Background|, and then receive the result
  ///         // at |DoneDoingStuff|.
  ///         background_.AsyncCall(&Background::DoStuff, 42)
  ///             .Then(receiver_.Once(&Owner::DoneDoingStuff));
  ///       }
  ///
  ///       void DoneDoingStuff(std::string result) {
  ///         // Check the result from |Background::DoStuff|.
  ///       }
  ///
  ///      private:
  ///       async_patterns::Receiver<Owner> receiver_;
  ///       async::Loop background_loop_;
  ///       async_patterns::DispatcherBound<Background> background_;
  ///     };
  ///
  /// See more in |async_patterns::DispatcherBound|.
  template <typename R>
  void Then(async_patterns::Callback<void(R)> on_result) && {
    constexpr bool kReceiverMatchesReturnValue =
        std::is_invocable_v<decltype(on_result), ReturnType>;
    static_assert(kReceiverMatchesReturnValue,
                  "The |async_patterns::Callback<void(R)>| must accept the return value "
                  "of the async call.");
    if constexpr (kReceiverMatchesReturnValue) {
      submit([call = std::move(call_), on_result = std::move(on_result)]() mutable {
        if constexpr (std::is_void_v<ReturnType>) {
          call();
          on_result();
        } else {
          on_result(call());
        }
      });
    }
  }

  //// |Call| should be a callable that takes zero arguments and returns |ReturnType|.
  ////
  //// |Submit| should be a callable that takes a |Call| and submits it for
  //// asynchronous execution. In addition, it should have an empty state reachable
  //// by calling |reset| and support checking for emptiness using |has_value|.
  PendingCall(Call call, Submit submit, internal::Tag<ReturnType>)
      : call_(std::move(call)), submit_(std::move(submit)) {}

  PendingCall(const PendingCall&) = delete;
  PendingCall& operator=(const PendingCall&) = delete;
  PendingCall(PendingCall&&) noexcept = delete;
  PendingCall& operator=(PendingCall&&) noexcept = delete;

 protected:
  template <typename Continuation>
  void CallWithContinuation(Continuation continuation) {
    submit([call = std::move(call_), c = std::move(continuation)]() mutable { c(call()); });
  }

 private:
  template <typename Task>
  void submit(Task&& task) {
    ZX_DEBUG_ASSERT(submit_.has_value());
    submit_(std::forward<Task>(task));
    submit_.reset();
  }

  Call call_;
  Submit submit_;
};

template <typename ReturnType, typename Call, typename Submit>
PendingCall(Call call, Submit submit, internal::Tag<ReturnType>)
    -> PendingCall<ReturnType, Call, Submit>;

}  // namespace async_patterns

#endif  // LIB_ASYNC_PATTERNS_CPP_PENDING_CALL_H_
