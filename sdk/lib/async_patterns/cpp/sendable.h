// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_CPP_SENDABLE_H_
#define LIB_ASYNC_PATTERNS_CPP_SENDABLE_H_

#include <lib/async_patterns/cpp/internal/sendable.h>
#include <lib/stdcompat/type_traits.h>

#include <tuple>
#include <type_traits>

namespace async_patterns {

/// |BindForSending| function packages up |args| so that they may be later
/// executed as arguments to |callable| in a different execution context,
/// asynchronously and possibly concurrently. This is called "sending". For
/// example, |DispatcherBound| uses |BindForSending| to send arguments to the
/// object owned by |DispatcherBound|, as part of asynchronously calling their
/// member functions.
///
/// It is similar to |std::bind_front|, but will first forward the |args| into
/// the capture state of the resulting lambda, and then move the captured args
/// into |callable| when invoked later.
///
/// ## Safety of sending arguments
///
/// Because the |args| are used at a later time, one needs to be aware of risks
/// of use-after-free and data races. There are also the following restrictions:
///
/// - When sending a ref-counted pointer such as |std::shared_ptr<Arg>|, both the
///   sender and the |callable| will be able to concurrently access |Arg|. Be
///   sure to add appropriate synchronization to |Arg|, for example protecting
///   fields with mutexes. This pattern is called "shared-state concurrency".
///
/// - Value types may be sent via copying or moving. Whether one passes an |Arg|,
///   |const Arg&|, or |Arg&&| etc on the sending side, the |callable| will have
///   its own uniquely owned instance that they may safely manipulate.
///
/// - Move-only types such as |std::unique_ptr<Arg>| may be sent via |std::move|.
///
/// - The |callable| may not take raw pointer or non-const reference arguments.
///   Sending those may result in use-after-frees when the callable uses a
///   pointee asynchronously after an unspecified amount of time. It's not worth
///   the memory safety risks to support the cases where a pointee outlives the
///   dispatcher that is running tasks for |T|.
///
/// - If the |callable| takes |const Arg&| as an argument, one may send an
///   instance of |Arg| via copying or moving. The instance will live until the
///   callable finishes executing.
///
/// - These checks are performed for each argument, but there could still be raw
///   pointers or references inside an object that is sent. One must ensure that
///   the pointee remain alive until the asynchronous tasks are run, possibly
///   indefinitely until the dispatcher is destroyed.
///
template <typename Callable, typename... Args>
auto BindForSending(Callable callable, Args&&... args) {
  // An individual |Arg| in |Args| is either an l-value reference or an r-value
  // reference, due to universal reference rules.
  //
  // Therefore, this check ensures the callable matches its arguments. If the
  // user passes an l-value to a move-only argument, the |is_invocable| will
  // fail, because it will try to invoke a function taking |Arg| with |Arg&|.
  static_assert(std::is_invocable_v<Callable, Args...>,
                "The |Callable| function must be invocable with argument |Args|. "
                "Check that the arguments provided are correct. "
                "For example, did you call a function that consumes a move-only type |Foo| "
                "with an argument type of |Foo&| instead of |Foo&&|?");

  // We package the arguments into a tuple because capturing a template
  // parameter pack is a C++20 feature.
  //
  // |make_tuple| moves |Arg&&| into |Arg|, and copies |const Arg&| into |Arg|.
  // c.f. https://en.cppreference.com/w/cpp/utility/tuple/make_tuple
  // The behavior of |make_tuple| w.r.t. |Arg&| is not relevant here since we
  // disallow sending non-const references.
  return [callable = std::move(callable),
          args = std::make_tuple(std::forward<Args>(args)...)]() mutable {
    // The |std::move| inside prevents |callable| from taking |Arg&| arguments.
    // Unfortunately, unlike pointer arguments, there is no good way to reject
    // non-const l-value references, because the universal reference rules will
    // make |Arg| an l-value reference even when the user provided |Arg| by
    // value. We should instead check that the underlying |callable| don't
    // _also_ accept an l-value reference -- library code using |BindForSending|
    // should use |internal::CheckArguments| on the relevant member function.
    // Here, as an extra safety measure, we always move each |arg| into the
    // call. A non-const l-value reference will not be able to bind to a
    // non-const r-value reference.
    //
    // ** NOTE TO USERS ** If you get a compiler error pointing to this
    // vicinity, check the documentation comments of |BindForSending|.
    // Passing mutable references and raw pointers are not allowed.
    return std::apply(
        [callable = std::move(callable)](auto&&... args) mutable {
          constexpr auto check_arg = [](auto&& arg) {
            using Arg = decltype(arg);
            static_assert(!std::is_pointer_v<cpp20::remove_cvref_t<Arg>>,
                          "Sending raw pointers is not allowed. "
                          "See class documentation comments of |BindForSending|.");
            return true;
          };
          (void)(check_arg(args) && ...);

          return callable(internal::UnSmuggle(std::move(args))...);
        },
        args);
  };
}

namespace internal {

template <typename Arg>
constexpr void CheckOneArg() {
  static_assert(!std::is_lvalue_reference_v<Arg> || std::is_const_v<std::remove_reference_t<Arg>>,
                "Sending non-const l-value references is not allowed. "
                "See class documentation comments of |BindForSending|.");
}

template <typename... Args>
struct CheckArguments {
  static constexpr void Check() {}
};

template <typename Arg, typename... Rest>
struct CheckArguments<Arg, Rest...> {
  static constexpr void Check() {
    CheckOneArg<Arg>();
    CheckArguments<Rest...>::Check();
  }
};

}  // namespace internal

}  // namespace async_patterns

#endif  // LIB_ASYNC_PATTERNS_CPP_SENDABLE_H_
