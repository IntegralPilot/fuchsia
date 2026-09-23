// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDKTL_INCLUDE_DDKTL_DEVICE_INTERNAL_H_
#define SRC_LIB_DDKTL_INCLUDE_DDKTL_DEVICE_INTERNAL_H_

#include <lib/ddk/device.h>
#include <lib/fdf/cpp/channel.h>

#include <string>
#include <type_traits>

#include <ddktl/fidl.h>
#include <ddktl/init-txn.h>
#include <ddktl/suspend-txn.h>
#include <ddktl/unbind-txn.h>

namespace ddk {
namespace internal {

// Macro for defining a trait that checks if a type T has a method with the
// given name. See fbl/macros.h.
//
// Example:
//
// DDTL_INTERNAL_DECLARE_HAS_MEMBER_FN(has_bar, Bar);
// template <typename T>
// class Foo {
//   static_assert(has_bar_v<T>, "Foo classes must implement Bar()!");
//   lands.
//   static_assert(is_same_v<decltype(&T::Bar), void (T::*)(int)>,
//                 "Bar must be a non-static member function with signature "
//                 "'void Bar(int)', and must be visible to Foo (either "
//                 "because it is public, or due to friendship).");
//  };
#define DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN(trait_name, fn_name)    \
  template <typename T>                                              \
  struct trait_name {                                                \
   private:                                                          \
    template <typename C>                                            \
    static std::true_type test(decltype(&C::fn_name));               \
    template <typename C>                                            \
    static std::false_type test(...);                                \
                                                                     \
   public:                                                           \
    static constexpr bool value = decltype(test<T>(nullptr))::value; \
  };                                                                 \
  template <typename T>                                              \
  static inline constexpr bool trait_name##_v = trait_name<T>::value

// Example:
//
// DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_c_str, c_str, const char* (C::*)()
// const);
#define DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(trait_name, fn_name, sig) \
  template <typename T>                                                               \
  struct trait_name {                                                                 \
   private:                                                                           \
    template <typename C>                                                             \
    static std::true_type test(decltype(static_cast<sig>(&C::fn_name)));              \
    template <typename C>                                                             \
    static std::false_type test(...);                                                 \
                                                                                      \
   public:                                                                            \
    static constexpr bool value = decltype(test<T>(nullptr))::value;                  \
  };                                                                                  \
  template <typename T>                                                               \
  static inline constexpr bool trait_name##_v = trait_name<T>::value

// Implemented like protocol op mixins, but defined on all base_device since they must implement
template <typename D>
class Releasable {
 public:
  static constexpr void InitOp(zx_protocol_device_t* proto) { proto->release = Release; }

 private:
  static void Release(void* ctx) { return static_cast<D*>(ctx)->DdkRelease(); }
};

// base_device is a tag that default initializes the zx_protocol_device_t so the mixin classes
// can fill in the table.
template <class D, template <typename> class... Mixins>
class base_device : public Mixins<D>... {
 protected:
  explicit base_device(zx_device_t* parent) : parent_(parent) {}

  // populated ops table at compile time. All Mixins must have a constexpr
  // InitOp function that manipulates ops for their respective function
  static const constexpr zx_protocol_device_t ddk_device_proto_ = []() {
    zx_protocol_device_t ops = {};
    ops.version = DEVICE_OPS_VERSION;
    // Releasable Mixin
    Releasable<D>::InitOp(&ops);
    //
    // This C++17 fold expression with the , operator means this ("iterating" at compile time):
    // for (typename Mixin : Mixins<D>) Mixin::Protocol(&ops);
    //
    // template <typename D>
    // class GetProtocolable : public base_mixin {
    //  protected:
    //   static constexpr void InitOp(zx_protocol_device_t* proto) {
    //     internal::CheckGetProtocolable<D>();
    //     proto->get_protocol = GetProtocol;
    //   }
    //
    //  private:
    //   static zx_status_t GetProtocol(void* ctx, uint32_t proto_id, void* out) {
    //     return static_cast<D*>(ctx)->DdkGetProtocol(proto_id, out);
    //   }
    // };
    (Mixins<D>::InitOp(&ops), ...);
    return ops;
  }();

  std::string name_;
  zx_device_t* zxdev_ = nullptr;
  zx_device_t* const parent_;
};

// base_mixin is a tag that all mixins must inherit from.
struct base_mixin {};

// base_protocol is a tag used by protocol implementations
struct base_protocol {
  uint32_t ddk_proto_id_ = 0;
  void* ddk_proto_ops_ = nullptr;

 protected:
  base_protocol() = default;
};

template <typename T>
using is_base_protocol = std::is_base_of<internal::base_protocol, T>;

template <typename T>
using is_base_proto = std::is_same<internal::base_protocol, T>;

// Deprecation helpers: transition a DDKTL protocol interface when there are implementations outside
// of zircon without breaking any builds.
//
// Example:
// template <typename D, bool NewFooMethod=false>
// class MyProtocol : public internal::base_protocol {
//   public:
//     MyProtocol() {
//         internal::CheckMyProtocol<D, NewMethod>();
//         ops_.foo = Foo;
//
//         // Can only inherit from one base_protocol implementation
//         ZX_ASSERT(this->ddk_proto_ops_ == nullptr);
//         ddk_proto_id_ = ZX_PROTOCOL_MY_FOO;
//         ddk_proto_ops_ = &ops_;
//     }
//
//   private:
//     DDKTL_DEPRECATED(NewFooMethod)
//     static Foo(void* ctx, uint32_t options, foo_info_t* info) {
//         return static_cast<D*>(ctx)->MyFoo(options);
//     }
//
//     DDKTL_NOTREADY(NewFooMethod)
//     static Foo(void* ctx, uint32_t options, foo_info_t* info) {
//         return static_cast<D*>(ctx)->MyFoo(options, info);
//     }
// };
//
// // This class hasn't been updated yet, so it uses the default value for NewFooMethod and has the
// // old MyFoo method implementation.
// class MyProtocolImpl : public ddk::Device<MyProtocolImpl, /* other mixins */>,
//                        public ddk::MyProtocol<MyProtocolImpl> {
//   public:
//     zx_status_t MyFoo(uint32_t options);
// };
//
// // The implementation transitions as follows:
// class MyProtocolImpl : public ddk::Device<MyProtocolImpl, /* other mixins */>,
//                        public ddk::MyProtocol<MyProtocolImpl, true> {
//   public:
//     zx_status_t MyFoo(uint32_t options, foo_info_t* info);
// };
//
// Now the DDKTL_DEPRECATED method can be removed, along with the NewFooMethod template parameter.
// At no stage should the build be broken. These annotations may also be used to add new methods, by
// providing a no-op in the DDKTL_DEPRECATED static method.
#define DDKTL_DEPRECATED(condition)              \
  template <typename T = D, bool En = condition, \
            typename std::enable_if<!std::integral_constant<bool, En>::value, int>::type = 0>
#define DDKTL_NOTREADY(condition)                \
  template <typename T = D, bool En = condition, \
            typename std::enable_if<std::integral_constant<bool, En>::value, int>::type = 0>

// Mixin checks: ensure that a type meets the following qualifications:
//
// 1) has a method with the correct name (this makes the compiler errors a little more sane),
// 2) inherits from ddk::Device (by checking that it inherits from ddk::internal::base_device), and
// 3) has the correct method signature.
//
// Note that the 3rd requirement supersedes the first, but the static_assert doesn't even compile if
// the method can't be found, leading to a slightly more confusing error message. Adding the first
// check gives a chance to show the user a more intelligible error message.

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN(has_ddk_get_protocol, DdkGetProtocol);

template <typename D>
constexpr void CheckGetProtocolable() {
  static_assert(has_ddk_get_protocol<D>::value,
                "GetProtocolable classes must implement DdkGetProtocol");
  static_assert(
      std::is_same<decltype(&D::DdkGetProtocol), zx_status_t (D::*)(uint32_t, void*)>::value,
      "DdkGetProtocol must be a public non-static member function with signature "
      "'zx_status_t DdkGetProtocol(uint32_t, void*)'.");
}

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN(has_ddk_init, DdkInit);

template <typename D>
constexpr void CheckInitializable() {
  static_assert(has_ddk_init<D>::value, "Init classes must implement DdkInit");
  static_assert(std::is_same<decltype(&D::DdkInit), void (D::*)(InitTxn txn)>::value,
                "DdkInit must be a public non-static member function with signature "
                "'void DdkInit(ddk::InitTxn)'.");
}

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN(has_ddk_unbind, DdkUnbind);

template <typename D>
constexpr void CheckUnbindable() {
  static_assert(has_ddk_unbind<D>::value, "Unbindable classes must implement DdkUnbind");
  static_assert(std::is_same<decltype(&D::DdkUnbind), void (D::*)(UnbindTxn txn)>::value,
                "DdkUnbind must be a public non-static member function with signature "
                "'void DdkUnbind(ddk::UnbindTxn)'.");
}

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN(has_ddk_release, DdkRelease);

template <typename D>
constexpr void CheckReleasable() {
  static_assert(has_ddk_release<D>::value, "Releasable classes must implement DdkRelease");
  static_assert(std::is_same<decltype(&D::DdkRelease), void (D::*)(void)>::value,
                "DdkRelease must be a public non-static member function with signature "
                "'void DdkRelease()'.");
}

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN(has_ddk_service_connect, DdkServiceConnect);

template <typename D>
constexpr void CheckServiceConnectable() {
  static_assert(has_ddk_service_connect<D>::value,
                "ServiceConnectable classes must implement DdkServiceConnect");
  static_assert(std::is_same<decltype(&D::DdkServiceConnect),
                             zx_status_t (D::*)(const char*, fdf::Channel)>::value,
                "DdkServiceConnect must be a public non-static member function with signature "
                "'zx_status_t DdkServiceConnect(const char*, fdf::Channel)'.");
}

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN(has_ddk_suspend, DdkSuspend);

template <typename D>
constexpr void CheckSuspendable() {
  static_assert(has_ddk_suspend<D>::value, "Suspendable classes must implement DdkSuspend");
  static_assert(std::is_same<decltype(&D::DdkSuspend), void (D::*)(SuspendTxn txn)>::value,
                "DdkSuspend must be a public non-static member function with signature "
                "'zx_status_t DdkSuspend(SuspendTxn txn)'.");
}

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN(has_ddk_child_pre_release, DdkChildPreRelease);

template <typename D>
constexpr void CheckChildPreReleaseable() {
  static_assert(has_ddk_child_pre_release<D>::value,
                "ChildPreReleaseable classes must implement DdkChildPreRelease");
  static_assert(std::is_same<decltype(&D::DdkChildPreRelease), void (D::*)(void*)>::value,
                "DdkChildPreRelease must be a public non-static member function with signature "
                "'void DdkChildPreRelease(void*)'.");
}

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN(has_ddk_made_visible, DdkMadeVisible);

template <typename D>
constexpr void CheckMadeVisibleable() {
  static_assert(has_ddk_made_visible<D>::value,
                "MadeVisableable classes must implement DdkMadeVisible");
  static_assert(std::is_same_v<decltype(&D::DdkMadeVisible), void (D::*)()>,
                "DdkMadeVisible must be a public non-static member function with signature "
                "'void DdkMadeVisible()'.");
}

// all_mixins
//
// Checks a list of types to ensure that all of them are ddk mixins (i.e., they inherit from the
// internal::base_mixin tag).
template <typename Base, typename...>
struct all_mixins : std::true_type {};

template <typename Base, typename Mixin, typename... Mixins>
struct all_mixins<Base, Mixin, Mixins...>
    : std::integral_constant<bool, std::is_base_of<Base, Mixin>::value &&
                                       all_mixins<Base, Mixins...>::value> {};

template <typename... Mixins>
constexpr void CheckMixins() {
  static_assert(all_mixins<base_mixin, Mixins...>::value,
                "All mixins must be from the ddk template library");
}

}  // namespace internal
}  // namespace ddk

#endif  // SRC_LIB_DDKTL_INCLUDE_DDKTL_DEVICE_INTERNAL_H_
