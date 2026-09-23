// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Given a Writer implementing the Writer interface in writer-internal.h, provide an api
// over the writer to allow serializing fxt to the Writer.
//
// Based heavily on libTrace in zircon/system/ulib/trace to allow compatibility,
// but modified to enable passing in an arbitrary buffering system.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_THREAD_REF_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_THREAD_REF_H_

#include <zircon/types.h>

#include <type_traits>

#include "record_types.h"

namespace fxt {

// Represents an FXT Thread Reference which is either inline in the record
// body, or an index included in the record header
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#thread-references
template <RefType>
class ThreadRef;

template <typename T, RefType ref_type>
using EnableIfConvertibleToThreadRef =
    std::enable_if_t<(std::is_convertible_v<T, ThreadRef<ref_type>> &&
                      !std::is_same_v<T, ThreadRef<ref_type>>),
                     bool>;
template <>
class ThreadRef<RefType::kInline> {
  enum Convert { kConvert };

 public:
  template <typename T, EnableIfConvertibleToThreadRef<T, RefType::kInline> = true>
  constexpr ThreadRef(const T& value) : ThreadRef{kConvert, value} {}

  constexpr ThreadRef(zx_koid_t process, zx_koid_t thread)
      : process_(Koid(process)), thread_(Koid(thread)) {}

  constexpr ThreadRef(Koid process, Koid thread) : process_(process), thread_(thread) {}

  constexpr ThreadRef(const ThreadRef&) = default;
  constexpr ThreadRef& operator=(const ThreadRef&) = default;

  constexpr WordSize PayloadSize() const { return WordSize(2); }
  constexpr uint64_t HeaderEntry() const { return 0; }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteWord(process_.koid);
    res.WriteWord(thread_.koid);
  }

  constexpr Koid process() const { return process_; }
  constexpr Koid thread() const { return thread_; }

 private:
  constexpr ThreadRef(Convert, const ThreadRef& value)
      : process_{value.process_}, thread_{value.thread_} {}

  Koid process_;
  Koid thread_;
};

ThreadRef(zx_koid_t, zx_koid_t) -> ThreadRef<RefType::kInline>;

ThreadRef(Koid, Koid) -> ThreadRef<RefType::kInline>;

template <typename T, EnableIfConvertibleToThreadRef<T, RefType::kInline> = true>
ThreadRef(const T&) -> ThreadRef<RefType::kInline>;

template <>
class ThreadRef<RefType::kId> {
  enum Convert { kConvert };

 public:
  template <typename T, EnableIfConvertibleToThreadRef<T, RefType::kId> = true>
  constexpr ThreadRef(const T& value) : ThreadRef{kConvert, value} {}

  explicit constexpr ThreadRef(uint8_t id) : id_(id) {}

  constexpr ThreadRef(const ThreadRef&) = default;
  constexpr ThreadRef& operator=(const ThreadRef&) = default;

  constexpr WordSize PayloadSize() const { return WordSize(0); }
  constexpr uint64_t HeaderEntry() const { return id_; }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {}

  constexpr uint8_t id() const { return id_; }

 private:
  constexpr ThreadRef(Convert, const ThreadRef& value) : id_{value.id_} {}

  uint8_t id_;
};
ThreadRef(uint8_t) -> ThreadRef<RefType::kId>;

template <typename T, EnableIfConvertibleToThreadRef<T, RefType::kId> = true>
ThreadRef(const T&) -> ThreadRef<RefType::kId>;

}  // namespace fxt

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_THREAD_REF_H_
