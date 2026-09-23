// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_CHROMIUM_UTILS_H_
#define SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_CHROMIUM_UTILS_H_

#include <algorithm>
#include <array>
#include <concepts>
#include <cstring>
#include <deque>
#include <memory>
#include <optional>
#include <ranges>
#include <sstream>
#include <type_traits>

#include <fbl/algorithm.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/span.h>
#include <lib/syslog/cpp/macros.h>
#include <safemath/safe_math.h>
#include <zircon/compiler.h>
#include "safemath/safe_conversions.h"
#include "time_delta.h"

#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/lib/fxl/strings/string_printf.h"

#define MEDIA_EXPORT
#define MEDIA_GPU_EXPORT
#define MEDIA_PARSERS_EXPORT

#define DCHECK FX_DCHECK
#define DCHECK_GE(a, b) FX_DCHECK((a) >= (b))
#define DCHECK_GT(a, b) FX_DCHECK((a) > (b))
#define DCHECK_LT(a, b) FX_DCHECK((a) < (b))
#define DCHECK_LE(a, b) FX_DCHECK((a) <= (b))
#define DCHECK_EQ(a, b) FX_DCHECK((a) == (b))
#define DCHECK_NE(a, b) FX_DCHECK((a) != (b))

#define CHECK FX_CHECK
#define CHECK_LT(a, b) FX_CHECK((a) < (b))
#define CHECK_LE(a, b) FX_CHECK((a) <= (b))
#define CHECK_GT(a, b) FX_CHECK((a) > (b))
#define CHECK_GE(a, b) FX_CHECK((a) >= (b))
#define CHECK_EQ(a, b) FX_CHECK((a) == (b))
#define CHECK_NE(a, b) FX_CHECK((a) != (b))

#ifndef BUILDFLAG
#define BUILDFLAG(flag) (flag)
#endif

#ifndef UNSAFE_BUFFERS
#define UNSAFE_BUFFERS(...) __VA_ARGS__
#endif

#ifndef UNSAFE_TODO
#define UNSAFE_TODO(...) __VA_ARGS__
#endif

#ifndef DLOG
#define DLOG FX_DLOGS
#endif

#ifndef VLOG
#define VLOG(verbose_level) FX_LOGS(DEBUG)
#endif

#define FORCE_ALL_LOGS 0
#if !FORCE_ALL_LOGS
#define DVLOG(verbose_level) FX_DLOGS(DEBUG)
#define DVLOG_IF(verbose_level, condition) \
  FX_LAZY_STREAM(FX_LOG_STREAM(DEBUG, nullptr), (condition))
#define DVLOGF(verbosity) FX_DLOGS(DEBUG)
#else
// These force logging to be enabled:
#define DVLOG(verbosity) \
  FX_LAZY_STREAM(FX_LOG_STREAM(ERROR, ""), (verbosity) <= 4)
#define DVLOG_IF(verbose_level, condition) \
  FX_LAZY_STREAM(FX_LOG_STREAM(ERROR, ""), (condition))
#define DVLOGF(verbosity) FX_LOGS(ERROR)
#endif

#include <cstdlib>

#define VLOGF(verbosity) FX_LOGS(DEBUG)

class NotReachedLogger {
 public:
  NotReachedLogger() = default;
  [[noreturn]] ~NotReachedLogger() {
    FX_LOGS(FATAL) << "NOTREACHED hit: " << stream_.str();
    std::abort();
  }
  template <typename T>
  NotReachedLogger& operator<<(const T& val) {
    stream_ << val;
    return *this;
  }

 private:
  std::ostringstream stream_;
};

#undef NOTREACHED
#undef NOTREACHED_IN_MIGRATION
#undef NOTREACHED_NORETURN
#define NOTREACHED() NotReachedLogger()
#define NOTREACHED_IN_MIGRATION() NotReachedLogger()
#define NOTREACHED_NORETURN() NotReachedLogger()
#define NOTIMPLEMENTED FX_NOTIMPLEMENTED

#define WARN_UNUSED_RESULT __WARN_UNUSED_RESULT
#define FALLTHROUGH __FALLTHROUGH

#ifndef SEQUENCE_CHECKER
#define SEQUENCE_CHECKER(name) static_assert(true, "")
#endif
#ifndef DCHECK_CALLED_ON_VALID_SEQUENCE
#define DCHECK_CALLED_ON_VALID_SEQUENCE(name, ...)
#endif
#ifndef DETACH_FROM_SEQUENCE
#define DETACH_FROM_SEQUENCE(name)
#endif

#ifndef DISALLOW_COPY_AND_ASSIGN
#define DISALLOW_COPY_AND_ASSIGN(TypeName) \
  TypeName(const TypeName&) = delete;      \
  TypeName& operator=(const TypeName&) = delete
#endif

// The main difference between scoped_refptr and shared_ptr is that
// scoped_refptr is intrusive, so you can make a new refptr from a raw pointer.
// That isn't used much in this codebase, so ignore it.
template <typename T>
using scoped_refptr = std::shared_ptr<T>;

// Fuchsia supports C++17, so use std::optional for base::Optional.
namespace base {
template <typename T>
using Optional = std::optional<T>;
inline constexpr std::nullopt_t nullopt = std::nullopt;

template <typename T, size_t N>
constexpr size_t size(const T (&array)[N]) noexcept {
  return N;
}

template <typename T, typename... Args>
inline scoped_refptr<T> MakeRefCounted(Args&&... args) {
  return std::make_shared<T>(std::forward<Args>(args)...);
}

// base/span.h
template <typename T, size_t Extent = cpp20::dynamic_extent>
class span : public cpp20::span<T, Extent> {
 public:
  using Base = cpp20::span<T, Extent>;
  using Base::Base;
  constexpr span() : Base() {}
  template <typename Container>
  constexpr span(Container&& c) : Base(std::forward<Container>(c)) {}
  template <typename OtherT, size_t OtherExtent>
  constexpr span(const cpp20::span<OtherT, OtherExtent>& other) : Base(other) {}

  template <typename OtherSpan>
  void copy_from(const OtherSpan& other) const {
    const uint8_t* src_ptr = reinterpret_cast<const uint8_t*>(std::data(other));
    size_t src_size = std::size(other) * sizeof(*std::data(other));
    FX_CHECK(this->size_bytes() == src_size);
    std::memcpy(this->data(), src_ptr, this->size_bytes());
  }

  template <typename OtherSpan>
  void copy_prefix_from(const OtherSpan& other) const {
    const uint8_t* src_ptr = reinterpret_cast<const uint8_t*>(std::data(other));
    size_t src_size = std::size(other) * sizeof(*std::data(other));
    size_t copy_size = std::min(this->size_bytes(), src_size);
    std::memcpy(this->data(), src_ptr, copy_size);
  }

  template <size_t Count>
  constexpr span<T, Count> first() const {
    return span<T, Count>(Base::template first<Count>());
  }
  constexpr span<T, cpp20::dynamic_extent> first(size_t count) const {
    return span<T, cpp20::dynamic_extent>(Base::first(count));
  }

  constexpr span<T, cpp20::dynamic_extent> take_first(size_t count) {
    span<T, cpp20::dynamic_extent> result = first(count);
    *this = this->subspan(count);
    return result;
  }
  template <size_t Count>
  constexpr span<T, Count> take_first() {
    span<T, Count> result = first<Count>();
    *this = this->subspan(Count);
    return result;
  }

  template <size_t Count>
  constexpr span<T, Count> last() const {
    return span<T, Count>(Base::template last<Count>());
  }
  constexpr span<T, cpp20::dynamic_extent> last(size_t count) const {
    return span<T, cpp20::dynamic_extent>(Base::last(count));
  }
};

template <typename T, size_t Extent = cpp20::dynamic_extent>
using raw_span = span<T, Extent>;

template <typename T, size_t N>
span(T (&)[N]) -> span<T, N>;

template <typename T, size_t N>
span(std::array<T, N>&) -> span<T, N>;

template <typename T, size_t N>
span(const std::array<T, N>&) -> span<const T, N>;

template <typename Container>
span(Container&)
    -> span<std::remove_reference_t<std::ranges::range_reference_t<Container>>>;

template <typename T, typename Integral>
span(T*, Integral) -> span<T>;

template <typename T>
span(T*, T*) -> span<T>;

template <typename T, size_t X>
inline span<const uint8_t> as_bytes(span<T, X> s) {
  return span<const uint8_t>(reinterpret_cast<const uint8_t*>(s.data()),
                             s.size_bytes());
}

template <typename T>
inline span<T> make_span(T* ptr, size_t size) {
  return span<T>(ptr, size);
}

template <typename C>
inline auto make_span(C&& container) {
  return span(container);
}

template <typename Container>
inline span<const uint8_t> as_byte_span(const Container& arg) {
  auto s = make_span(arg);
  return span<const uint8_t>(reinterpret_cast<const uint8_t*>(s.data()),
                             s.size_bytes());
}

template <typename Container>
inline span<uint8_t> as_writable_byte_span(Container& arg) {
  auto s = make_span(arg);
  return span<uint8_t>(reinterpret_cast<uint8_t*>(s.data()), s.size_bytes());
}

template <typename T>
class HeapArray {
 public:
  HeapArray() : ptr_(nullptr), size_(0) {}
  explicit HeapArray(size_t size)
      : ptr_(size ? new T[size] : nullptr), size_(size) {}

  HeapArray(const HeapArray&) = delete;
  HeapArray& operator=(const HeapArray&) = delete;

  HeapArray(HeapArray&& other) noexcept
      : ptr_(std::move(other.ptr_)), size_(other.size_) {
    other.size_ = 0;
  }
  HeapArray& operator=(HeapArray&& other) noexcept {
    if (this != &other) {
      ptr_ = std::move(other.ptr_);
      size_ = other.size_;
      other.size_ = 0;
    }
    return *this;
  }

  static HeapArray Uninit(size_t size) { return HeapArray(size); }

  size_t size() const { return size_; }
  bool empty() const { return size_ == 0; }
  T* data() { return ptr_.get(); }
  const T* data() const { return ptr_.get(); }

  T& operator[](size_t idx) { return ptr_[idx]; }
  const T& operator[](size_t idx) const { return ptr_[idx]; }

  span<T> as_span() { return span<T>(ptr_.get(), size_); }
  span<const T> as_span() const { return span<const T>(ptr_.get(), size_); }

  span<T> first(size_t n) { return as_span().first(n); }
  span<const T> first(size_t n) const { return as_span().first(n); }

  span<T> subspan(size_t offset, size_t count = cpp20::dynamic_extent) {
    return as_span().subspan(offset, count);
  }
  span<const T> subspan(size_t offset,
                        size_t count = cpp20::dynamic_extent) const {
    return as_span().subspan(offset, count);
  }

  T* begin() { return ptr_.get(); }
  const T* begin() const { return ptr_.get(); }
  T* end() { return ptr_.get() + size_; }
  const T* end() const { return ptr_.get() + size_; }

 private:
  std::unique_ptr<T[]> ptr_;
  size_t size_;
};

// base/numerics/checked_math.h
template <typename T>
using CheckedNumeric = safemath::internal::CheckedNumeric<T>;
using safemath::checked_cast;
using safemath::IsValueInRangeForNumericType;
using safemath::saturated_cast;
using safemath::strict_cast;

// base/callback_forward.h
using OnceClosure = fit::callback<void()>;
using RepeatingClosure = fit::function<void()>;
template <typename T>
using OnceCallback = fit::callback<T>;

// base/containers/circular_deque.h
template <typename T>
using circular_deque = std::deque<T>;

// base/memory/weak_ptr.h
template <typename T>
using WeakPtr = std::weak_ptr<T>;

template <typename T>
using WeakPtrFactory = fxl::WeakPtrFactory<T>;

// base/cxx17_backports.h
using std::clamp;

// base/sys_byteorder.h
inline uint16_t NetToHost16(uint16_t x) {
  return __builtin_bswap16(x);
}
inline uint32_t NetToHost32(uint32_t x) {
  return __builtin_bswap32(x);
}
inline uint64_t NetToHost64(uint64_t x) {
  return __builtin_bswap64(x);
}

// Converts the bytes in |x| from host to network order (endianness), and
// returns the result.
inline uint16_t HostToNet16(uint16_t x) {
  return __builtin_bswap16(x);
}
inline uint32_t HostToNet32(uint32_t x) {
  return __builtin_bswap32(x);
}
inline uint64_t HostToNet64(uint64_t x) {
  return __builtin_bswap64(x);
}

namespace numerics {

template <typename SpanT>
inline uint16_t U16FromBigEndian(SpanT bytes) {
  uint16_t val = 0;
  FX_DCHECK(base::make_span(bytes).size_bytes() >= sizeof(val));
  std::memcpy(&val, std::data(bytes), sizeof(val));
  return NetToHost16(val);
}

template <typename SpanT>
inline uint32_t U32FromBigEndian(SpanT bytes) {
  uint32_t val = 0;
  FX_DCHECK(base::make_span(bytes).size_bytes() >= sizeof(val));
  std::memcpy(&val, std::data(bytes), sizeof(val));
  return NetToHost32(val);
}

template <typename SpanT>
inline uint64_t U64FromBigEndian(SpanT bytes) {
  uint64_t val = 0;
  FX_DCHECK(base::make_span(bytes).size_bytes() >= sizeof(val));
  std::memcpy(&val, std::data(bytes), sizeof(val));
  return NetToHost64(val);
}

inline std::array<uint8_t, 2> U16ToBigEndian(uint16_t val) {
  val = HostToNet16(val);
  std::array<uint8_t, 2> bytes;
  std::memcpy(bytes.data(), &val, sizeof(val));
  return bytes;
}

inline std::array<uint8_t, 4> U32ToBigEndian(uint32_t val) {
  val = HostToNet32(val);
  std::array<uint8_t, 4> bytes;
  std::memcpy(bytes.data(), &val, sizeof(val));
  return bytes;
}

inline std::array<uint8_t, 8> U64ToBigEndian(uint64_t val) {
  val = HostToNet64(val);
  std::array<uint8_t, 8> bytes;
  std::memcpy(bytes.data(), &val, sizeof(val));
  return bytes;
}

}  // namespace numerics

// base/big_endian.h
// Fuchsia is little endian
// (https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0030_fidl_is_little_endian)
// so implementing a big endian reader will always require byte swaps.
class BigEndianReader {
 public:
  BigEndianReader(const uint8_t* buf, size_t len)
      : ptr_(buf), end_(ptr_ + len) {}
  explicit BigEndianReader(base::span<const uint8_t> buf)
      : ptr_(buf.data()), end_(buf.data() + buf.size()) {}

  const uint8_t* ptr() const { return ptr_; }
  size_t remaining() const { return static_cast<size_t>(end_ - ptr_); }
  span<const uint8_t> remaining_bytes() const {
    return span<const uint8_t>(ptr_, remaining());
  }

  bool Skip(size_t len) {
    if (len > remaining())
      return false;
    ptr_ += len;
    return true;
  }

  bool ReadBytes(void* out, size_t len) {
    if (len > remaining())
      return false;
    std::memcpy(out, ptr_, len);
    ptr_ += len;
    return true;
  }

  template <typename T, size_t N>
  bool ReadBytes(T (&out)[N]) {
    return ReadBytes(out, N * sizeof(T));
  }

  template <typename Span>
  auto ReadBytes(Span&& out)
      -> decltype(ReadBytes(out.data(), out.size_bytes())) {
    return ReadBytes(out.data(), out.size_bytes());
  }

  bool ReadU8(uint8_t* value) {
    if (sizeof(uint8_t) > remaining()) {
      return false;
    }

    *value = *reinterpret_cast<const uint8_t*>(ptr_);
    ptr_ += sizeof(uint8_t);
    return true;
  }

  bool ReadU16(uint16_t* value) {
    if (sizeof(uint16_t) > remaining()) {
      return false;
    }

    // can be unaligned, so use memcpy
    uint16_t tmp;
    memcpy(&tmp, ptr_, sizeof(tmp));
    *value = __builtin_bswap16(tmp);
    ptr_ += sizeof(uint16_t);
    return true;
  }

  bool ReadU32(uint32_t* value) {
    if (sizeof(uint32_t) > remaining()) {
      return false;
    }

    // can be unaligned, so use memcpy
    uint32_t tmp;
    memcpy(&tmp, ptr_, sizeof(tmp));
    *value = __builtin_bswap32(tmp);
    ptr_ += sizeof(uint32_t);
    return true;
  }

  bool ReadU64(uint64_t* value) {
    if (sizeof(uint64_t) > remaining()) {
      return false;
    }

    // can be unaligned, so use memcpy
    uint64_t tmp;
    memcpy(&tmp, ptr_, sizeof(tmp));
    *value = __builtin_bswap64(tmp);
    ptr_ += sizeof(uint64_t);
    return true;
  }

 private:
  const uint8_t* ptr_;
  const uint8_t* end_;
};

using numerics::U16FromBigEndian;
using numerics::U16ToBigEndian;
using numerics::U32FromBigEndian;
using numerics::U32ToBigEndian;
using numerics::U64FromBigEndian;
using numerics::U64ToBigEndian;

template <typename T>
cpp20::span<uint8_t> byte_span_from_ref(T& ref) {
  return cpp20::span<uint8_t>(reinterpret_cast<uint8_t*>(&ref), sizeof(T));
}

template <typename T>
cpp20::span<const uint8_t> byte_span_from_ref(const T& ref) {
  return cpp20::span<const uint8_t>(reinterpret_cast<const uint8_t*>(&ref),
                                    sizeof(T));
}

template <typename T, size_t E>
auto as_chars(cpp20::span<T, E> s) {
  if constexpr (std::is_const_v<T>) {
    return cpp20::span<const char>(reinterpret_cast<const char*>(s.data()),
                                   s.size() * sizeof(T));
  } else {
    return cpp20::span<char>(reinterpret_cast<char*>(s.data()),
                             s.size() * sizeof(T));
  }
}

template <typename T, size_t E>
auto as_bytes(cpp20::span<T, E> s) {
  return cpp20::span<const uint8_t>(reinterpret_cast<const uint8_t*>(s.data()),
                                    s.size() * sizeof(T));
}

template <typename T, size_t E>
auto as_writable_bytes(cpp20::span<T, E> s) {
  return cpp20::span<uint8_t>(reinterpret_cast<uint8_t*>(s.data()),
                              s.size() * sizeof(T));
}

class SpanReader {
 public:
  explicit SpanReader(cpp20::span<const uint8_t> buf)
      : buf_(buf), remaining_(buf) {}
  explicit SpanReader(cpp20::span<uint8_t> buf) : buf_(buf), remaining_(buf) {}

  size_t remaining() const { return remaining_.size(); }
  size_t num_read() const { return buf_.size() - remaining_.size(); }
  cpp20::span<const uint8_t> remaining_span() const { return remaining_; }

  bool Skip(size_t n) {
    if (n > remaining_.size()) {
      return false;
    }
    remaining_ = remaining_.subspan(n);
    return true;
  }

  template <typename Range>
  bool ReadCopy(Range&& out_range) {
    auto span = cpp20::span(out_range);
    size_t bytes_needed =
        span.size() * sizeof(typename decltype(span)::element_type);
    if (bytes_needed > remaining_.size()) {
      return false;
    }
    std::memcpy(span.data(), remaining_.data(), bytes_needed);
    remaining_ = remaining_.subspan(bytes_needed);
    return true;
  }

  template <typename T>
  bool ReadU8BigEndian(T& out) {
    if (remaining_.size() < 1) {
      return false;
    }
    out = static_cast<T>(remaining_[0]);
    remaining_ = remaining_.subspan(1);
    return true;
  }

  template <typename T>
  bool ReadU16BigEndian(T& out) {
    if (remaining_.size() < 2) {
      return false;
    }
    out = static_cast<T>(numerics::U16FromBigEndian(remaining_.first(2)));
    remaining_ = remaining_.subspan(2);
    return true;
  }

  template <typename T>
  bool ReadU32BigEndian(T& out) {
    if (remaining_.size() < 4) {
      return false;
    }
    out = static_cast<T>(numerics::U32FromBigEndian(remaining_.first(4)));
    remaining_ = remaining_.subspan(4);
    return true;
  }

  template <typename T>
  bool ReadU64BigEndian(T& out) {
    if (remaining_.size() < 8) {
      return false;
    }
    out = static_cast<T>(numerics::U64FromBigEndian(remaining_.first(8)));
    remaining_ = remaining_.subspan(8);
    return true;
  }

 private:
  cpp20::span<const uint8_t> buf_;
  cpp20::span<const uint8_t> remaining_;
};

class SpanWriter {
 public:
  explicit SpanWriter(cpp20::span<uint8_t> buf) : buf_(buf), remaining_(buf) {}

  size_t remaining() const { return remaining_.size(); }
  size_t num_written() const { return buf_.size() - remaining_.size(); }
  cpp20::span<uint8_t> remaining_span() const { return remaining_; }

  bool Skip(size_t n) {
    if (n > remaining_.size()) {
      return false;
    }
    remaining_ = remaining_.subspan(n);
    return true;
  }

  template <typename Range>
  bool WriteCopy(const Range& in_range) {
    auto span = cpp20::span(in_range);
    size_t bytes_needed =
        span.size() * sizeof(typename decltype(span)::element_type);
    if (bytes_needed > remaining_.size()) {
      return false;
    }
    std::memcpy(remaining_.data(), span.data(), bytes_needed);
    remaining_ = remaining_.subspan(bytes_needed);
    return true;
  }

  bool WriteU8BigEndian(uint8_t val) {
    if (remaining_.size() < 1) {
      return false;
    }
    remaining_[0] = val;
    remaining_ = remaining_.subspan(1);
    return true;
  }

  bool WriteU16BigEndian(uint16_t val) {
    if (remaining_.size() < 2) {
      return false;
    }
    auto arr = numerics::U16ToBigEndian(val);
    std::memcpy(remaining_.data(), arr.data(), 2);
    remaining_ = remaining_.subspan(2);
    return true;
  }

  bool WriteU32BigEndian(uint32_t val) {
    if (remaining_.size() < 4) {
      return false;
    }
    auto arr = numerics::U32ToBigEndian(val);
    std::memcpy(remaining_.data(), arr.data(), 4);
    remaining_ = remaining_.subspan(4);
    return true;
  }

  bool WriteU64BigEndian(uint64_t val) {
    if (remaining_.size() < 8) {
      return false;
    }
    auto arr = numerics::U64ToBigEndian(val);
    std::memcpy(remaining_.data(), arr.data(), 8);
    remaining_ = remaining_.subspan(8);
    return true;
  }

 private:
  cpp20::span<uint8_t> buf_;
  cpp20::span<uint8_t> remaining_;
};

// base/strings/stringprintf.h
inline auto StringPrintf = fxl::StringPrintf;

// base/bits.h
namespace bits {
template <typename T>
concept UnsignedInteger = std::unsigned_integral<T> && !std::same_as<T, bool>;

template <class T,
          class U,
          class L = std::conditional_t<sizeof(T) >= sizeof(U), T, U>,
          class = std::enable_if_t<std::is_unsigned_v<T>>,
          class = std::enable_if_t<std::is_unsigned_v<U>>>
constexpr const L AlignUp(const T& val_, const U& multiple_) {
  const L val = static_cast<L>(val_);
  const L multiple = static_cast<L>(multiple_);
  return val == 0 ? 0
         : cpp20::has_single_bit<L>(multiple)
             ? (val + (multiple - 1)) & ~(multiple - 1)
             : ((val + (multiple - 1)) / multiple) * multiple;
}
template <typename T, unsigned bits = sizeof(T) * 8>
constexpr typename std::enable_if<std::is_unsigned<T>::value && sizeof(T) <= 8,
                                  unsigned>::type
CountLeadingZeroBits(T value) {
  static_assert(bits > 0, "invalid instantiation");
  return value ? bits == 64
                     ? __builtin_clzll(static_cast<uint64_t>(value))
                     : __builtin_clz(static_cast<uint32_t>(value)) - (32 - bits)
               : bits;
}

constexpr int Log2Ceiling(uint32_t n) {
  // When n == 0, we want the function to return -1.
  // When n == 0, (n - 1) will underflow to 0xFFFFFFFF, which is
  // why the statement below starts with (n ? 32 : -1).
  return (n ? 32 : -1) - CountLeadingZeroBits(n - 1);
}

template <typename T>
constexpr T LeftmostBit() {
  static_assert(std::is_unsigned_v<T>, "T must be unsigned");
  return T(1) << (sizeof(T) * 8 - 1);
}
}  // namespace bits
}  // namespace base

namespace media {

namespace limits {
enum {
  // Clients take care of their own frame requirements
  kMaxVideoFrames = 0,
};

}  // namespace limits

}  // namespace media

#endif  // SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_CHROMIUM_UTILS_H_
