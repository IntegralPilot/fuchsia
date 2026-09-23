// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/stdcompat/internal/atomic.h"

#include <lib/stdcompat/atomic.h>
#include <lib/stdcompat/type_traits.h>
#include <unistd.h>

#include <array>
#include <limits>
#include <ostream>
#include <type_traits>
#include <utility>

#include "gtest.h"
namespace {

using int128_t = __int128;
using uint128_t = unsigned __int128;

template <typename T, typename = void>
static constexpr bool kHasDifferenceType = false;

template <typename T>
static constexpr bool
    kHasDifferenceType<T, std::enable_if_t<!std::is_same_v<typename T::value_type, bool> &&
                                           (std::is_pointer_v<typename T::value_type> ||
                                            std::is_integral_v<typename T::value_type> ||
                                            std::is_floating_point_v<typename T::value_type>)>> =
        true;

// Workaround for libc++ not supporting qualified types for atomic ref.
// BUG(https://fxbug.dev/352337748): Unblocks toolchain roll.
// BUG(https://github.com/llvm/llvm-project/issues/98689): Upstream libc++ bug.
#if defined(_LIBCPP_VERSION)
template <typename T>
using LibcxxBugWorkaround = std::remove_cv_t<T>;
#else
template <typename T>
using LibcxxBugWorkaround = T;
#endif

// For signature name, the specialization type T, can be used.
#define ATOMIC_REF_HAS_METHOD_TRAIT(trait_name, fn_name, sig)            \
                                                                         \
  template <typename T, typename = void>                                 \
  struct trait_name {                                                    \
    static constexpr bool value = false;                                 \
  };                                                                     \
                                                                         \
  template <typename T>                                                  \
  struct trait_name<T, std::enable_if_t<kHasDifferenceType<T>>> {        \
   private:                                                              \
    template <typename C>                                                \
    static std::true_type test(decltype(static_cast<sig>(&C::fn_name))); \
    template <typename C>                                                \
    static std::false_type test(...);                                    \
                                                                         \
   public:                                                               \
    static constexpr bool value = decltype(test<T>(nullptr))::value;     \
  };                                                                     \
  template <typename T>                                                  \
  static inline constexpr bool trait_name##_v = trait_name<T>::value

ATOMIC_REF_HAS_METHOD_TRAIT(specialization_has_fetch_sub, fetch_add,
                            std::remove_volatile_t<typename T::value_type> (C::*)(
                                std::remove_volatile_t<typename T::difference_type>,
                                std::memory_order) const noexcept);
ATOMIC_REF_HAS_METHOD_TRAIT(specialization_has_fetch_add, fetch_sub,
                            std::remove_volatile_t<typename T::value_type> (C::*)(
                                std::remove_volatile_t<typename T::difference_type>,
                                std::memory_order) const noexcept);
ATOMIC_REF_HAS_METHOD_TRAIT(specialization_has_fetch_or, fetch_or,
                            std::remove_volatile_t<typename T::value_type> (C::*)(
                                std::remove_volatile_t<typename T::difference_type>,
                                std::memory_order) const noexcept);
ATOMIC_REF_HAS_METHOD_TRAIT(specialization_has_fetch_and, fetch_and,
                            std::remove_volatile_t<typename T::value_type> (C::*)(
                                std::remove_volatile_t<typename T::difference_type>,
                                std::memory_order) const noexcept);
ATOMIC_REF_HAS_METHOD_TRAIT(specialization_has_fetch_xor, fetch_xor,
                            std::remove_volatile_t<typename T::value_type> (C::*)(
                                std::remove_volatile_t<typename T::difference_type>,
                                std::memory_order) const noexcept);

template <typename T>
void CheckIntegerSpecialization() {
  using U = LibcxxBugWorkaround<T>;
  static_assert(std::is_same_v<typename cpp20::atomic_ref<U>::value_type, U>, "");
  static_assert(std::is_same_v<typename cpp20::atomic_ref<U>::difference_type, U>, "");
  static_assert(cpp20::atomic_ref<U>::required_alignment == std::max(alignof(U), sizeof(U)), "");
  static_assert(kHasDifferenceType<cpp20::atomic_ref<U>>, "");
}

template <typename T, typename U>
std::enable_if_t<std::is_pointer_v<T>, T> cast_helper(U u) {
  return reinterpret_cast<T>(u);
}

template <typename T, typename U>
std::enable_if_t<!std::is_pointer_v<T>, std::remove_volatile_t<T>> cast_helper(U u) {
  return static_cast<T>(u);
}

template <typename T, typename = void>
struct CheckAtomicOpsMutable {
  static void CheckStoreAndExchange() {}
};

template <typename T>
struct CheckAtomicOpsMutable<T, std::enable_if_t<!std::is_const_v<T>>> {
  static void CheckStoreAndExchange() {
    using U = LibcxxBugWorkaround<T>;

    U zero = cast_helper<U>(0);
    U one = cast_helper<U>(1);

    U obj = zero;
    cpp20::atomic_ref<U> ref(obj);

    ref.store(one);
    EXPECT_EQ(obj, one);

    obj = zero;
    EXPECT_EQ(ref.exchange(one), zero);
    EXPECT_EQ(obj, one);

    // Assignment
    ref = zero;
    EXPECT_EQ(obj, zero);
  }
};

template <typename T, typename = void>
struct CheckAtomicOpsCompareExchange {
  static void CheckCompareExchange() {}
};

template <typename T>
struct CheckAtomicOpsCompareExchange<T, std::enable_if_t<cpp20::atomic_internal::unqualified<T>>> {
  static void CheckCompareExchange() {
    using U = LibcxxBugWorkaround<T>;
    U zero = cast_helper<U>(0);
    U one = cast_helper<U>(1);

    std::remove_const_t<U> obj = zero;
    cpp20::atomic_ref<U> ref(obj);

    obj = zero;
    std::remove_const_t<U> exp = one;

    EXPECT_FALSE(ref.compare_exchange_weak(exp, one));
    EXPECT_EQ(obj, zero);

    obj = zero;
    exp = one;
    EXPECT_FALSE(ref.compare_exchange_strong(exp, one));
    EXPECT_EQ(obj, zero);

    obj = zero;
    exp = zero;
    // This might fail for any reason, but should eventually succeed.
    while (!ref.compare_exchange_weak(exp, one)) {
    }
    EXPECT_EQ(obj, one);

    obj = zero;
    exp = zero;
    EXPECT_TRUE(ref.compare_exchange_strong(exp, one));
    EXPECT_EQ(obj, one);
  }
};

template <typename T>
void CheckAtomicOperations() {
  using U = LibcxxBugWorkaround<T>;
  U zero = cast_helper<U>(0);
  U one = cast_helper<U>(1);

  U obj = zero;
  cpp20::atomic_ref<U> ref(obj);
  EXPECT_EQ(ref.load(), zero);

  U obj2 = one;
  cpp20::atomic_ref<U> ref2(obj2);
  EXPECT_EQ(ref2.load(), one);

  // Copy.
  cpp20::atomic_ref<U> ref_cpy(ref);
  EXPECT_EQ(ref_cpy.load(), ref.load());

  // Operator T.
  U val = ref;
  EXPECT_EQ(val, ref.load());

  // store, exchange and compare_and_exchange depends on |T|'s qualification.
  // * |const T| does not support any mutable operation.
  // * |volatile T| does not support compare_and_exchange operations.
  //
  // The following checks, will be No-Op depending on |T|.
  CheckAtomicOpsMutable<U>::CheckStoreAndExchange();
  CheckAtomicOpsCompareExchange<U>::CheckCompareExchange();
}

template <typename T>
constexpr void CheckIntegerOperations() {
  using U = LibcxxBugWorkaround<T>;
  U obj = 20;
  cpp20::atomic_ref<U> ref(obj);

  {
    U prev = ref.load();
    EXPECT_EQ(ref.fetch_add(4), prev);
    EXPECT_EQ(ref.load(), prev + 4);
    EXPECT_EQ(ref.load(), obj);

    U val = ref;
    EXPECT_EQ(val, ref.load());
  }

  {
    U prev = ref.load();
    EXPECT_EQ(ref.fetch_sub(4), prev);
    EXPECT_EQ(ref.load(), prev - 4);
    EXPECT_EQ(ref.load(), obj);
  }

  {
    U prev = ref.load();
    EXPECT_EQ((ref += 4), prev + 4);
    EXPECT_EQ(ref.load(), prev + 4);
    EXPECT_EQ(ref.load(), obj);
  }

  {
    U prev = ref.load();
    EXPECT_EQ((ref -= 4), prev - 4);
    EXPECT_EQ(ref.load(), prev - 4);
    EXPECT_EQ(ref.load(), obj);
  }

  U mask = static_cast<T>(0b01010101);

  {
    obj = -1;
    EXPECT_EQ(ref.fetch_and(mask), static_cast<T>(-1));
    EXPECT_EQ(obj, mask);

    obj = -1;
    EXPECT_EQ(ref &= mask, mask);
    EXPECT_EQ(obj, mask);

    obj = ~mask;
    EXPECT_EQ(ref.fetch_and(mask), static_cast<T>(~mask));
    EXPECT_EQ(obj, static_cast<T>(0));

    obj = ~mask;
    EXPECT_EQ(ref &= mask, static_cast<T>(0));
    EXPECT_EQ(obj, static_cast<T>(0));
  }

  {
    obj = 0;
    EXPECT_EQ(ref.fetch_or(mask), static_cast<T>(0));
    EXPECT_EQ(obj, mask);

    obj = 0;
    EXPECT_EQ(ref |= mask, mask);
    EXPECT_EQ(obj, mask);

    obj = -1;
    EXPECT_EQ(ref.fetch_or(mask), static_cast<T>(-1));
    EXPECT_EQ(obj, static_cast<T>(-1));

    obj = ~mask;
    EXPECT_EQ(ref |= mask, static_cast<T>(-1));
    EXPECT_EQ(obj, static_cast<T>(-1));
  }

  {
    obj = -1;
    EXPECT_EQ(ref.fetch_xor(mask), static_cast<T>(-1));
    EXPECT_EQ(obj, static_cast<T>(~mask));

    obj = -1;
    EXPECT_EQ(ref ^= mask, static_cast<T>(~mask));
    EXPECT_EQ(obj, static_cast<T>(~mask));

    obj = ~mask;
    EXPECT_EQ(ref.fetch_xor(mask), static_cast<T>(~mask));
    EXPECT_EQ(obj, static_cast<T>(-1));

    obj = ~mask;
    EXPECT_EQ(ref ^= mask, static_cast<T>(-1));
    EXPECT_EQ(obj, static_cast<T>(-1));
  }
}

template <typename T>
void CheckFloatSpecialization() {
  using U = LibcxxBugWorkaround<T>;
  static_assert(kHasDifferenceType<cpp20::atomic_ref<U>>, "");
  static_assert(!specialization_has_fetch_and_v<cpp20::atomic_ref<U>>, "");
  static_assert(!specialization_has_fetch_or_v<cpp20::atomic_ref<U>>, "");
  static_assert(!specialization_has_fetch_xor_v<cpp20::atomic_ref<U>>, "");
  static_assert(std::is_same_v<typename cpp20::atomic_ref<U>::value_type, U>, "");
  static_assert(std::is_same_v<typename cpp20::atomic_ref<U>::difference_type, U>, "");
  static_assert(cpp20::atomic_ref<U>::required_alignment == alignof(U), "");
}

template <typename T>
constexpr void CheckFloatOperations() {
  using U = LibcxxBugWorkaround<T>;
  U obj = 4.f;
  cpp20::atomic_ref<U> ref(obj);

  {
    U prev = ref.load();
    EXPECT_EQ(ref.fetch_add(4.f), prev);
    EXPECT_EQ(ref.load(), prev + 4.f);
    EXPECT_EQ(ref.load(), obj);
  }

  {
    U prev = ref.load();
    EXPECT_EQ(ref.fetch_sub(4.f), prev);
    EXPECT_EQ(ref.load(), prev - 4.f);
    EXPECT_EQ(ref.load(), obj);
  }

  {
    U prev = ref.load();
    EXPECT_EQ((ref += 4.f), prev + 4.f);
    EXPECT_EQ(ref.load(), prev + 4.f);
    EXPECT_EQ(ref.load(), obj);
  }

  {
    U prev = ref.load();
    EXPECT_EQ((ref -= 4.f), prev - 4.f);
    EXPECT_EQ(ref.load(), prev - 4.f);
    EXPECT_EQ(ref.load(), obj);
  }
}

template <typename T>
constexpr void CheckPointerSpecialization() {
  static_assert(kHasDifferenceType<cpp20::atomic_ref<T*>>, "");
  static_assert(!specialization_has_fetch_and_v<cpp20::atomic_ref<T*>>, "");
  static_assert(!specialization_has_fetch_or_v<cpp20::atomic_ref<T*>>, "");
  static_assert(!specialization_has_fetch_xor_v<cpp20::atomic_ref<T*>>, "");
  static_assert(std::is_same_v<typename cpp20::atomic_ref<T*>::value_type, T*>, "");
  static_assert(std::is_same_v<typename cpp20::atomic_ref<T*>::difference_type, ptrdiff_t>, "");
  static_assert(cpp20::atomic_ref<T*>::required_alignment == alignof(T*), "");
}

template <typename T>
constexpr void CheckPointerOperations() {
  T a = {};
  T* obj = &a;
  cpp20::atomic_ref<T*> ref(obj);

  {
    T* prev = ref.load();
    EXPECT_EQ(ref.fetch_add(4), prev);
    EXPECT_EQ(ref.load(), prev + 4);
    EXPECT_EQ(ref.load(), obj);
  }

  {
    T* prev = ref.load();
    EXPECT_EQ(ref.fetch_sub(4), prev);
    EXPECT_EQ(ref.load(), prev - 4);
    EXPECT_EQ(ref.load(), obj);
  }

  {
    T* prev = ref.load();
    EXPECT_EQ((ref += 4), prev + 4);
    EXPECT_EQ(ref.load(), prev + 4);
    EXPECT_EQ(ref.load(), obj);
  }

  {
    T* prev = ref.load();
    EXPECT_EQ((ref -= 4), prev - 4);
    EXPECT_EQ(ref.load(), prev - 4);
    EXPECT_EQ(ref.load(), obj);
  }
}

template <typename T>
void CheckNoSpecialization() {
  static_assert(!kHasDifferenceType<cpp20::atomic_ref<T>>, "");
  static_assert(!specialization_has_fetch_add_v<cpp20::atomic_ref<T>>, "");
  static_assert(!specialization_has_fetch_sub_v<cpp20::atomic_ref<T>>, "");
  static_assert(!specialization_has_fetch_and_v<cpp20::atomic_ref<T>>, "");
  static_assert(!specialization_has_fetch_or_v<cpp20::atomic_ref<T>>, "");
  static_assert(!specialization_has_fetch_xor_v<cpp20::atomic_ref<T>>, "");

  static_assert(cpp20::atomic_ref<T>::required_alignment >= alignof(T), "");
  static_assert(std::is_same_v<typename cpp20::atomic_ref<T>::value_type, T>, "");
}

TEST(AtomicRefTest, NoSpecialization) {
  CheckAtomicOperations<TriviallyCopyable>();
  CheckNoSpecialization<TriviallyCopyable>();

  CheckAtomicOperations<bool>();
  CheckNoSpecialization<bool>();
}

TEST(AtomicRefTest, IntegerSpecialization) {
  CheckAtomicOperations<int>();
  CheckIntegerSpecialization<int>();
  CheckIntegerOperations<int>();

  // 1 byte
  CheckAtomicOperations<char>();
  CheckIntegerSpecialization<char>();
  CheckIntegerOperations<char>();

  CheckAtomicOperations<uint8_t>();
  CheckIntegerSpecialization<uint8_t>();
  CheckIntegerOperations<uint8_t>();

  // 8 bytes
  CheckAtomicOperations<int64_t>();
  CheckIntegerSpecialization<int64_t>();
  CheckIntegerOperations<int64_t>();

  CheckAtomicOperations<uint64_t>();
  CheckIntegerSpecialization<uint64_t>();
  CheckIntegerOperations<uint64_t>();

  CheckAtomicOperations<volatile int>();
  CheckIntegerSpecialization<volatile int>();
  CheckIntegerOperations<volatile int>();

  CheckAtomicOperations<const int>();
  CheckIntegerSpecialization<const int>();

  // 16 bytes -- if supported, to silence oversized atomic operations.
  if (!cpp20::atomic_ref<int128_t>::is_always_lock_free) {
    return;
  }
  CheckAtomicOperations<int128_t*>();
  CheckIntegerSpecialization<int128_t>();
  CheckIntegerOperations<int128_t>();

  CheckAtomicOperations<uint128_t*>();
  CheckIntegerSpecialization<uint128_t>();
  CheckIntegerOperations<uint128_t>();
}

TEST(AtomicRefTest, FloatSpecialization) {
  CheckAtomicOperations<float>();
  CheckFloatSpecialization<float>();
  CheckFloatOperations<float>();

  CheckAtomicOperations<double>();
  CheckFloatSpecialization<double>();
  CheckFloatOperations<double>();

  CheckAtomicOperations<volatile float>();
  CheckFloatSpecialization<volatile float>();
  CheckFloatOperations<volatile float>();

  CheckAtomicOperations<const float>();
  CheckFloatSpecialization<const float>();
}

TEST(AtomicRefTest, PointerSpecialization) {
  CheckAtomicOperations<uint8_t*>();
  CheckPointerSpecialization<uint8_t>();
  CheckPointerOperations<uint8_t>();

  CheckAtomicOperations<int8_t*>();
  CheckPointerSpecialization<int8_t>();
  CheckPointerOperations<int8_t>();

  CheckAtomicOperations<uint64_t*>();
  CheckPointerSpecialization<uint64_t>();
  CheckPointerOperations<uint64_t>();
}

TEST(AtomicRefTest, ConstFromNonConst) {
  int a = 12345;

  // From non const T.
  cpp20::atomic_ref<LibcxxBugWorkaround<const int>> b(a);
  EXPECT_EQ(b.load(), a);
}

#if defined(__cpp_lib_atomic_ref) && __cpp_lib_atomic_ref < 201806L || \
    defined(LIB_STDCOMPAT_USE_POLYFILLS)

TEST(AtomicRefTest, InvalidFailOrderAborts) {
  ASSERT_DEATH(
      {
        int a;
        int c = 1;
        cpp20::atomic_ref<int> b(a);
        b.compare_exchange_strong(c, 2, std::memory_order_relaxed, std::memory_order_acq_rel);
      },
      ".*");

  ASSERT_DEATH(
      {
        int a;
        int c = 1;
        cpp20::atomic_ref<int> b(a);
        b.compare_exchange_strong(c, 2, std::memory_order_relaxed, std::memory_order_release);
      },
      ".*");

  ASSERT_DEATH(
      {
        int a;
        int c = 1;
        cpp20::atomic_ref<int> b(a);
        b.compare_exchange_weak(c, 2, std::memory_order_relaxed, std::memory_order_acq_rel);
      },
      ".*");

  ASSERT_DEATH(
      {
        int a;
        int c = 1;
        cpp20::atomic_ref<int> b(a);
        b.compare_exchange_weak(c, 2, std::memory_order_relaxed, std::memory_order_release);
      },
      ".*");
}

#endif

#if defined(__cpp_lib_atomic_ref) && __cpp_lib_atomic_ref >= 201806L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

TEST(AtomicRefTest, IsAliasForStd) {
  static_assert(
      std::is_same_v<cpp20::atomic_ref<TriviallyCopyable>, std::atomic_ref<TriviallyCopyable>>, "");
  static_assert(std::is_same_v<cpp20::atomic_ref<bool>, std::atomic_ref<bool>>, "");
  static_assert(std::is_same_v<cpp20::atomic_ref<int>, std::atomic_ref<int>>, "");
  static_assert(std::is_same_v<cpp20::atomic_ref<float>, std::atomic_ref<float>>, "");
  static_assert(std::is_same_v<cpp20::atomic_ref<int*>, std::atomic_ref<int*>>, "");
}

#endif

}  // namespace

namespace std {

std::ostream& operator<<(std::ostream& os, int128_t a) {
  int128_t mask = std::numeric_limits<int64_t>::min();
  os << "0x" << std::hex << static_cast<int64_t>(a >> 64) << static_cast<int64_t>(a & mask);
  return os;
}

std::ostream& operator<<(std::ostream& os, uint128_t a) {
  uint128_t mask = std::numeric_limits<uint64_t>::min();
  os << "0x" << std::hex << static_cast<uint64_t>(a >> 64) << static_cast<uint64_t>(a & mask);
  return os;
}

}  // namespace std
