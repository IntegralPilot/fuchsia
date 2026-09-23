// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_CPP_DISPATCHER_BOUND_H_
#define LIB_ASYNC_PATTERNS_CPP_DISPATCHER_BOUND_H_

#include <lib/async/dispatcher.h>
#include <lib/async_patterns/cpp/internal/dispatcher_bound_storage.h>
#include <lib/async_patterns/cpp/pending_call.h>
#include <lib/fit/function.h>
#include <lib/fit/function_traits.h>
#include <lib/stdcompat/functional.h>
#include <zircon/assert.h>

#include <cstdlib>
#include <utility>

namespace async_patterns {

/// |DispatcherBound<T>| does not allow sending raw pointers to the wrapped
/// object. However, it is common for an async object to obtain its associated
/// |async_dispatcher_t*|. Often that can be accomplished with
/// |async_get_default_dispatcher|, but in case where that's not feasible, one
/// may specify the |async_patterns::PassDispatcher| constant in place of an
/// |async_dispatcher_t*|, at the argument location where the wrapped async
/// object desires a dispatcher, and |DispatcherBound| will automatically supply
/// the correct dispatcher that the async object is associated with.
constexpr auto PassDispatcher = internal::PassDispatcherT{};

/// |DispatcherBound<T>| enables an owner object living on some arbitrary thread,
/// to construct, call methods on, and destroy an object of type |T| that must be
/// used from a particular [synchronized async dispatcher][synchronized-dispatcher].
///
/// Thread-unsafe asynchronous types should be used from synchronized dispatchers
/// (e.g. a single-threaded async loop). Because the dispatcher may be running
/// code to manipulate such objects, one should not use the same objects from
/// other unrelated threads and cause data races.
///
/// However, it may not always be possible for an entire tree of objects to
/// live on the same async dispatcher, due to design or legacy constraints.
/// |DispatcherBound| helps one divide classes along dispatcher boundaries.
///
/// An example:
///
///     // |Background| always lives on a background dispatcher, provided
///     // at construction time.
///     class Background {
///      public:
///       explicit Background() {
///         // Perform some asynchronous work. The work is canceled if
///         // |Background| is destroyed.
///         task_.Post(async_get_default_dispatcher());
///       }
///
///      private:
///       void DoSomething();
///
///       // |task_| manages an async task that borrows the containing
///       // |Background| object and is not thread safe. It must be destroyed
///       // on the dispatcher to ensure that task cancellation is not racy.
///       async::TaskClosureMethod<Background, &Background::DoSomething> task_{this};
///     };
///
///     class Owner {
///      public:
///       // Asynchronously constructs a |Background| object on its dispatcher.
///       // Code in |Owner| and code in |Background| may run concurrently.
///       //
///       // The dispatcher will not be attached to the current thread, but will
///       // be attached to the loop thread. This way, the |Background| object
///       // can obtain a dispatcher from its constructor using
///       // |async_get_default_dispatcher|.
///       explicit Owner() :
///           background_loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
///           background_{background_loop_.dispatcher(), std::in_place} {}
///
///      private:
///       // The async loop which will manage |Background| objects.
///       // This will always be paired with a |DispatcherBound| object.
///       async::Loop background_loop_;
///
///       // The |DispatcherBound| which manages |Background| on its loop.
///       // During destruction, |background_| will schedule the asynchronous
///       // destruction of the wrapped |Background| object on the dispatcher.
///       async_patterns::DispatcherBound<Background> background_;
///     };
///
/// |DispatcherBound| itself is thread-compatible.
///
/// ## Safety of sending arguments
///
/// When constructing |T| and calling member functions of |T|, it is possible to
/// pass additional arguments if the constructor or member function requires it.
/// The argument will be forwarded from the caller's thread into a heap data
/// structure, and later moved into the thread which would run the dispatcher
/// task asynchronously. Each argument must be safe to send to a different
/// thread. See |async_patterns::BindForSending| for the detailed requirements.
///
/// [synchronized-dispatcher]:
/// https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/thread-safe-async#synchronized-dispatcher
template <typename T>
class DispatcherBound {
 public:
  // Asynchronously constructs |T| on a task posted to |dispatcher|.
  ///
  /// Arguments after |std::in_place| are sent to the constructor of |T|.
  /// See |async_patterns::BindForSending| for detailed requirements on |args|.
  ///
  /// If you'd like to pass a |dispatcher| to |T| as a constructor argument,
  /// see |async_patterns::PassDispatcher|.
  ///
  /// If the dispatcher is shutdown, |T| will be synchronously constructed.
  template <typename... Args>
  explicit DispatcherBound(async_dispatcher_t* dispatcher, std::in_place_t, Args&&... args)
      : dispatcher_(dispatcher) {
    storage_.Construct<T, T>(dispatcher, std::forward<Args>(args)...);
  }

  /// Constructs a |DispatcherBound| that does not hold an instance of |T|.
  ///
  /// One may later construct |T| using |emplace| on the |dispatcher|.
  explicit DispatcherBound(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  /// Asynchronously constructs |T| on a task posted to the dispatcher.
  ///
  /// If this object already holds an instance of |T|, that older instance will
  /// be asynchronously destroyed on the dispatcher.
  ///
  /// If |T2| is specified, it must be same as |T| or a subclass. Then an instance
  /// of |T2| will be constructed. This can be useful for mocking: |T| may be some
  /// interface, and when constructing the object, either a fake (in unit tests)
  /// or a real concrete type (in production) will be specified.
  ///
  /// If you'd like to pass a |dispatcher| to |T| as a constructor argument,
  /// see |async_patterns::PassDispatcher|.
  ///
  /// See |async_patterns::BindForSending| for detailed requirements on |args|.
  template <typename T2 = T, typename... Args>
  void emplace(Args&&... args) {
    static_assert(std::is_base_of_v<T, T2>, "|T| must be a base class of |T2|.");
    reset();
    storage_.Construct<T, T2>(dispatcher_, std::forward<Args>(args)...);
  }

  /// Asynchronously calls |member|, a pointer to member function of |T|, using
  /// the provided |args|.
  ///
  /// |AsyncCall| returns a |PendingCall| object that lets you asynchronously
  /// monitor the result. You may either:
  ///
  /// - Make a fire-and-forget call, by discarding the returned object, or
  /// - Get a promise carrying the return value of the function by calling
  ///   `promise()` on the object, yielding a |fpromise::promise<ReturnType>|, or
  /// - Call `Then()` on the object and pass a |Callback<void(ReturnType)>|.
  ///
  /// See |PendingCall| for details.
  ///
  /// In particular, if |member| returns void, you could attach promises/callbacks
  /// that take void to asynchronously get notified when |member| has finished execution.
  ///
  /// Example:
  ///
  ///     class Owner {
  ///      public:
  ///       Owner(async_dispatcher_t* owner_dispatcher) : receiver_{this, owner_dispatcher} {
  ///         background_.emplace();
  ///         // Tell |background_| to |DoSomething|, then send back the return
  ///         // value to |Owner| using |receiver_|.
  ///         background_
  ///             .AsyncCall(&Background::DoSomething)
  ///             .Then(receiver_.Once(&Owner::DoneSomething));
  ///       }
  ///
  ///       void DoneSomething(Result result) {
  ///         // |Background::DoSomething| has completed with |result|...
  ///       }
  ///
  ///      private:
  ///       async::Loop background_loop_;
  ///       async_patterns::DispatcherBound<Background> background_{background_loop_.dispatcher()};
  ///       async_patterns::Receiver<Owner> receiver_;
  ///     };
  ///
  /// See |async_patterns::BindForSending| for detailed requirements on |args|.
  ///
  /// If |Background::DoSomething| is an overloaded member function, you may
  /// disambiguate it by spelling out its signature:
  ///
  ///     background_.AsyncCall<void(Result)>(&Background::DoSomething);
  ///
  /// The task will be synchronously called if the dispatcher is shutdown.
  template <typename Member, typename... Args>
  auto AsyncCall(Member T::*member, Args&&... args) {
    ZX_ASSERT(has_value());
    constexpr bool kIsInvocable = std::is_invocable_v<Member, Args...>;
    static_assert(kIsInvocable,
                  "|Member| must be callable with the provided |Args|. "
                  "Check that you specified each argument correctly to the |member| function.");
    if constexpr (kIsInvocable) {
      CheckArgs(typename fit::callable_traits<Member>::args{});
      return UnsafeAsyncCallImpl(member, std::forward<Args>(args)...);
    }
  }

  /// Typically, asynchronous classes would contain internal self-pointers that
  /// make moving dangerous, so we disable moves here for now.
  DispatcherBound(DispatcherBound&&) noexcept = delete;
  DispatcherBound& operator=(DispatcherBound&&) noexcept = delete;

  DispatcherBound(const DispatcherBound&) noexcept = delete;
  DispatcherBound& operator=(const DispatcherBound&) noexcept = delete;

  /// If |has_value|, asynchronously destroys the managed |T| on a task
  /// posted to the dispatcher.
  ///
  /// If the dispatcher is shutdown, |T| will be synchronously destroyed.
  ~DispatcherBound() { reset(); }

  /// If |has_value|, asynchronously destroys the managed |T| on a task
  /// posted to the dispatcher.
  ///
  /// If the dispatcher is shutdown, |T| will be synchronously destroyed.
  void reset() {
    if (!has_value()) {
      return;
    }
    storage_.Destruct(dispatcher_);
  }

  /// Returns if this object holds an instance of |T|.
  bool has_value() const { return storage_.has_value(); }

 protected:
  /// Calls an arbitrary |callable| asynchronously on the |dispatcher_|.
  template <template <typename, typename, typename> typename Builder = PendingCall,
            typename Callable, typename... Args>
  auto UnsafeAsyncCallImpl(Callable&& callable, Args&&... args) {
    using Result = std::invoke_result_t<Callable, T*, Args...>;
    return storage_.AsyncCall<Builder, Result, T>(dispatcher_, std::forward<Callable>(callable),
                                                  std::forward<Args>(args)...);
  }

  template <typename... Args>
  constexpr void CheckArgs(fit::parameter_pack<Args...>) {
    internal::CheckArguments<Args...>::Check();
  }

 private:
  async_dispatcher_t* dispatcher_;
  internal::DispatcherBoundStorage storage_;
};

/// Constructs a |DispatcherBound<T>| that holds an instance of |T| by sending
/// the |args| to the constructor of |T| run from a |dispatcher| task.
///
/// See |DispatcherBound| constructor for details.
template <typename T, typename... Args>
DispatcherBound<T> MakeDispatcherBound(async_dispatcher_t* dispatcher, Args&&... args) {
  return DispatcherBound<T>{dispatcher, std::in_place, std::forward<Args>(args)...};
}

}  // namespace async_patterns

#endif  // LIB_ASYNC_PATTERNS_CPP_DISPATCHER_BOUND_H_
