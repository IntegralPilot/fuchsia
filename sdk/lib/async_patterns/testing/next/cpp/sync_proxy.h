// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_TESTING_NEXT_CPP_SYNC_PROXY_H_
#define LIB_ASYNC_PATTERNS_TESTING_NEXT_CPP_SYNC_PROXY_H_

#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/async_patterns/testing/next/cpp/internal/sync_proxy_base.h>
#include <lib/fit/traits.h>

#define ASYNC_PATTERNS_DEFINE_SYNC_METHOD(CLASS_NAME, NAME) \
  _ASYNC_PATTERNS_DEFINE_SYNC_METHOD_IMPL(CLASS_NAME, NAME)

#define ASYNC_PATTERNS_ADD_SYNC_METHOD(NAME, CLASS_NAME, MEMBER) \
  _ASYNC_PATTERNS_ADD_SYNC_METHOD_IMPL(NAME, CLASS_NAME, MEMBER)

namespace async_patterns {

// |SyncProxy| is an ergonomic wrapper over |TestDispatcherBound| that simplifies
// the |SyncCall| syntax into direct function calls:
//
//     auto r = sync_proxy.Foo(arg);
//
// is equivalent to
//
//     auto r = test_dispatcher_bound.SyncCall(&T::Foo, arg);
//
// To use it, suppose there's a |Background| class with two methods |Foo|, |Bar|.
// We will choose |BackgroundSyncProxy| as the name of the sync proxy class.
// First tell the library about those methods using this macro:
//
//     ASYNC_PATTERNS_DEFINE_SYNC_METHOD(BackgroundSyncProxy, Foo);
//     ASYNC_PATTERNS_DEFINE_SYNC_METHOD(BackgroundSyncProxy, Bar);
//
// Then define |BackgroundSyncProxy| as the following type alias:
//
//     using BackgroundSyncProxy = async_patterns::SyncProxy<
//         Background,
//         ASYNC_PATTERNS_ADD_SYNC_METHOD(BackgroundSyncProxy, Background, Foo),
//         ASYNC_PATTERNS_ADD_SYNC_METHOD(BackgroundSyncProxy, Background, Bar)
//     >;
//
// After that, you may use |BackgroundSyncProxy| like so:
//
//     // The sync proxy supports the same construction API as |TestDispatcherBound|.
//     BackgroundSyncProxy background(dispatcher);
//     background.emplace(constructor_args);
//
//     // But instead of |SyncCall|, you can directly call |Foo| and |Bar|.
//     auto return_value = background.Foo(...);
//     auto return_value = background.Bar(...);
//
// |SyncProxy| methods cannot be overloaded, or templated. If one of your member functions
// uses those features, you may call it through dispatcher bound:
//
//     background
//         .dispatcher_bound()
//         .SyncCall<void(int, int)>(&Background::OverloadedFunction, 1, 2);
//
template <typename T, typename... Methods>
class SyncProxy : public internal::InheritFromArgs<T, Methods...> {
 private:
  using Methods_ = internal::InheritFromArgs<T, Methods...>;

 public:
  // Constructs a sync proxy with a |TestDispatcherBound| holding an object.
  //
  // See |async_patterns::TestDispatcherBound| for more information.
  template <typename... Args>
  explicit SyncProxy(async_dispatcher_t* dispatcher, std::in_place_t, Args&&... args)
      : Methods_(&inner_), inner_(dispatcher, std::in_place, std::forward<Args>(args)...) {}

  // Constructs a sync proxy with an empty |TestDispatcherBound|.
  explicit SyncProxy(async_dispatcher_t* dispatcher) : Methods_(&inner_), inner_(dispatcher) {}

  // Constructs an object inside the |TestDispatcherBound|.
  //
  // See |async_patterns::DispatcherBound<T>::emplace| for more information.
  template <typename T2 = T, typename... Args>
  void emplace(Args&&... args) {
    inner_.template emplace<T2>(std::forward<Args>(args)...);
  }

  // If |has_value|, asynchronously destroys the managed |T| on a task
  // posted to the dispatcher.
  //
  // See |async_patterns::DispatcherBound<T>::reset| for more information.
  void reset() { inner_.reset(); }

  // Returns if this object holds an instance of |T|.
  //
  // See |async_patterns::DispatcherBound<T>::reset| for more information.
  bool has_value() const { return inner_.has_value(); }

  // Returns a reference to the underlying |TestDispatcherBound|.
  async_patterns::TestDispatcherBound<T>& dispatcher_bound() { return inner_.dispatcher_bound(); }

 private:
  SyncProxy(SyncProxy&&) noexcept = delete;
  SyncProxy& operator=(SyncProxy&&) noexcept = delete;

  SyncProxy(const SyncProxy&) noexcept = delete;
  SyncProxy& operator=(const SyncProxy&) noexcept = delete;

  async_patterns::TestDispatcherBound<T> inner_;
};

}  // namespace async_patterns

#endif  // LIB_ASYNC_PATTERNS_TESTING_NEXT_CPP_SYNC_PROXY_H_
