// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_CPP_HOST_LOG_BUFFER_H_
#define LIB_SYSLOG_CPP_HOST_LOG_BUFFER_H_

#include <lib/syslog/cpp/log_level.h>

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>

namespace fuchsia_logging {

template <typename Key, typename Value>
class KeyValue final {
 public:
  constexpr KeyValue(Key key, Value value) : key_(key), value_(value) {}

  constexpr const Key& key() const { return key_; }
  constexpr const Value& value() const { return value_; }

 private:
  Key key_;
  Value value_;
};

// Opaque structure representing the backend encode state.
// This structure only has meaning to the backend and application code shouldn't
// touch these values.
class LogBuffer final {
 public:
  void WriteKeyValue(std::string_view key, std::string_view value);

  void WriteKeyValue(std::string_view key, int64_t value);

  void WriteKeyValue(std::string_view key, uint64_t value);

  void WriteKeyValue(std::string_view key, double value);

  void WriteKeyValue(std::string_view key, bool value);

  void WriteKeyValue(std::string_view key, const char* value) {
    WriteKeyValue(key, std::string_view(value));
  }

  // Encodes an int8
  void Encode(KeyValue<const char*, int8_t> value) {
    Encode(KeyValue<const char*, int64_t>(value.key(), value.value()));
  }

  // Encodes an int16
  void Encode(KeyValue<const char*, int16_t> value) {
    Encode(KeyValue<const char*, int64_t>(value.key(), value.value()));
  }

  // Encodes an int32
  void Encode(KeyValue<const char*, int32_t> value) {
    Encode(KeyValue<const char*, int64_t>(value.key(), value.value()));
  }

  // Encodes an int64
  void Encode(KeyValue<const char*, int64_t> value) { WriteKeyValue(value.key(), value.value()); }

#ifdef __APPLE__
  // Encodes a size_t. On Apple Clang, size_t is a special type.
  void Encode(KeyValue<const char*, size_t> value) {
    WriteKeyValue(value.key(), static_cast<int64_t>(value.value()));
  }
#endif

  // Encodes an uint8_t
  void Encode(KeyValue<const char*, uint8_t> value) {
    Encode(KeyValue<const char*, uint64_t>(value.key(), value.value()));
  }

  // Encodes an uint16_t
  void Encode(KeyValue<const char*, uint16_t> value) {
    Encode(KeyValue<const char*, uint64_t>(value.key(), value.value()));
  }

  // Encodes a uint32_t
  void Encode(KeyValue<const char*, uint32_t> value) {
    Encode(KeyValue<const char*, uint64_t>(value.key(), value.value()));
  }

  // Encodes an uint64
  void Encode(KeyValue<const char*, uint64_t> value) { WriteKeyValue(value.key(), value.value()); }

  // Encodes a NULL-terminated C-string.
  void Encode(KeyValue<const char*, const char*> value) {
    WriteKeyValue(value.key(), value.value());
  }

  // Encodes a NULL-terminated C-string.
  void Encode(KeyValue<const char*, char*> value) { WriteKeyValue(value.key(), value.value()); }

  // Encodes a C++ std::string.
  void Encode(KeyValue<const char*, std::string> value) {
    WriteKeyValue(value.key(), value.value());
  }

  // Encodes a C++ std::string_view.
  void Encode(KeyValue<const char*, std::string_view> value) {
    WriteKeyValue(value.key(), value.value());
  }

  // Encodes a double floating point value
  void Encode(KeyValue<const char*, double> value) { WriteKeyValue(value.key(), value.value()); }

  // Encodes a floating point value
  void Encode(KeyValue<const char*, float> value) { WriteKeyValue(value.key(), value.value()); }

  // Encodes a boolean value
  void Encode(KeyValue<const char*, bool> value) { WriteKeyValue(value.key(), value.value()); }

  uint64_t* data() { return data_; }

  uint64_t* record_state() { return record_state_; }

  static constexpr size_t record_state_size() { return sizeof(record_state_); }

  static constexpr size_t data_size() { return sizeof(data_); }

 private:
  // Max size of log buffer. This number may change as additional fields
  // are added to the internal encoding state. It is based on trial-and-error
  // and is adjusted when compilation fails due to it not being large enough.
  static constexpr auto kBufferSize = (1 << 15) / 8;
  // Additional storage for internal log state.
  static constexpr auto kStateSize = 18;
  // Record state (for keeping track of backend-specific details)
  uint64_t record_state_[kStateSize];
  // Log data (used by the backend to encode the log into). The format
  // for this is backend-specific.
  uint64_t data_[kBufferSize];
};

}  // namespace fuchsia_logging

#endif  // LIB_SYSLOG_CPP_HOST_LOG_BUFFER_H_
