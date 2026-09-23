// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_BUFFER_H_
#define SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_BUFFER_H_

#include <concepts>
#include <cstddef>
#include <memory>
#include <span>
#include <string_view>
#include <type_traits>

namespace zxdump {

// Forward declaration.
class Process;

namespace internal {

// Forward declaration.
class DumpFile;

// This is a private class used in the implementation of the Buffer API, below.
class BufferImpl {
 public:
  virtual ~BufferImpl() = 0;
};

}  // namespace internal

// zxdump::Buffer<T> provides a view over a chunk of memory returned by
// zxdump::Process::read_memory.
//
// This is a move-only object that "owns" the storage viewed, but it's also
// tied to the lifetime of the zxdump::TaskHolder object that owns the
// zxdump::Process object.  The data pointers from this object cannot be used
// after either the zxdump::Buffer object or the associated zxdump::TaskHolder
// object has been destroyed.
//
// zxdump::Buffer<T> acts like a smart-pointer type to std::span<const T> (or
// a similar type) in that it has get() and the * and -> operators to access
// that object's standard methods.  Unlike other smart-pointer types, a
// zxdump::Buffer has no nullptr-like state (except when default-constructed)
// and is not contextually convertible to bool.  A default-constructed
// zxdump::Buffer can be assigned to, but not otherwise used.
//
// All the memory-reading operations that return a zxdump::Buffer return a
// result type that never reflects success with a default-constructed
// zxdump::Buffer object.  However, they can return an object whose value
// (View) is in its empty, default-constructed state.  This indicates that the
// process memory was valid to access, but was elided (wholly or partially)
// from the dump.  In this case, reading a shorter region might succeed if the
// containing segment was truncated rather than elided entirely.
//
// Note that all memory will appear to have been elided if the dump was read in
// by a zxdump::TaskHolder::Insert with read_memory=false.

template <typename T = std::byte, class View = std::span<const T>>
class Buffer {
 public:
  Buffer() = default;

  Buffer(const Buffer&) = delete;

  Buffer(Buffer&& other) noexcept
      : data_(std::exchange(other.data_, {})), impl_(std::move(other.impl_)) {}

  // Converting move construction is allowed if the pointers are convertible.
  template <typename OtherT, class OtherView>
    requires std::convertible_to<const OtherT*, const T*>
  explicit(false) Buffer(Buffer<OtherT, OtherView>&& other) noexcept
      : data_{static_cast<const T*>(other->data()), other->size_bytes() / sizeof(T)},
        impl_{std::move(other.impl_)} {
    other.data_ = {};
  }

  Buffer& operator=(Buffer&& other) noexcept {
    std::swap(data_, other.data_);
    std::swap(impl_, other.impl_);
    return *this;
  }

  template <typename OtherT, class OtherView>
    requires std::convertible_to<const OtherT*, const T*>
  Buffer& operator=(Buffer<OtherT, OtherView>&& other) noexcept {
    *this = Buffer(std::move(other));
    return *this;
  }

  // This is like a reinterpret_cast, but applied to the rvalue reference.
  template <typename OtherT, class OtherView>
  Buffer<OtherT, OtherView> Reinterpret() && {
    Buffer<OtherT, OtherView> other;
    other.data_ = {
        reinterpret_cast<const OtherT*>(data_.data()),
        data_.size_bytes() / sizeof(OtherT),
    };
    data_ = {};
    other.impl_ = std::move(impl_);
    return other;
  }

  ~Buffer() = default;

  const View& get() const { return data_; }

  const View& operator*() const { return data_; }

  const View* operator->() const { return &data_; }

 private:
  template <typename OtherT, class OtherView>
  friend class Buffer;
  friend Process;
  friend internal::DumpFile;

  View data_;
  std::unique_ptr<internal::BufferImpl> impl_;
};

// This is just zxdump::Buffer using std::string_view and cousins instead of
// std::span to present the data.
template <typename T = char>
using StringBuffer = Buffer<T, std::basic_string_view<T>>;

}  // namespace zxdump

#endif  // SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_BUFFER_H_
