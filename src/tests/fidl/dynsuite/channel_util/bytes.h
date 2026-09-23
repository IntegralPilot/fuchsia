// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file defines a GIDL-like DSL in C++ to help with defining FIDL bytes.

#ifndef SRC_TESTS_FIDL_DYNSUITE_CHANNEL_UTIL_BYTES_H_
#define SRC_TESTS_FIDL_DYNSUITE_CHANNEL_UTIL_BYTES_H_

#include <lib/fidl/cpp/natural_types.h>
#include <lib/fidl/cpp/transaction_header.h>
#include <zircon/assert.h>
#include <zircon/fidl.h>
#include <zircon/types.h>

#include <utility>
#include <vector>

namespace channel_util {

// Bytes is a wrapper around std::vector<uint8_t> that provides list
// initialization which automatically flattens nested Bytes values.
class Bytes {
 public:
  explicit Bytes(std::vector<uint8_t> data) : data_(std::move(data)) {}
  Bytes(std::initializer_list<uint8_t> data) : data_(data) {}
  Bytes(std::initializer_list<Bytes> bytes) {
    for (auto& b : bytes) {
      data_.insert(data_.end(), b.data_.begin(), b.data_.end());
    }
  }

  size_t size() const { return data_.size(); }
  const uint8_t* data() const { return data_.data(); }

  bool operator==(const Bytes& rhs) const { return data_ == rhs.data_; }
  bool operator!=(const Bytes& rhs) const { return data_ != rhs.data_; }

  // Returns the first four bytes interpreted as a txid.
  zx_txid_t& txid() {
    ZX_ASSERT(data_.size() >= sizeof(zx_txid_t));
    return *reinterpret_cast<zx_txid_t*>(data_.data());
  }

 private:
  std::vector<uint8_t> data_;
};

template <typename T>
inline Bytes as_bytes(const T& value) {
  static_assert(std::has_unique_object_representations_v<T>,
                "objects with internal padding disallowed as the padding bytes are undefined");
  std::vector<uint8_t> vec(sizeof(T), 0);
  memcpy(vec.data(), &value, sizeof(T));
  return Bytes(std::move(vec));
}

// Like fidl_message_header_t but with convenient defaults and implicit conversion to Bytes.
struct Header {
  zx_txid_t txid;
  uint8_t at_rest_flags[2] = {0x02, 0x00};  // v2 wire format
  uint8_t dynamic_flags = 0x00;
  uint8_t magic_number = 1;  // FIDL 2023 magic number
  uint64_t ordinal;

  // NOLINTNEXTLINE(google-explicit-constructor)
  operator Bytes() const { return as_bytes(*this); }
};

// We don't use these values directly, to keep tests independent of what they're
// testing. But we expect them to be the same, so statically assert that.
static_assert(sizeof(Header) == sizeof(fidl_message_header_t));
static_assert(Header{}.magic_number == kFidlWireFormatMagicNumberInitial);
static_assert(Header{}.at_rest_flags[0] == FIDL_MESSAGE_HEADER_AT_REST_FLAGS_0_USE_VERSION_V2);
static_assert(Header{}.at_rest_flags[1] == 0);

// A placeholder to indicate that the txid is unknown when using
// Channel::read_and_check_unknown_txid.
const zx_txid_t kTxidNotKnown = 0;

// An arbitrary nonzero txid to use for two-way methods in tests.
const zx_txid_t kTwoWayTxid = 0x174d3b6a;

// The dynamic flag indicating a method is flexible, not strict.
const uint8_t kDynamicFlagsFlexible = 0x80;

// An invalid magic number.
const uint8_t kBadMagicNumber = 0x5a;

inline Bytes uint8(uint8_t value) { return as_bytes(value); }
inline Bytes uint16(uint16_t value) { return as_bytes(value); }
inline Bytes uint32(uint32_t value) { return as_bytes(value); }
inline Bytes uint64(uint64_t value) { return as_bytes(value); }
inline Bytes int8(int8_t value) { return as_bytes(value); }
inline Bytes int16(int16_t value) { return as_bytes(value); }
inline Bytes int32(int32_t value) { return as_bytes(value); }
inline Bytes int64(int64_t value) { return as_bytes(value); }

struct RepeatOp {
  uint8_t byte;
  Bytes times(size_t count) const { return Bytes(std::vector<uint8_t>(count, byte)); }
};

inline RepeatOp repeat(uint8_t byte) { return RepeatOp{byte}; }

inline Bytes padding(size_t count) { return repeat(0).times(count); }

template <typename FidlType>
inline Bytes encode(FidlType message) {
  auto result = fidl::StandaloneEncode(message);
  ZX_ASSERT_MSG(result.message().ok(), "encode failed");
  ZX_ASSERT_MSG(result.message().handle_actual() == 0u, "message contained handles");
  auto copied_bytes = result.message().CopyBytes();
  std::vector<uint8_t> vector(copied_bytes.data(), copied_bytes.data() + copied_bytes.size());
  return Bytes(vector);
}

inline Bytes handle_present() { return repeat(0xff).times(4); }
inline Bytes handle_absent() { return repeat(0x00).times(4); }

inline Bytes pointer_present() { return repeat(0xff).times(8); }
inline Bytes pointer_absent() { return repeat(0x00).times(8); }

inline Bytes union_ordinal(fidl_xunion_tag_t ordinal) { return uint64(ordinal); }
inline Bytes table_max_ordinal(uint64_t ordinal) { return uint64(ordinal); }

const fidl_xunion_tag_t kResultUnionSuccess = 1;
const fidl_xunion_tag_t kResultUnionDomainError = 2;
const fidl_xunion_tag_t kResultUnionFrameworkError = 3;

inline Bytes string_length(uint64_t length) { return uint64(length); }
inline Bytes vector_length(uint64_t length) { return uint64(length); }

inline Bytes framework_err_unknown_method() { return int32(ZX_ERR_NOT_SUPPORTED); }

inline Bytes string_header(uint64_t length) { return {string_length(length), pointer_present()}; }
inline Bytes vector_header(uint64_t length) { return {vector_length(length), pointer_present()}; }

inline Bytes out_of_line_envelope(uint32_t num_bytes, uint8_t num_handles = 0) {
  return {uint32(num_bytes), uint16(num_handles), uint16(0)};
}

inline Bytes inline_envelope(const Bytes& value, bool has_handles = false) {
  ZX_ASSERT_MSG(value.size() <= 4, "inline envelope values must be <= 4 bytes");
  return {value, padding(4 - value.size()), uint16(has_handles), uint16(1)};
}

}  // namespace channel_util

#endif  // SRC_TESTS_FIDL_DYNSUITE_CHANNEL_UTIL_BYTES_H_
