// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_TESTING_NEXT_CPP_INTERNAL_SYNC_PROXY_BASE_H_
#define LIB_ASYNC_PATTERNS_TESTING_NEXT_CPP_INTERNAL_SYNC_PROXY_BASE_H_

#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>

// This macro defines a class named
// `AsyncPatternsSyncMemberFunctionMixin_{CLASS_NAME}_{NAME}`, which will contain
// a single method named `NAME`, that will take the same arguments as |MemberFunc|, and return
// the same return type as |MemberFunc|, by making a |SyncCall| on the |TestDispatcherBound|
// from the virtual base.
#define _ASYNC_PATTERNS_DEFINE_SYNC_METHOD_IMPL(CLASS_NAME, NAME)                             \
  template <typename MemberFunc, auto member_func>                                            \
  class AsyncPatternsSyncMemberFunctionMixin_##CLASS_NAME##_##NAME;                           \
                                                                                              \
  template <auto member_func, typename Ret, typename T, typename... Args>                     \
  class AsyncPatternsSyncMemberFunctionMixin_##CLASS_NAME##_##NAME<Ret (T::*)(Args...),       \
                                                                   member_func> {             \
    using MemberPointer = Ret (T::*)(Args...);                                                \
    async_patterns::TestDispatcherBound<T>* object_;                                          \
    MemberPointer f_;                                                                         \
                                                                                              \
   public:                                                                                    \
    AsyncPatternsSyncMemberFunctionMixin_##CLASS_NAME##_##NAME(                               \
        async_patterns::TestDispatcherBound<T>* object)                                       \
        : object_(object), f_(member_func) {}                                                 \
                                                                                              \
    Ret NAME(Args... args) { return object_->SyncCall(f_, std::forward<Args>(args)...); }     \
  };                                                                                          \
                                                                                              \
  template <auto member_func, typename Ret, typename T, typename... Args>                     \
  class AsyncPatternsSyncMemberFunctionMixin_##CLASS_NAME##_##NAME<Ret (T::*)(Args...) const, \
                                                                   member_func> {             \
    using MemberPointer = Ret (T::*)(Args...) const;                                          \
    async_patterns::TestDispatcherBound<T>* object_;                                          \
    MemberPointer f_;                                                                         \
                                                                                              \
   public:                                                                                    \
    AsyncPatternsSyncMemberFunctionMixin_##CLASS_NAME##_##NAME(                               \
        async_patterns::TestDispatcherBound<T>* object)                                       \
        : object_(object), f_(member_func) {}                                                 \
                                                                                              \
    Ret NAME(Args... args) { return object_->SyncCall(f_, std::forward<Args>(args)...); }     \
  }

#define _ASYNC_PATTERNS_ADD_SYNC_METHOD_IMPL(NAME, CLASS_NAME, MEMBER)                  \
  AsyncPatternsSyncMemberFunctionMixin_##NAME##_##MEMBER<decltype(&CLASS_NAME::MEMBER), \
                                                         &CLASS_NAME::MEMBER>

namespace async_patterns::internal {

// Utility class to inherit from all of |Args|, as well as |SyncProxyBase<T>|.
template <typename T, typename... Args>
class InheritFromArgs;

template <typename T>
class InheritFromArgs<T> {
 public:
  explicit InheritFromArgs(async_patterns::TestDispatcherBound<T>*) {}
};

template <typename T, typename First, typename... Rest>
class InheritFromArgs<T, First, Rest...> : public First, public InheritFromArgs<T, Rest...> {
 private:
  using Base_ = InheritFromArgs<T, Rest...>;

 public:
  explicit InheritFromArgs(async_patterns::TestDispatcherBound<T>* dispatcher_bound)
      : First(dispatcher_bound), Base_(dispatcher_bound) {}
};

}  // namespace async_patterns::internal

#endif  // LIB_ASYNC_PATTERNS_TESTING_NEXT_CPP_INTERNAL_SYNC_PROXY_BASE_H_
