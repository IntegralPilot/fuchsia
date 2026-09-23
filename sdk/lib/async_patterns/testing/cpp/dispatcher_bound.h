// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_TESTING_CPP_DISPATCHER_BOUND_H_
#define LIB_ASYNC_PATTERNS_TESTING_CPP_DISPATCHER_BOUND_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>
#include <lib/async_patterns/cpp/receiver.h>

#include <future>

namespace async_patterns {

// |TestDispatcherBound| adds the ability to issue synchronous calls on top of
// |DispatcherBound|. Synchronous test code may use this class instead of
// manually running an event loop.
//
// See |async_patterns::DispatcherBound| for general information.
//
// See |async_patterns::SyncProxy| which uses this class to manage a thread-unsafe
// object and expose synchronous wrapper methods for every member method you specify.
template <typename T>
class TestDispatcherBound final : public DispatcherBound<T> {
 public:
  // See |async_patterns::DispatcherBound| for how to construct a |DispatcherBound|.
  // The same applies to |TestDispatcherBound|.
  using DispatcherBound<T>::DispatcherBound;

  // See |async_patterns::DispatcherBound::emplace| for how to construct an object
  // inside a |DispatcherBound|. The same applies to |TestDispatcherBound::emplace|.
  using DispatcherBound<T>::emplace;

  // See |async_patterns::DispatcherBound::reset| for how to destroy an object
  // inside a |DispatcherBound|. The same applies to |TestDispatcherBound::reset|.
  using DispatcherBound<T>::reset;

  // Refer to |DispatcherBound<T>::AsyncCall| for documentation.
  //
  // This test version of |AsyncCall| adds the ability to encapsulate the result
  // of the call into a |std::future<R>|. This way, tests can synchronously block
  // on that future, leading to more direct code:
  //
  //     std::future<std::string> fut = some_dispatcher_bound
  //         .AsyncCall(&Object::GetSomeString)
  //         .ToFuture();
  //
  template <typename Member, typename... Args>
  auto AsyncCall(Member T::*member, Args&&... args) {
    ZX_ASSERT(DispatcherBound<T>::has_value());
    constexpr bool kIsInvocable = std::is_invocable_v<Member, Args...>;
    static_assert(kIsInvocable,
                  "|Member| must be callable with the provided |Args|. "
                  "Check that you specified each argument correctly to the |member| function.");
    if constexpr (kIsInvocable) {
      DispatcherBound<T>::CheckArgs(typename fit::callable_traits<Member>::args{});
      return DispatcherBound<T>::template UnsafeAsyncCallImpl<TestAsyncCallBuilder>(
          member, std::forward<Args>(args)...);
    }
  }

  // Schedules an asynchronous |callable| on |dispatcher| with |args| and then
  // block the caller until the asynchronous call is processed, returning the
  // result of the asynchronous call.
  //
  // |callable| should take |T*| as the first argument, followed by optionally
  // |Args| if any. A special case of |callable| is a pointer-to-member, e.g.
  // |&T::SomeFunction|. Example:
  //
  //     struct Foo {
  //       int Add(int a, int b) { return a + b; }
  //     };
  //     async_patterns::TestDispatcherBound<Foo> object{foo_loop.dispatcher()};
  //
  //     int result = object.SyncCall(&Object::Add, 1, 2);  // result == 3
  //
  // If |Object::Add| is an overloaded member function, you may disambiguate it
  // by spelling out its signature:
  //
  //     int result = object.SyncCall<int(int, int)>(&Object::Add, 1, 2);
  //
  // General callables are also supported. The provided callable can safely
  // capture state without data races, because the current thread will be
  // suspended while the dispatcher thread is accessing the captures. However,
  // if you capture a pointer or reference, be careful to not let them escape to
  // the wrapped object. When in doubt, only use the captured data inside the
  // lambda scope:
  //
  //     int result = object.SyncCall([&] (Foo* foo) {
  //         return foo->Add(1, 2);
  //     });  // result == 3
  //
  // If |callable| returns |void|, |SyncCall| will still block until the
  // |callable| finishes executing on the target |dispatcher|. To schedule a
  // fire-and-forget call, use |AsyncCall|.
  template <typename Callable, typename... Args>
  auto SyncCall(Callable&& callable, Args&&... args) {
    ZX_ASSERT(DispatcherBound<T>::has_value());
    constexpr bool kIsInvocable = std::is_invocable_v<Callable, T*, Args...>;
    static_assert(kIsInvocable,
                  "|Callable| must be callable with |T*| and the provided |Args|. "
                  "Check that you specified each argument correctly to the |callable| function.");
    if constexpr (kIsInvocable) {
      using Result = std::invoke_result_t<Callable, T*, Args...>;
      if constexpr (std::is_void_v<Result>) {
        // Wrap a void-returning function into one that returns a unit type.
        [[maybe_unused]] std::monostate unit = BlockOn(DispatcherBound<T>::UnsafeAsyncCallImpl(
            [callable = std::forward<Callable>(callable)](T* ptr, auto&&... args) mutable {
              std::invoke(callable, ptr, std::move(args)...);
              return std::monostate{};
            },
            std::forward<Args>(args)...));
        return;
      } else {
        return BlockOn(DispatcherBound<T>::UnsafeAsyncCallImpl(std::forward<Callable>(callable),
                                                               std::forward<Args>(args)...));
      }
    }
  }
  template <typename Member, typename... Args>
  auto SyncCall(Member T::*member, Args&&... args) {
    return SyncCall(std::mem_fn(member), std::forward<Args>(args)...);
  }

 private:
  template <typename ReturnType, typename Task, typename Submit>
  class TestAsyncCallBuilder;

  template <typename ReturnType, typename Task, typename Submit>
  std::invoke_result_t<Task> BlockOn(PendingCall<ReturnType, Task, Submit>&& pending_call) {
    using R = std::invoke_result_t<Task>;
    struct Slot {
      async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
      std::optional<R> result;
      async_patterns::Receiver<Slot> receiver{this, loop.dispatcher()};
    } slot;
    std::move(pending_call).Then(slot.receiver.Once([](Slot* slot, R result) {
      slot->result.emplace(std::move(result));
      slot->loop.Quit();
    }));
    ZX_ASSERT(slot.loop.Run() == ZX_ERR_CANCELED);
    return std::move(slot.result.value());
  }
};

// The return type of |TestDispatcherBound<T>::AsyncCall|. In addition to
// everything supported by |async_patterns::PendingCall|, it also supports
// transforming the pending result to a |std::future|.
template <typename T>
template <typename ReturnType, typename Task, typename Submit>
class TestDispatcherBound<T>::TestAsyncCallBuilder : public PendingCall<ReturnType, Task, Submit> {
 private:
  using Base = PendingCall<ReturnType, Task, Submit>;

 public:
  // Arranges the |on_result| callback to be called with the result of the async
  // call. See |DispatcherBound<T>::AsyncCall| for documentation.
  using Base::Then;

  // Gets an asynchronous promise that will be resolved with the result of the async
  // call. See |DispatcherBound<T>::AsyncCall| for documentation.
  using Base::promise;

  // Gets a |std::future| that will resolve with the return value of the
  // async call. Because one needs to block on an |std::future| to obtain its
  // value, this function is only for use in tests. Use |promise| or |Then| with a
  // |Callback| in production code instead.
  std::future<ReturnType> ToFuture() && {
    std::promise<ReturnType> promise;
    std::future<ReturnType> fut = promise.get_future();
    Base::CallWithContinuation(
        [promise = std::move(promise)](ReturnType r) mutable { promise.set_value(std::move(r)); });
    return fut;
  }

  using Base::Base;
};

// Constructs a |TestDispatcherBound<T>| that holds an instance of |T| by sending
// the |args| to the constructor of |T| run from a |dispatcher| task.
//
// See |TestDispatcherBound| constructor for details.
template <typename T, typename... Args>
TestDispatcherBound<T> MakeTestDispatcherBound(async_dispatcher_t* dispatcher, Args&&... args) {
  return TestDispatcherBound<T>{dispatcher, std::in_place, std::forward<Args>(args)...};
}

}  // namespace async_patterns

#endif  // LIB_ASYNC_PATTERNS_TESTING_CPP_DISPATCHER_BOUND_H_
