// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_CPP_RECEIVER_H_
#define LIB_ASYNC_PATTERNS_CPP_RECEIVER_H_

#include <lib/async/dispatcher.h>
#include <lib/async_patterns/cpp/callback.h>
#include <lib/async_patterns/cpp/function.h>
#include <lib/async_patterns/cpp/internal/receiver_base.h>
#include <lib/fit/function_traits.h>
#include <lib/fit/nullable.h>

namespace async_patterns {

/// A |Receiver| is a hub for an owner object to asynchronously receive messages
/// and calls from other objects living on different async dispatchers. A
/// |Receiver| should be embedded as a member variable of an owner object which
/// wishes to receive messages. The receiver will silently discard pending
/// messages when it destructs, typically as part of its parent.
///
/// The |Receiver| is thread-unsafe, and must be used and managed from a
/// [synchronized dispatcher][synchronized-dispatcher].
///
/// Calls posted to the same |Receiver| will be processed in the order they are
/// made, regardless which |Function|s and |Callback|s they are made from.
///
/// Calls posted to different |Receiver|s living on the same dispatcher are not
/// guaranteed to be processed before or after one another.
///
/// [synchronized-dispatcher]:
/// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#synchronized-dispatcher
template <typename Owner>
class Receiver : private internal::ReceiverBase {
 public:
  /// Constructs a receiver. |owner| should be `this`. |dispatcher| should be
  /// the dispatcher that the current task is running on, and where |Owner|
  /// typically lives.
  explicit Receiver(Owner* owner, async_dispatcher_t* dispatcher)
      : ReceiverBase(dispatcher), owner_(owner) {}

  Receiver(const Receiver&) = delete;
  Receiver& operator=(const Receiver&) = delete;
  Receiver(Receiver&&) = delete;
  Receiver& operator=(Receiver&&) = delete;

  /// Mints a |Callback| object that holds a capability to send |Args| to the
  /// owner object once. When the resulting |Callback| is invoked on some other
  /// thread, |member| is scheduled to be called on the |dispatcher|.
  ///
  /// |member| should be a pointer to member function. It should have the
  /// function signature `void Owner::SomeMember(Args...)` where Args are some
  /// number of arguments.
  template <typename Member>
  auto Once(Member Owner::*member) {
    return BindImpl<Callback, typename fit::callable_traits<Member>::return_type>(
        std::mem_fn(member), typename fit::callable_traits<Member>::args{});
  }

  /// Mints a |Callback| object that holds a capability to send |Args| to the
  /// owner object once. When the resulting |Callback| is invoked on some other
  /// thread, |callable| is scheduled to be called on the |dispatcher|.
  ///
  /// |callable| should be a lambda without any captures. It should have the
  /// function signature `void(Owner*, Args...)` where Args are some number of
  /// arguments.
  template <typename StatelessLambda>
  auto Once(StatelessLambda callable) {
    // TODO(https://fxbug.dev/42070737): We'll be able to support lambda with captures
    // given compiler tooling that inspects the capture list and determine
    // they're safe.
    static_assert(internal::is_stateless<StatelessLambda>,
                  "|callable| must not capture any state.");
    return BindImpl<Callback, typename fit::callable_traits<StatelessLambda>::return_type>(
        callable, typename Pop<typename fit::callable_traits<StatelessLambda>::args>::pack{});
  }

  /// Mints a |Function| object that holds a capability to send |Args| to the
  /// owner object repeatedly. When the resulting |Function| is invoked on some
  /// other thread, |member| is scheduled to be called on the |dispatcher|.
  ///
  /// |member| should be a pointer to member function. It should have the
  /// function signature `void Owner::SomeMember(Args...)` where Args are some
  /// number of arguments.
  template <typename Member>
  auto Repeating(Member Owner::*member) {
    return BindImpl<Function, typename fit::callable_traits<Member>::return_type>(
        std::mem_fn(member), typename fit::callable_traits<Member>::args{});
  }

  /// Mints a |Function| object that holds a capability to send |Args| to the
  /// owner object repeatedly. When the resulting |Function| is invoked on some
  /// other thread, |callable| is scheduled to be called on the |dispatcher|.
  ///
  /// |callable| should be a lambda without any captures. It should have the
  /// function signature `void(Owner*, Args...)` where Args are some number of
  /// arguments.
  template <typename StatelessLambda>
  auto Repeating(StatelessLambda callable) {
    // TODO(https://fxbug.dev/42070737): We'll be able to support lambda with captures
    // given compiler tooling that inspects the capture list and determine
    // they're safe.
    static_assert(internal::is_stateless<StatelessLambda>,
                  "|callable| must not capture any state.");
    return BindImpl<Function, typename fit::callable_traits<StatelessLambda>::return_type>(
        callable, typename Pop<typename fit::callable_traits<StatelessLambda>::args>::pack{});
  }

 private:
  template <template <typename... Args> typename FunctionOrCallback, typename ReturnType,
            typename Callable, typename... Args>
  FunctionOrCallback<ReturnType(Args...)> BindImpl(Callable&& callable,
                                                   fit::parameter_pack<Args...>) {
    return FunctionOrCallback<ReturnType(Args...)>{
        task_queue_handle(),
        [callable = std::forward<Callable>(callable), owner = owner_](Args... args) mutable {
          internal::CheckArguments<Args...>::Check();
          internal::CheckArguments<ReturnType>::Check();
          return callable(owner, std::forward<Args>(args)...);
        }};
  }

  // |Pop| pops off the first element of a |fit::parameter_pack|.
  template <typename P>
  struct Pop;

  template <typename First, typename... Rest>
  struct Pop<fit::parameter_pack<First, Rest...>> {
    using pack = fit::parameter_pack<Rest...>;
  };

  Owner* owner_;
};

}  // namespace async_patterns

#endif  // LIB_ASYNC_PATTERNS_CPP_RECEIVER_H_
