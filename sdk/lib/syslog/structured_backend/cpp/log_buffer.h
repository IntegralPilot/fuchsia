// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_STRUCTURED_BACKEND_CPP_LOG_BUFFER_H_
#define LIB_SYSLOG_STRUCTURED_BACKEND_CPP_LOG_BUFFER_H_

#include <lib/fit/function.h>
#include <lib/stdcompat/span.h>
#include <lib/syslog/structured_backend/fuchsia_syslog.h>
#include <zircon/availability.h>

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
#include <lib/zx/socket.h>
#else
#include <zircon/types.h>
#endif

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>

namespace fuchsia_logging {

class LogBuffer;
struct FlushConfig;

namespace internal {

// Opaque structure representing the backend encode state.
// This structure only has meaning to the backend and application code shouldn't
// touch these values.
// LogBuffers store the state of a log record that is in the process of being
// encoded.
// A LogBuffer is initialized by calling BeginRecord, and is written to
// the LogSink by calling FlushRecord.
// Calling BeginRecord on a LogBuffer will always initialize it to its
// clean state.
struct LogBufferData {
  // Record state (for keeping track of backend-specific details)
  uint64_t record_state[15];

  // Log data (used by the backend to encode the log into). The format
  // for this is backend-specific.
  uint64_t data[4096];
};

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
using FlushCallback = fit::callback<bool(cpp20::span<const uint8_t>, FlushConfig)>;
void SetFlushCallback(LogBuffer& buffer, FlushCallback flush_callback);
#endif
bool FlushToSocket(zx_handle_t socket, cpp20::span<const uint8_t> data, FlushConfig flush_config);

}  // namespace internal

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

// Configuration for flushing a log record.
struct FlushConfig {
  // If true, the call to flush will block if the socket is full.
  // If false, the call to flush will return immediately if the socket is full.
  bool block_if_full = false;
};

// Opaque structure representing the backend encode state.
// This structure only has meaning to the backend and application code shouldn't
// touch these values.
// LogBuffers store the state of a log record that is in the process of being
// encoded.
// A LogBuffer is initialized by calling BeginRecord,
// and is written to the LogSink by calling FlushRecord.
// Calling BeginRecord on a LogBuffer will always initialize it to its
// clean state.
class LogBuffer final {
 public:
  // Initializes a LogBuffer
  //
  // buffer -- The buffer to initialize
  // severity -- The severity of the log
  // file_name -- The name of the file that generated the log message
  // line -- The line number that caused this message to be generated
  // message -- The message to output.
  // the message should be interpreted as a C-style printf before being displayed to the
  // user.
  // dropped_count -- Number of dropped messages
  // pid -- The process ID that generated the message.
  // tid -- The thread ID that generated the message.
  void BeginRecord(FuchsiaLogSeverity severity, std::optional<std::string_view> file_name,
                   unsigned int line, std::optional<std::string_view> message,
                   uint32_t dropped_count, zx_koid_t pid, zx_koid_t tid);

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  void BeginRecord(FuchsiaLogSeverity severity, std::optional<std::string_view> file_name,
                   unsigned int line, std::optional<std::string_view> message,
                   zx::unowned_socket socket, uint32_t dropped_count, zx_koid_t pid, zx_koid_t tid);

  void BeginRecord(FuchsiaLogSeverity severity, std::optional<std::string_view> file_name,
                   unsigned int line, std::optional<std::string_view> message, zx_handle_t socket,
                   uint32_t dropped_count, zx_koid_t pid, zx_koid_t tid) {
    BeginRecord(severity, file_name, line, message, zx::unowned_socket(socket), dropped_count, pid,
                tid);
  }
#endif

  // Writes a key/value pair to the buffer.
  void WriteKeyValue(std::string_view key, const char* value) {
    WriteKeyValue(key, std::string_view(value, strlen(value)));
  }

  void WriteKeyValue(std::string_view key, std::string_view value);

  // Writes a key/value pair to the buffer.
  void WriteKeyValue(std::string_view key, int64_t value);

  // Writes a key/value pair to the buffer.
  void WriteKeyValue(std::string_view key, uint64_t value);

  // Writes a key/value pair to the buffer.
  void WriteKeyValue(std::string_view key, double value);

  // Writes a key/value pair to the buffer.
  void WriteKeyValue(std::string_view key, bool value);

  // Ends the record and returns a span for it. This is safe to be called multiple times.
  // If an encoding error has occurred an empty span will be returned.
  cpp20::span<const uint8_t> EndRecord();

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

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  // Writes the LogBuffer to the socket.
  bool FlushRecord(FlushConfig flush_config = {});

  // Writes the LogBuffer to the socket.
  bool Flush() { return FlushRecord(); }
#endif

 private:
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  friend void internal::SetFlushCallback(LogBuffer&, internal::FlushCallback flush_callback);
#endif

  internal::LogBufferData data_;
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  internal::FlushCallback flush_callback_;
#endif
};

}  // namespace fuchsia_logging

#endif  // LIB_SYSLOG_STRUCTURED_BACKEND_CPP_LOG_BUFFER_H_
