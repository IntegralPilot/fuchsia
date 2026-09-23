// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_USER_COPY_INCLUDE_LIB_USER_COPY_USER_PTR_H_
#define ZIRCON_KERNEL_LIB_USER_COPY_INCLUDE_LIB_USER_COPY_USER_PTR_H_

#include <lib/user_copy/internal.h>
#include <lib/zx/result.h>
#include <stddef.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <arch/user_copy.h>
#include <ktl/declval.h>
#include <ktl/type_traits.h>

// user_*_ptr<> wraps a pointer to user memory, to differentiate it from kernel
// memory. They can be in, out, or inout pointers.
//
// user_*_ptr<> ensure that types copied to/from usermode are ABI-safe (see |is_copy_allowed|).

namespace internal {

enum InOutPolicy {
  kIn = 1,
  kOut = 2,
  kInOut = kIn | kOut,
};

template <typename T>
struct MemberAccess {
  static constexpr bool kValid = false;
};

template <typename T>
  requires(ktl::is_class_v<ktl::remove_const_t<T>>)
struct MemberAccess<T> {
  using ValueType = ktl::remove_const_t<T>;

  template <auto ValueType::* Member>
  using MemberType = ktl::remove_reference_t<decltype(ktl::declval<ValueType>().*Member)>;

  template <auto ValueType::* Member>
  using AccessType = ktl::conditional_t<ktl::is_const_v<T>, ktl::add_const_t<MemberType<Member>>,
                                        MemberType<Member>>;

  template <auto ValueType::* Member>
  using ElementAccessType = ktl::remove_pointer_t<ktl::decay_t<AccessType<Member>>>;

  static constexpr bool kValid = true;
};

template <typename T, InOutPolicy Policy>
class user_ptr {
 public:
  using ValueType = ktl::remove_const_t<T>;

  static_assert(ktl::is_const<T>::value == (Policy == kIn),
                "In pointers must be const, and Out and InOut pointers must not be const.");

  static_assert(ktl::is_void_v<ValueType> || is_copy_allowed<ValueType>::value,
                "Type must be ABI-safe.");

  explicit user_ptr(T* p) : ptr_(p) {}

  // Allow copy.
  user_ptr(const user_ptr& other) = default;
  user_ptr& operator=(const user_ptr& other) = default;

  enum { is_out = ((Policy & kOut) == kOut) };

  T* get() const { return ptr_; }

  // Only a user_in_ptr<const void> or user_out_ptr<void> can be reinterpreted
  // as a different type.  Use sparingly and with great care.
  template <typename C>
    requires(ktl::is_void_v<T>)
  user_ptr<C, Policy> reinterpret() const {
    return user_ptr<C, Policy>(reinterpret_cast<C*>(ptr_));
  }

  // This requires an explicit template parameter that's a pointer-to-member
  // for a flexible array member of ValueType and yields a user_ptr to the
  // first element.  This checks that the element count (first argument)
  // matches the total size in bytes of the user buffer.  When this succeeds,
  // it should be safe to use copy_array_* on the returned user_ptr with the
  // same count.
  template <auto Member, typename U = T>
    requires(ktl::is_same_v<T, U> && MemberAccess<U>::kValid &&
             ktl::is_unbounded_array_v<typename MemberAccess<U>::template AccessType<Member>>)
  zx::result<user_ptr<typename MemberAccess<U>::template ElementAccessType<Member>, Policy>>
  flex_array(size_t count, size_t size_bytes) {
    // The argument is always just `&zx_foo_t::bar`, so ElementT itself is
    // never const even though the type of ptr_->*member will be const if T is
    // const.  So the return type must propagate const from T to ElementT,
    // which is never const by itself.
    using AccessT = MemberAccess<T>::template ElementAccessType<Member>;
    static_assert(ktl::is_same_v<AccessT[], ktl::remove_reference_t<decltype(ptr_->*Member)>>);

    // The non-varying parts of the struct should already been examined, so the
    // pointer can't be null.
    ZX_ASSERT(ptr_);

    const size_t expected_array_size_bytes = size_bytes - sizeof(ValueType);
    size_t computed_array_size_bytes;
    if (mul_overflow(sizeof(AccessT), count, &computed_array_size_bytes) ||
        computed_array_size_bytes != expected_array_size_bytes) {
      return zx::error{ZX_ERR_INVALID_ARGS};
    }

    return zx::ok(user_ptr<AccessT, Policy>(ptr_->*Member));
  }

  // special operator to return the nullness of the pointer
  explicit operator bool() const { return ptr_ != nullptr; }

  // Returns a user_ptr pointing to the |index|-th element from this one, or a null user_ptr if
  // this pointer is null. Note: This does no other validation, and the behavior is undefined on
  // overflow. (Using this will fail to compile if T is |void|.)
  user_ptr element_offset(size_t index) const {
    return ptr_ ? user_ptr(ptr_ + index) : user_ptr(nullptr);
  }

  // Returns a user_ptr offset by |offset| bytes from this one.
  user_ptr byte_offset(size_t offset) const {
    return ptr_ ? user_ptr(reinterpret_cast<T*>(reinterpret_cast<uintptr_t>(ptr_) + offset))
                : user_ptr(nullptr);
  }

  // Copies a single T to user memory. T must not be |void|.
  template <typename S>
  [[nodiscard]] zx_status_t copy_to_user(const S& src) const {
    static_assert(!ktl::is_void<S>::value, "Type cannot be void. Use .reinterpret<>().");
    static_assert(ktl::is_same<S, T>::value, "S and T must be the same type.");
    static_assert(is_copy_allowed<S>::value, "Type must be ABI-safe.");
    static_assert(Policy & kOut, "Can only copy to user for kOut or kInOut user_ptr.");
    return arch_copy_to_user(ptr_, &src, sizeof(S));
  }

  // Copies a single T to user memory. T must not be |void|. Captures permission and translation
  // faults. Access faults (on architectures that have them) will be handled transparently.
  //
  // On success ZX_OK is returned and the values in pf_va and pf_flags are undefined, otherwise they
  // are filled with fault information.
  template <typename S>
  [[nodiscard]] UserCopyCaptureFaultsResult copy_to_user_capture_faults(const S& src) const {
    static_assert(!ktl::is_void<S>::value, "Type cannot be void. Use .reinterpret<>().");
    static_assert(ktl::is_same<S, T>::value, "S and T must be the same type.");
    static_assert(is_copy_allowed<S>::value, "Type must be ABI-safe.");
    static_assert(Policy & kOut, "Can only copy to user for kOut or kInOut user_ptr.");
    return arch_copy_to_user_capture_faults(ptr_, &src, sizeof(S));
  }

  // Copies a sub-array of T to user memory. Note: This takes a count not a size. T cannot be
  // |void|.
  [[nodiscard]] zx_status_t copy_array_to_user(const T* src, size_t count, size_t offset) const {
    static_assert(!ktl::is_void<T>::value, "Type cannot be void. Use .reinterpret<>().");
    static_assert(is_copy_allowed<T>::value, "Type must be ABI-safe.");
    static_assert(Policy & kOut, "Can only copy to user for kOut or kInOut user_ptr.");
    size_t len;
    if (mul_overflow(count, sizeof(T), &len)) {
      return ZX_ERR_INVALID_ARGS;
    }
    return arch_copy_to_user(ptr_ + offset, src, len);
  }

  // Copies an array of T to user memory. Note: This takes a count not a size. T cannot be |void|.
  [[nodiscard]] zx_status_t copy_array_to_user(const T* src, size_t count) const {
    return copy_array_to_user(src, count, 0);
  }

  // Copies a sub-array array of T to user memory. Note: This takes a count not a size. T cannot be
  // |void|.
  //
  // On success ZX_OK is returned and the values in pf_va and pf_flags are undefined, otherwise they
  // are filled with fault information.
  [[nodiscard]] UserCopyCaptureFaultsResult copy_array_to_user_capture_faults(const T* src,
                                                                              size_t count,
                                                                              size_t offset) const {
    static_assert(!ktl::is_void<T>::value, "Type cannot be void. Use .reinterpret<>().");
    static_assert(is_copy_allowed<T>::value, "Type must be ABI-safe.");
    static_assert(Policy & kOut, "Can only copy to user for kOut or kInOut user_ptr.");
    size_t len;
    if (mul_overflow(count, sizeof(T), &len)) {
      return UserCopyCaptureFaultsResult{ZX_ERR_INVALID_ARGS};
    }
    return arch_copy_to_user_capture_faults(ptr_ + offset, src, len);
  }

  // Copies an array of T to user memory. Note: This takes a count not a size. T cannot be |void|.
  //
  // On success ZX_OK is returned and the values in pf_va and pf_flags are undefined, otherwise they
  // are filled with fault information.
  [[nodiscard]] UserCopyCaptureFaultsResult copy_array_to_user_capture_faults(const T* src,
                                                                              size_t count) const {
    return copy_array_to_user_capture_faults(src, count, 0);
  }

  // Copies a single T from user memory. T must not be |void|.
  [[nodiscard]] zx_status_t copy_from_user(typename ktl::remove_const<T>::type* dst) const {
    static_assert(!ktl::is_void<T>::value, "Type cannot be void. Use .reinterpret<>().");
    static_assert(is_copy_allowed<T>::value, "Type must be ABI-safe.");
    static_assert(Policy & kIn, "Can only copy from user for kIn or kInOut user_ptr.");
    return arch_copy_from_user(dst, ptr_, sizeof(T));
  }

  // Copies a single T from user memory. T must not be |void|. Captures permission and translation
  // faults. Access faults (on architectures that have them) will be handled transparently.
  //
  // On success ZX_OK is returned and the values in pf_va and pf_flags are undefined, otherwise they
  // are filled with fault information.
  [[nodiscard]] UserCopyCaptureFaultsResult copy_from_user_capture_faults(
      typename ktl::remove_const<T>::type* dst) const {
    static_assert(!ktl::is_void<T>::value, "Type cannot be void. Use .reinterpret<>().");
    static_assert(is_copy_allowed<T>::value, "Type must be ABI-safe.");
    static_assert(Policy & kIn, "Can only copy from user for kIn or kInOut user_ptr.");
    return arch_copy_from_user_capture_faults(dst, ptr_, sizeof(T));
  }

  // Copies a sub-array of T from user memory. Note: This takes a count not a size. T cannot be
  // |void|.
  [[nodiscard]] zx_status_t copy_array_from_user(typename ktl::remove_const<T>::type* dst,
                                                 size_t count, size_t offset) const {
    static_assert(!ktl::is_void<T>::value, "Type cannot be void. Use .reinterpret<>().");
    static_assert(is_copy_allowed<T>::value, "Type must be ABI-safe.");
    static_assert(Policy & kIn, "Can only copy from user for kIn or kInOut user_ptr.");
    size_t len;
    if (mul_overflow(count, sizeof(T), &len)) {
      return ZX_ERR_INVALID_ARGS;
    }
    return arch_copy_from_user(dst, ptr_ + offset, len);
  }

  // Copies an array of T from user memory. Note: This takes a count not a size. T cannot be
  // |void|.
  [[nodiscard]] zx_status_t copy_array_from_user(typename ktl::remove_const<T>::type* dst,
                                                 size_t count) const {
    return copy_array_from_user(dst, count, 0);
  }

  // Copies an array of T from user memory. Note: This takes a count not a size. T cannot be
  // |void|.
  //
  // On success ZX_OK is returned and the values in pf_va and pf_flags are undefined, otherwise they
  // are filled with fault information.
  [[nodiscard]] UserCopyCaptureFaultsResult copy_array_from_user_capture_faults(
      typename ktl::remove_const<T>::type* dst, size_t count, size_t offset) const {
    static_assert(!ktl::is_void<T>::value, "Type cannot be void. Use .reinterpret<>().");
    static_assert(is_copy_allowed<T>::value, "Type must be ABI-safe.");
    static_assert(Policy & kIn, "Can only copy from user for kIn or kInOut user_ptr.");
    size_t len;
    if (mul_overflow(count, sizeof(T), &len)) {
      return UserCopyCaptureFaultsResult{ZX_ERR_INVALID_ARGS};
    }
    return arch_copy_from_user_capture_faults(dst, ptr_ + offset, len);
  }

  [[nodiscard]] UserCopyCaptureFaultsResult copy_array_from_user_capture_faults(
      typename ktl::remove_const<T>::type* dst, size_t count) const {
    return copy_array_from_user_capture_faults(dst, count, 0);
  }

 private:
  // It is very important that this class only wrap the pointer type itself
  // and not include any other members so as not to break the ABI between
  // the kernel and user space.
  T* ptr_;
};

}  // namespace internal

template <typename T>
using user_in_ptr = internal::user_ptr<T, internal::kIn>;

template <typename T>
using user_out_ptr = internal::user_ptr<T, internal::kOut>;

template <typename T>
using user_inout_ptr = internal::user_ptr<T, internal::kInOut>;

template <typename T>
user_in_ptr<T> make_user_in_ptr(T* p) {
  return user_in_ptr<T>(p);
}

template <typename T>
user_out_ptr<T> make_user_out_ptr(T* p) {
  return user_out_ptr<T>(p);
}

template <typename T>
user_inout_ptr<T> make_user_inout_ptr(T* p) {
  return user_inout_ptr<T>(p);
}

#endif  // ZIRCON_KERNEL_LIB_USER_COPY_INCLUDE_LIB_USER_COPY_USER_PTR_H_
