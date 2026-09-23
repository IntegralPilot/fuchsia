// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/structured_backend/cpp/log_buffer.h>
#include <lib/zx/clock.h>
#include <lib/zx/socket.h>
#include <stdlib.h>

namespace fuchsia_logging {
namespace {

// Represents a byte offset that has no alignment guarantees.
class ByteOffset final {
 public:
  ByteOffset(const ByteOffset& other) = default;
  ByteOffset(ByteOffset&& other) = default;
  static ByteOffset FromBuffer(size_t offset, size_t capacity) {
    return ByteOffset(offset, capacity).AssertValid();
  }

  static ByteOffset Unbounded(size_t offset) { return ByteOffset(offset, -1); }

  size_t unsafe_get() const { return value_; }

  size_t capacity() const { return capacity_; }

  ByteOffset AssertAlignedTo(size_t size) const {
    assert((value_ % size) == 0);
    if (!((value_ % size) == 0)) {
      abort();
    }
    return *this;
  }

  ByteOffset& operator=(ByteOffset&& other) = default;

  ByteOffset& operator=(const ByteOffset& other) = default;

  ByteOffset operator+(size_t offset) const {
    return ByteOffset(value_ + offset, capacity_).AssertValid();
  }

 private:
  ByteOffset(size_t value, size_t capacity) : value_(value), capacity_(capacity) {}

  ByteOffset& AssertValid() {
    if (!((capacity_ == 0) && (value_ == 0))) {
      assert(value_ < capacity_);
      if (!(value_ < capacity_)) {
        abort();
      }
    }
    return *this;
  }

  size_t value_;
  size_t capacity_;
};

// A word offset, which guarantees that the pointer is
// aligned to alignof(T). Operations are done with respect
// to words, not bytes.
template <typename T>
class WordOffset final {
 public:
  WordOffset() = delete;
  WordOffset& operator=(const WordOffset& other) = default;
  WordOffset(const WordOffset& other) {
    capacity_ = other.capacity_;
    value_ = other.value_;
  }

  WordOffset operator+(size_t offset) const {
    return WordOffset(value_ + offset, capacity_).AssertValid();
  }

  WordOffset operator+(WordOffset offset) const {
    return WordOffset(value_ + offset.value_, capacity_).AssertValid();
  }

  WordOffset operator++(int) const { return WordOffset(value_ + 1, capacity_).AssertValid(); }

  WordOffset AddPadded(const ByteOffset& byte_offset) {
    size_t needs_padding = (byte_offset.unsafe_get() % sizeof(T)) > 0;
    // Multiply by needs_padding to set padding to 0 if no padding
    // is necessary. This avoids unnecessary branching.
    return WordOffset(value_ + (byte_offset.unsafe_get() / sizeof(T)) + needs_padding, capacity_);
  }

  WordOffset begin() { return WordOffset(0, capacity_); }

  size_t capacity() { return capacity_; }

  static WordOffset FromByteOffset(const ByteOffset& value) {
    return WordOffset(value.AssertAlignedTo(sizeof(T)).unsafe_get() / sizeof(T),
                      value.capacity() / sizeof(T));
  }

  size_t unsafe_get() const { return value_; }

  bool in_bounds(WordOffset<T> offset) { return !((offset.value_ + value_) >= capacity_); }

  ByteOffset ToByteOffset() const {
    return ByteOffset::FromBuffer(value_ * sizeof(T), capacity_ * sizeof(T));
  }

  void reset() { value_ = 0; }

  static WordOffset invalid() { return WordOffset(0, 0); }

 private:
  WordOffset(size_t value, size_t capacity) : capacity_(capacity), value_(value) {}

  WordOffset& AssertValid() {
    if (!((capacity_ == 0) && (value_ == 0))) {
      if (!(value_ < capacity_)) {
        abort();
      }
      assert(value_ < capacity_);
    }
    return *this;
  }

  size_t capacity_;
  size_t value_;
};

template <typename T>
WordOffset<T> WritePaddedInternal(T* buffer, const void* msg, const ByteOffset& length) {
  if (length.unsafe_get() == 0) {
    return WordOffset<T>::FromByteOffset(ByteOffset::Unbounded(0));
  }
  size_t needs_padding = (length.unsafe_get() % sizeof(T)) > 0;
  size_t padding = sizeof(T) - (length.unsafe_get() % sizeof(T));
  // Multiply by needs_padding to set padding to 0 if no padding
  // is necessary. This avoids unnecessary branching.
  padding *= needs_padding;
  // If we added padding -- zero the padding bytes in a single write operation
  size_t is_nonzero_length = length.unsafe_get() != 0;
  size_t eof_in_bytes = length.unsafe_get() + padding;
  size_t eof_in_words = eof_in_bytes / sizeof(T);
  size_t last_word = eof_in_words - is_nonzero_length;
  // Set the last word in the buffer to zero before writing
  // the data to it if we added padding. If we didn't add padding,
  // multiply by 1 which ends up writing back the current contents of that word
  // resulting in a NOP.
  buffer[last_word] *= !needs_padding;
  memcpy(buffer, msg, length.unsafe_get());
  return WordOffset<T>::FromByteOffset(ByteOffset::Unbounded(length.unsafe_get() + padding));
}

// Bitfield definitions copy-pasted from
// https://fuchsia.googlesource.com/fuchsia/+/c81451cd683e/sdk/lib/syslog/streams/cpp/fields.h.

template <size_t begin, size_t end>
struct Field final {
  static_assert(begin < sizeof(uint64_t) * 8, "begin is out of bounds");
  static_assert(end < sizeof(uint64_t) * 8, "end is out of bounds");
  static_assert(begin <= end, "begin must not be larger than end");
  static_assert(end - begin + 1 < 64, "must be a part of a word, not a whole word");

  static constexpr uint64_t kMask = (uint64_t(1) << (end - begin + 1)) - 1;

  template <typename T>
  static constexpr uint64_t Make(T value) {
    return static_cast<uint64_t>(value) << begin;
  }

  template <typename U>
  static constexpr U Get(uint64_t word) {
    return static_cast<U>((word >> (begin % 64)) & kMask);
  }

  static constexpr void Set(uint64_t* word, uint64_t value) {
    *word = (*word & ~(kMask << begin)) | (value << begin);
  }
};

// HeaderField structure for a Record
// see
// https://fuchsia.dev/fuchsia-src/reference/platform-spec/diagnostics/logs-encoding?hl=en#header
struct HeaderFields final {
  using Type = Field<0, 3>;
  using SizeWords = Field<4, 15>;
  using Reserved = Field<16, 55>;
  using Severity = Field<56, 63>;
};

// ArgumentField structure for an Argument
// see
// https://fuchsia.dev/fuchsia-src/reference/platform-spec/diagnostics/logs-encoding?hl=en#arguments
struct ArgumentFields {
  using Type = Field<0, 3>;
  using SizeWords = Field<4, 15>;
  using NameRefVal = Field<16, 30>;
  using NameRefMSB = Field<31, 31>;
};

struct BoolArgumentFields final : ArgumentFields {
  using Value = Field<32, 32>;
};

struct StringArgumentFields final : ArgumentFields {
  using ValueRef = Field<32, 47>;
};

struct ReservedFields final : ArgumentFields {
  using Value = Field<32, 63>;
};

using log_word_t = uint64_t;

// Represents a slice of a buffer of type T.
template <typename T>
class DataSlice final {
 public:
  DataSlice(T* ptr, WordOffset<T> slice) : ptr_(ptr), slice_(slice) {}

  T& operator[](WordOffset<T> offset) {
    offset.AssertValid();
    return ptr_[offset];
  }

  const T& operator[](WordOffset<T> offset) const { return ptr_[offset.unsafe_get()]; }

  WordOffset<T> slice() { return slice_; }

  T* data() { return ptr_; }

 private:
  T* ptr_;
  WordOffset<T> slice_;
};

static DataSlice<const char> SliceFromString(const std::string_view& string) {
  return DataSlice<const char>(
      string.data(), WordOffset<const char>::FromByteOffset(ByteOffset::Unbounded(string.size())));
}

template <typename T, size_t size>
static DataSlice<const T> SliceFromArray(const T (&array)[size]) {
  return DataSlice<const T>(array, size);
}

template <size_t size>
static DataSlice<const char> SliceFromArray(const char (&array)[size]) {
  return DataSlice<const char>(
      array, WordOffset<const char>::FromByteOffset(ByteOffset::Unbounded(size - 1)));
}

struct RecordState final {
  RecordState()
      : arg_size(WordOffset<log_word_t>::FromByteOffset(
            ByteOffset::FromBuffer(0, sizeof(internal::LogBufferData::data)))),
        current_key_size(ByteOffset::FromBuffer(0, sizeof(internal::LogBufferData::data))),
        cursor(WordOffset<log_word_t>::FromByteOffset(
            ByteOffset::FromBuffer(0, sizeof(internal::LogBufferData::data)))) {}
  // Header of the record itself
  uint64_t* header;
  FuchsiaLogSeverity raw_severity;
  // arg_size in words
  WordOffset<log_word_t> arg_size;
  // key_size in bytes
  ByteOffset current_key_size;
  // Header of the current argument being encoded
  uint64_t* current_header_position = 0;
  uint32_t dropped_count = 0;
  // Current position (in 64-bit words) into the buffer.
  WordOffset<log_word_t> cursor;
  // True if encoding was successful, false otherwise
  bool encode_success = true;
  // True if end was called
  bool ended = false;
  static RecordState* CreatePtr(internal::LogBufferData* buffer) {
    return reinterpret_cast<RecordState*>(&buffer->record_state);
  }
};
static_assert(sizeof(RecordState) <= sizeof(internal::LogBufferData::record_state),
              "Expected sizeof(RecordState) <= sizeof(LogBuffer::record_state)");
static_assert(std::alignment_of<RecordState>() == sizeof(uint64_t),
              "Expected std::alignment_of<RecordState>() == sizeof(uint64_t)");

// Used for accessing external data buffers provided by clients.
// Used by the Encoder to do in-place encoding of data
class ExternalDataBuffer final {
 public:
  explicit ExternalDataBuffer(internal::LogBufferData* buffer)
      : buffer_(&buffer->data[0]), cursor_(RecordState::CreatePtr(buffer)->cursor) {}

  ExternalDataBuffer(log_word_t* data, size_t length, WordOffset<log_word_t>& cursor)
      : buffer_(data), cursor_(cursor) {}
  __WARN_UNUSED_RESULT bool Write(const log_word_t* data, WordOffset<log_word_t> length) {
    if (!cursor_.in_bounds(length)) {
      // TODO(https://fxbug.dev/42161388): Add test for this.
      return false;
    }
    for (size_t i = 0; i < length.unsafe_get(); i++) {
      buffer_[(cursor_ + i).unsafe_get()] = data[i];
    }
    cursor_ = cursor_ + length;
    return true;
  }

  __WARN_UNUSED_RESULT bool WritePadded(const void* msg, const ByteOffset& byte_count,
                                        WordOffset<log_word_t>* written) {
    assert(written != nullptr);
    WordOffset<log_word_t> word_count = cursor_.begin().AddPadded(byte_count);
    if (!cursor_.in_bounds(word_count)) {
      // TODO(https://fxbug.dev/42161388): Add test for this.
      return false;
    }
    auto retval = WritePaddedInternal(buffer_ + cursor_.unsafe_get(), msg, byte_count);
    cursor_ = cursor_ + retval;
    *written = retval;
    return true;
  }

  template <typename T>
  __WARN_UNUSED_RESULT bool Write(const T& data) {
    static_assert(sizeof(T) >= sizeof(log_word_t), "Expected sizeof(T) >= sizeof(log_word_t)");
    static_assert(alignof(T) >= sizeof(log_word_t), "Expected alignof(T) >= sizeof(log_word_t)");
    return Write(reinterpret_cast<const log_word_t*>(&data),
                 WordOffset<log_word_t>::FromByteOffset(
                     ByteOffset::Unbounded((sizeof(T) / sizeof(log_word_t)) * sizeof(log_word_t))));
  }

  uint64_t* data() { return buffer_ + cursor_.unsafe_get(); }

  DataSlice<log_word_t> GetSlice() { return DataSlice<log_word_t>(buffer_, cursor_); }

 private:
  // Start of buffer
  log_word_t* buffer_ = nullptr;
  // Current location in buffer (in words)
  WordOffset<log_word_t>& cursor_;
};

// Encoder for structured logs
template <typename T>
class Encoder final {
 public:
  explicit Encoder(T& buffer) { buffer_ = &buffer; }

  // Begins the log record.

#if FUCHSIA_API_LEVEL_AT_LEAST(24)
  void Begin(RecordState& state, zx::basic_time<ZX_CLOCK_BOOT> timestamp,
             FuchsiaLogSeverity severity) {
#else
  void Begin(RecordState& state, zx::basic_time<ZX_CLOCK_MONOTONIC> timestamp,
             FuchsiaLogSeverity severity) {
#endif
    state.raw_severity = severity;
    state.header = buffer_->data();
    log_word_t empty_header = 0;
    state.encode_success &= buffer_->Write(empty_header);
    state.encode_success &= buffer_->Write(timestamp.get());
  }

  // Flushes a previous argument after it has been fully encoded.
  void FlushPreviousArgument(RecordState& state) { state.arg_size.reset(); }

  // Appends the key portion of an argument to the encode buffer.
  void AppendArgumentKey(RecordState& state, DataSlice<const char> key) {
    FlushPreviousArgument(state);
    auto header_position = buffer_->data();
    log_word_t empty_header = 0;
    state.encode_success &= buffer_->Write(empty_header);
    WordOffset<log_word_t> s_size =
        WordOffset<log_word_t>::FromByteOffset(ByteOffset::Unbounded(0));
    state.encode_success &= buffer_->WritePadded(key.data(), key.slice().ToByteOffset(), &s_size);
    state.arg_size = s_size + 1;  // offset by 1 for the header
    state.current_key_size = key.slice().ToByteOffset();
    state.current_header_position = header_position;
  }

  // Generates an argument header
  uint64_t ComputeArgHeader(RecordState& state, int type) {
    return ArgumentFields::Type::Make(type) |
           ArgumentFields::SizeWords::Make(state.arg_size.unsafe_get()) |
           ArgumentFields::NameRefVal::Make(state.current_key_size.unsafe_get()) |
           ArgumentFields::NameRefMSB::Make(state.current_key_size.unsafe_get() > 0 ? 1 : 0) |
           ReservedFields::Value::Make(0);
    ;
  }

  // Append a value to the current argument
  void AppendArgumentValue(RecordState& state, int64_t value) {
    // int64
    int type = 3;
    state.encode_success &= buffer_->Write(value);
    state.arg_size++;
    *state.current_header_position = ComputeArgHeader(state, type);
  }

  // Append a value to the current argument
  void AppendArgumentValue(RecordState& state, uint64_t value) {
    // uint64
    int type = 4;
    state.encode_success &= buffer_->Write(value);
    state.arg_size = state.arg_size++;
    *state.current_header_position = ComputeArgHeader(state, type);
  }

  // Append a value to the current argument
  void AppendArgumentValue(RecordState& state, double value) {
    // double
    int type = 5;
    state.encode_success &= buffer_->Write(value);
    state.arg_size = state.arg_size++;
    *state.current_header_position = ComputeArgHeader(state, type);
  }

  // Append a value to the current argument
  void AppendArgumentValue(RecordState& state, DataSlice<const char> string) {
    // string
    int type = 6;
    WordOffset<log_word_t> written =
        WordOffset<log_word_t>::FromByteOffset(ByteOffset::Unbounded(0));
    state.encode_success &=
        buffer_->WritePadded(string.data(), string.slice().ToByteOffset(), &written);
    state.arg_size = state.arg_size + written;
    uint64_t value_ref =
        string.slice().unsafe_get() > 0 ? (1 << 15) | string.slice().unsafe_get() : 0;
    *state.current_header_position =
        ComputeArgHeader(state, type) | StringArgumentFields::ValueRef::Make(value_ref);
  }

  // Append a value to the current argument
  void AppendArgumentValue(RecordState& state, bool value) {
    // bool
    int type = 9;
    *state.current_header_position = ComputeArgHeader(state, type) |
                                     BoolArgumentFields::Value::Make(static_cast<uint64_t>(value));
  }

  // Append a value to the current argument
  void End(RecordState& state) {
    // See src/lib/diagnostics/stream/rust/src/lib.rs
    constexpr auto kTracingFormatLogRecordType = 9;
    FlushPreviousArgument(state);
    uint64_t header =
        HeaderFields::Type::Make(kTracingFormatLogRecordType) |
        HeaderFields::SizeWords::Make(static_cast<size_t>(buffer_->data() - state.header)) |
        HeaderFields::Reserved::Make(0) | HeaderFields::Severity::Make(state.raw_severity);
    *state.header = header;
  }

 private:
  T* buffer_;
};

const char kMessageFieldName[] = "message";
const char kPidFieldName[] = "pid";
const char kTidFieldName[] = "tid";
const char kDroppedLogsFieldName[] = "dropped_logs";
const char kFileFieldName[] = "file";
const char kLineFieldName[] = "line";

std::string_view StripDots(std::string_view path) {
  while (strncmp(path.data(), "../", 3) == 0) {
    path = path.substr(3);
  }
  return path;
}

}  // namespace

namespace internal {

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
void SetFlushCallback(LogBuffer& buffer, internal::FlushCallback flush_callback) {
  buffer.flush_callback_ = std::move(flush_callback);
}
#endif

bool FlushToSocket(zx_handle_t socket_handle, cpp20::span<const uint8_t> data,
                   FlushConfig flush_config) {
  zx::unowned_socket socket(socket_handle);
  zx_status_t status;
  while (true) {
    status = socket->write(0, data.data(), data.size(), nullptr);
    if (status != ZX_ERR_SHOULD_WAIT || !flush_config.block_if_full) {
      break;
    }
    zx_signals_t observed;
    status = socket->wait_one(ZX_SOCKET_WRITABLE | ZX_SOCKET_PEER_CLOSED, zx::time::infinite(),
                              &observed);
    if (status != ZX_OK) {
      break;
    }
    if (observed & ZX_SOCKET_PEER_CLOSED) {
      status = ZX_ERR_PEER_CLOSED;
      break;
    }
    if (!(observed & ZX_SOCKET_WRITABLE)) {
      continue;
    }
  }

  return status != ZX_ERR_BAD_STATE && status != ZX_ERR_PEER_CLOSED;
}

}  // namespace internal

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
void LogBuffer::BeginRecord(FuchsiaLogSeverity severity, std::optional<std::string_view> file_name,
                            unsigned int line, std::optional<std::string_view> message,
                            zx::unowned_socket socket, uint32_t dropped_count, zx_koid_t pid,
                            zx_koid_t tid) {
  BeginRecord(severity, file_name, line, message, dropped_count, pid, tid);
  internal::SetFlushCallback(*this, [socket](cpp20::span<const uint8_t> data, FlushConfig config) {
    return internal::FlushToSocket(socket->get(), data, config);
  });
}
#endif

void LogBuffer::BeginRecord(FuchsiaLogSeverity severity, std::optional<std::string_view> file_name,
                            unsigned int line, std::optional<std::string_view> message,
                            uint32_t dropped_count, zx_koid_t pid, zx_koid_t tid) {
  // Initialize the encoder targeting the passed buffer, and begin the record.

#if FUCHSIA_API_LEVEL_AT_LEAST(24)
  auto time = zx::clock::get_boot();
#else
  auto time = zx::clock::get_monotonic();
#endif

  auto* state = RecordState::CreatePtr(&data_);
  RecordState& record = *state;
  // Invoke the constructor of RecordState to construct a valid RecordState
  // inside the LogBuffer.
  new (state) RecordState;
  ExternalDataBuffer external_buffer(&data_);
  Encoder<ExternalDataBuffer> encoder(external_buffer);
  encoder.Begin(*state, time, severity);
  // Initialize common PID/TID fields
  encoder.AppendArgumentKey(record, SliceFromArray(kPidFieldName));
  encoder.AppendArgumentValue(record, static_cast<uint64_t>(pid));
  encoder.AppendArgumentKey(record, SliceFromArray(kTidFieldName));
  encoder.AppendArgumentValue(record, static_cast<uint64_t>(tid));
  record.dropped_count = dropped_count;
  if (dropped_count) {
    encoder.AppendArgumentKey(record, SliceFromString(kDroppedLogsFieldName));
    encoder.AppendArgumentValue(record, static_cast<uint64_t>(dropped_count));
  }
  if (message) {
    encoder.AppendArgumentKey(record, SliceFromString(kMessageFieldName));
    encoder.AppendArgumentValue(record, SliceFromString(*message));
  }
  if (file_name) {
    encoder.AppendArgumentKey(record, SliceFromString(kFileFieldName));
    encoder.AppendArgumentValue(record, SliceFromString(StripDots(*file_name)));
  }
  encoder.AppendArgumentKey(record, SliceFromString(kLineFieldName));
  encoder.AppendArgumentValue(record, static_cast<uint64_t>(line));
}

void LogBuffer::WriteKeyValue(std::string_view key, std::string_view value) {
  auto* state = RecordState::CreatePtr(&data_);
  ExternalDataBuffer external_buffer(&data_);
  Encoder<ExternalDataBuffer> encoder(external_buffer);
  encoder.AppendArgumentKey(
      *state, DataSlice<const char>(key.data(), WordOffset<const char>::FromByteOffset(
                                                    ByteOffset::Unbounded(key.size()))));
  encoder.AppendArgumentValue(
      *state, DataSlice<const char>(value.data(), WordOffset<const char>::FromByteOffset(
                                                      ByteOffset::Unbounded(value.size()))));
}

void LogBuffer::WriteKeyValue(std::string_view key, int64_t value) {
  auto* state = RecordState::CreatePtr(&data_);
  ExternalDataBuffer external_buffer(&data_);
  Encoder<ExternalDataBuffer> encoder(external_buffer);
  encoder.AppendArgumentKey(
      *state, DataSlice<const char>(key.data(), WordOffset<const char>::FromByteOffset(
                                                    ByteOffset::Unbounded(key.size()))));
  encoder.AppendArgumentValue(*state, value);
}

void LogBuffer::WriteKeyValue(std::string_view key, uint64_t value) {
  auto* state = RecordState::CreatePtr(&data_);
  ExternalDataBuffer external_buffer(&data_);
  Encoder<ExternalDataBuffer> encoder(external_buffer);
  encoder.AppendArgumentKey(
      *state, DataSlice<const char>(key.data(), WordOffset<const char>::FromByteOffset(
                                                    ByteOffset::Unbounded(key.size()))));
  encoder.AppendArgumentValue(*state, value);
}

void LogBuffer::WriteKeyValue(std::string_view key, double value) {
  auto* state = RecordState::CreatePtr(&data_);
  ExternalDataBuffer external_buffer(&data_);
  Encoder<ExternalDataBuffer> encoder(external_buffer);
  encoder.AppendArgumentKey(
      *state, DataSlice<const char>(key.data(), WordOffset<const char>::FromByteOffset(
                                                    ByteOffset::Unbounded(key.size()))));
  encoder.AppendArgumentValue(*state, value);
}

void LogBuffer::WriteKeyValue(std::string_view key, bool value) {
  auto* state = RecordState::CreatePtr(&data_);
  ExternalDataBuffer external_buffer(&data_);
  Encoder<ExternalDataBuffer> encoder(external_buffer);
  encoder.AppendArgumentKey(
      *state, DataSlice<const char>(key.data(), WordOffset<const char>::FromByteOffset(
                                                    ByteOffset::Unbounded(key.size()))));
  encoder.AppendArgumentValue(*state, value);
}

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
bool LogBuffer::FlushRecord(FlushConfig flush_config) {
  auto span = EndRecord();
  if (span.empty() || !flush_callback_) {
    return false;
  }
  return flush_callback_(span, flush_config);
}
#endif

cpp20::span<const uint8_t> LogBuffer::EndRecord() {
  auto* state = RecordState::CreatePtr(&data_);
  ExternalDataBuffer external_buffer(&data_);
  if (!state->ended) {
    state->ended = true;
    Encoder<ExternalDataBuffer> encoder(external_buffer);
    encoder.End(*state);
  }
  if (!state->encode_success) {
    return {};
  }
  auto slice = external_buffer.GetSlice();
  return cpp20::span(reinterpret_cast<const uint8_t*>(slice.data()),
                     slice.slice().ToByteOffset().unsafe_get());
}

}  // namespace fuchsia_logging
