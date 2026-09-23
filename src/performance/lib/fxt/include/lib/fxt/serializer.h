// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Given a Writer implementing the Writer interface in writer-internal.h, provide an api
// over the writer to allow serializing fxt to the Writer.
//
// Based heavily on libTrace in zircon/system/ulib/trace to allow compatibility,
// but modified to enable passing in an arbitrary buffering system.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_SERIALIZER_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_SERIALIZER_H_

#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cstdint>

#include "argument.h"
#include "fields.h"
#include "record_types.h"
#include "string_ref.h"
#include "thread_ref.h"
#include "writer_internal.h"
#include "zircon/syscalls/object.h"
#include "zircon/types.h"

namespace fxt {

inline constexpr uint64_t MakeHeader(RecordType type, WordSize size_words) {
  return RecordFields::Type::Make(ToUnderlyingType(type)) |
         RecordFields::RecordSize::Make(size_words.SizeInWords());
}

inline constexpr uint64_t MakeLargeHeader(LargeRecordType type, WordSize words) {
  return LargeRecordFields::Type::Make(15) |
         LargeRecordFields::RecordSize::Make(words.SizeInWords()) |
         LargeRecordFields::LargeType::Make(ToUnderlyingType(type));
}

// Create a Provider Info Metadata Record using a given Writer
//
// This metadata identifies a trace provider that has contributed information
// to the trace.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#format_3
template <typename Writer, internal::EnableIfWriter<Writer> = 0>
zx_status_t WriteProviderInfoMetadataRecord(Writer* writer, uint32_t provider_id, const char* name,
                                            size_t name_length) {
  const WordSize record_size = WordSize(1) /* header*/ + WordSize::FromBytes(name_length);
  const uint64_t header =
      MakeHeader(RecordType::kMetadata, record_size) |
      MetadataRecordFields::MetadataType::Make(ToUnderlyingType(MetadataType::kProviderInfo)) |
      ProviderInfoMetadataRecordFields::Id::Make(provider_id) |
      ProviderInfoMetadataRecordFields::NameLength::Make(name_length);
  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteBytes(name, name_length);
    res->Commit();
  }
  return res.status_value();
}

// Write a Profiler Module Record using the given Writer
//
// Associates a 16-bit module ID with a module's full build ID and
// name. This record is emitted once per module, allowing subsequent
// records (like Mmap records) to reference the module efficiently
// using the module ID.
//
// See also: TODO(https://fxbug.dev/444078023): Update
// https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format for the new
// profiler record.
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type>
zx_status_t WriteProfilerModuleRecord(Writer* writer, uint64_t timestamp,
                                      const ThreadRef<thread_type>& thread_ref, uint16_t module_id,
                                      const char* name, uint8_t name_length,
                                      const uint8_t* build_id, size_t build_id_length) {
  const WordSize record_size = WordSize(2) /* header, timestamp */ +
                               WordSize::FromBytes(name_length) +
                               WordSize::FromBytes(build_id_length) + thread_ref.PayloadSize();
  const uint64_t header = MakeHeader(RecordType::kProfiler, record_size) |
                          ProfilerRecordFields::ProfilerRecordType::Make(
                              ToUnderlyingType(ProfilerRecordType::kModule)) |
                          ProfilerRecordFields::ThreadRef::Make(thread_ref.HeaderEntry()) |
                          ProfilerModuleRecordFields::ModuleId::Make(module_id) |
                          ProfilerModuleRecordFields::NameLength::Make(name_length) |
                          ProfilerModuleRecordFields::BuildIdLength::Make(build_id_length);

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(timestamp);
    thread_ref.Write(*res);
    res->WriteBytes(name, name_length);
    res->WriteBytes(build_id, build_id_length);
    res->Commit();
  }
  return res.status_value();
}

// Write a Profiler Mmap Record using the given Writer
//
// Describes a specific memory mapping for a module. It links to a previously
// emitted Module Record via the module_id and specifies the mapping's
// start address, address range, module-relative address (vaddr), and
// memory permissions (flags).
//
// See also: TODO(https://fxbug.dev/444078023): Update
// https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format for the new
// profiler record.
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type>
zx_status_t WriteProfilerMmapRecord(Writer* writer, uint64_t timestamp,
                                    const ThreadRef<thread_type>& thread_ref, uint16_t module_id,
                                    uint64_t start_address, uint64_t addr_range, uint64_t vaddr,
                                    uint8_t flags) {
  const WordSize record_size =
      WordSize(5) /* header, timestamp, start_addr, addr_range, vaddr */ + thread_ref.PayloadSize();
  const uint64_t header =
      MakeHeader(RecordType::kProfiler, record_size) |
      ProfilerRecordFields::ProfilerRecordType::Make(ToUnderlyingType(ProfilerRecordType::kMmap)) |
      ProfilerRecordFields::ThreadRef::Make(thread_ref.HeaderEntry()) |
      ProfilerMmapRecordFields::ModuleId::Make(module_id) |
      ProfilerMmapRecordFields::Flags::Make(flags);

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(timestamp);
    thread_ref.Write(*res);
    res->WriteWord(start_address);
    res->WriteWord(addr_range);
    res->WriteWord(vaddr);
    res->Commit();
  }
  return res.status_value();
}

// Write a Profiler Backtrace Record using the given Writer
//
// Contains a list of backtrace frames captured at a specific moment in time.
// Batching frames into a single record simplifies downstream processing and
// sample grouping.
//
// See also: TODO(https://fxbug.dev/444078023): Update
// https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format for the new
// profiler record.
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type>
zx_status_t WriteProfilerBacktraceRecord(Writer* writer, uint64_t timestamp,
                                         const ThreadRef<thread_type>& thread_ref,
                                         const uint64_t* backtrace_data,
                                         size_t backtrace_data_length_bytes) {
  const size_t backtrace_data_length = backtrace_data_length_bytes / sizeof(uint64_t);
  const uint8_t truncated_backtrace_data_length =
      static_cast<uint8_t>(std::min<size_t>(backtrace_data_length, 255));
  const size_t truncated_backtrace_data_length_bytes =
      static_cast<size_t>(truncated_backtrace_data_length) * sizeof(uint64_t);
  const WordSize record_size = WordSize(2) /* header, timestamp */ +
                               WordSize::FromBytes(truncated_backtrace_data_length_bytes) +
                               thread_ref.PayloadSize();
  const uint64_t header =
      MakeHeader(RecordType::kProfiler, record_size) |
      ProfilerRecordFields::ProfilerRecordType::Make(
          ToUnderlyingType(ProfilerRecordType::kBacktrace)) |
      ProfilerRecordFields::ThreadRef::Make(thread_ref.HeaderEntry()) |
      ProfilerBacktraceRecordFields::BacktraceDataLength::Make(truncated_backtrace_data_length);

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(timestamp);
    thread_ref.Write(*res);
    res->WriteBytes(backtrace_data, truncated_backtrace_data_length_bytes);
    res->Commit();
  }
  return res.status_value();
}

// Create a Provider Section Metadata Record using a given Writer
//
// This metadata delimits sections of the trace that have been obtained from
// different providers. All data that follows until the next provider section
// metadata or provider info metadata is encountered is assumed to have been
// collected from the same provider.
//
// See also:
// https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#provider-section-metadata
template <typename Writer, internal::EnableIfWriter<Writer> = 0>
zx_status_t WriteProviderSectionMetadataRecord(Writer* writer, uint32_t provider_id) {
  const WordSize record_size(1);
  const uint64_t header =
      MakeHeader(RecordType::kMetadata, record_size) |
      MetadataRecordFields::MetadataType::Make(ToUnderlyingType(MetadataType::kProviderSection)) |
      ProviderSectionMetadataRecordFields::Id::Make(provider_id);
  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->Commit();
  }
  return res.status_value();
}

// Create a Provider Section Metadata Record using Writer
//
// This metadata delimits sections of the trace that have been obtained from
// different providers. All data that follows until the next provider section
// metadata or provider info metadata is encountered is assumed to have been
// collected from the same provider.
//
// See also:
// https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#provider-section-metadata
template <typename Writer, internal::EnableIfWriter<Writer> = 0>
zx_status_t WriteProviderEventMetadataRecord(Writer* writer, uint32_t provider_id,
                                             uint8_t event_id) {
  const WordSize record_size(1);
  const uint64_t header =
      MakeHeader(RecordType::kMetadata, record_size) |
      MetadataRecordFields::MetadataType::Make(ToUnderlyingType(MetadataType::kProviderEvent)) |
      ProviderEventMetadataRecordFields::Id::Make(provider_id) |
      ProviderEventMetadataRecordFields::Event::Make(event_id);
  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->Commit();
  }
  return res.status_value();
}

// Create a Magic Number Record using Writer
//
// This record serves as an indicator that the binary data is in the Fuchsia tracing format.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#magic-number-record
template <typename Writer, internal::EnableIfWriter<Writer> = 0>
zx_status_t WriteMagicNumberRecord(Writer* writer) {
  const uint64_t header = 0x0016547846040010;
  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->Commit();
  }
  return res.status_value();
}

// Write an Initialization Record using Writer
//
// An Initialization Record provides additional information which modifies how
// following records are interpreted.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#initialization-record
template <typename Writer, internal::EnableIfWriter<Writer> = 0>
zx_status_t WriteInitializationRecord(Writer* writer, zx_ticks_t ticks_per_second) {
  const WordSize record_size(2);
  const uint64_t header = MakeHeader(RecordType::kInitialization, record_size);
  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(ticks_per_second);
    res->Commit();
  }
  return res.status_value();
}

// Write String Record using Writer
//
// Registers a string in the string table
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#string-record
template <typename Writer, internal::EnableIfWriter<Writer> = 0>
zx_status_t WriteStringRecord(Writer* writer, uint16_t index, const char* string,
                              size_t string_length) {
  const WordSize record_size = WordSize(1) + WordSize::FromBytes(string_length);
  const uint64_t header = MakeHeader(RecordType::kString, record_size) |
                          StringRecordFields::StringIndex::Make(index) |
                          StringRecordFields::StringLength::Make(string_length);
  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteBytes(string, string_length);
    res->Commit();
  }
  return res.status_value();
}

// Write Thread Record using Writer
//
// Registers a thread in the thread table
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#thread-record
template <typename Writer, internal::EnableIfWriter<Writer> = 0>
zx_status_t WriteThreadRecord(Writer* writer, uint16_t index, Koid process, Koid thread) {
  const WordSize record_size(3);
  const uint64_t header =
      MakeHeader(RecordType::kThread, record_size) | ThreadRecordFields::ThreadIndex::Make(index);
  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(process.koid);
    res->WriteWord(thread.koid);
    res->Commit();
  }
  return res.status_value();
}

namespace internal {

constexpr WordSize EventContentWords(EventType eventType) {
  switch (eventType) {
    case EventType::kInstant:
    case EventType::kDurationBegin:
    case EventType::kDurationEnd:
      return WordSize(0);
    case EventType::kCounter:
    case EventType::kDurationComplete:
    case EventType::kAsyncBegin:
    case EventType::kAsyncInstant:
    case EventType::kAsyncEnd:
    case EventType::kFlowBegin:
    case EventType::kFlowStep:
    case EventType::kFlowEnd:
      return WordSize(1);
  }
  __builtin_unreachable();
}

constexpr WordSize TotalPayloadSize() { return WordSize(0); }

template <typename First, typename... Rest>
constexpr WordSize TotalPayloadSize(const First& first, const Rest&... rest) {
  return first.PayloadSize() + TotalPayloadSize(rest...);
}

// Terminal case.
template <typename Reservation>
constexpr void WriteElements(Reservation& res) {}

template <typename Reservation, typename First, typename... Rest>
constexpr void WriteElements(Reservation& res, const First& first, const Rest&... rest) {
  first.Write(res);
  WriteElements(res, rest...);
}

template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr void WriteEventRecord(typename internal::WriterTraits<Writer>::Reservation& res,
                                uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
                                const StringRef<category_type>& category_ref,
                                const StringRef<name_type>& name_ref,
                                const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  res.WriteWord(event_time);
  WriteElements(res, thread_ref, category_ref, name_ref, args...);
}

template <RefType thread_type, RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr uint64_t MakeEventHeader(
    EventType eventType, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  const WordSize content_size = EventContentWords(eventType);
  WordSize record_size = WordSize::FromBytes(sizeof(RecordHeader)) + WordSize(1) +
                         TotalPayloadSize(thread_ref, category_ref, name_ref) + content_size +
                         TotalPayloadSize(args...);
  return MakeHeader(RecordType::kEvent, record_size) |
         EventRecordFields::EventType::Make(ToUnderlyingType(eventType)) |
         EventRecordFields::ArgumentCount::Make(sizeof...(args)) |
         EventRecordFields::ThreadRef::Make(thread_ref.HeaderEntry()) |
         EventRecordFields::CategoryStringRef::Make(category_ref.HeaderEntry()) |
         EventRecordFields::NameStringRef::Make(name_ref.HeaderEntry());
}

// Write an event with no event specific data such as an Instant Event or
// Duration Begin Event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteZeroWordEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    EventType eventType, const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  const uint64_t header = MakeEventHeader(eventType, thread_ref, category_ref, name_ref, args...);
  zx::result<typename WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    WriteEventRecord<Writer>(*res, event_time, thread_ref, category_ref, name_ref, args...);
    res->Commit();
  }
  return res.status_value();
}

// Write an event with one word of event specific data such as a Counter Event
// or Async Begin Event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteOneWordEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    EventType eventType, uint64_t content,
    const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  const uint64_t header = MakeEventHeader(eventType, thread_ref, category_ref, name_ref, args...);
  zx::result<typename WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    WriteEventRecord<Writer>(*res, event_time, thread_ref, category_ref, name_ref, args...);
    res->WriteWord(content);
    res->Commit();
  }
  return res.status_value();
}

}  // namespace internal

// Write an Instant Event using the given Writer
//
// Instant Events marks a moment in time on a thread.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#instant-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteInstantEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  return internal::WriteZeroWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                            EventType::kInstant, args...);
}

// Write a Counter Event using the given Writer
//
// Counter Events sample values of each argument as data in a time series
// associated with the counter's name and id.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#instant-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteCounterEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    uint64_t counter_id, const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  return internal::WriteOneWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                           EventType::kCounter, counter_id, args...);
}

// Write a Duration Begin Event using the given Writer
//
// A Duration Begin Event marks the beginning of an operation on a particular
// thread. Must be matched by a duration end event. May be nested.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#duration-begin-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteDurationBeginEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    Argument<arg_types, arg_name_types, arg_val_types>... args) {
  return internal::WriteZeroWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                            EventType::kDurationBegin, args...);
}

// Write a Duration End Event using the given Writer
//
// A Duration End Event marks the end of an operation on a particular
// thread.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#duration-end-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteDurationEndEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    Argument<arg_types, arg_name_types, arg_val_types>... args) {
  return internal::WriteZeroWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                            EventType::kDurationEnd, args...);
}

// Write a Duration Complete Event using the given Writer
//
// A Duration Complete Event marks the beginning and end of an operation on a particular thread.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#duration-complete-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types, RefType... ref_types2>
constexpr zx_status_t WriteDurationCompleteEventRecord(
    Writer* writer, uint64_t start_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    uint64_t end_time, Argument<arg_types, arg_name_types, arg_val_types>... args) {
  return internal::WriteOneWordEventRecord(writer, start_time, thread_ref, category_ref, name_ref,
                                           EventType::kDurationComplete, end_time, args...);
}

// Write an Async Begin Event using the given Writer
//
// An Async Begin event marks the beginning of an operation that may span
// threads. Must be matched by an async end event using the same async
// correlation id.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#async-begin-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteAsyncBeginEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    uint64_t async_id, Argument<arg_types, arg_name_types, arg_val_types>... args) {
  return internal::WriteOneWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                           EventType::kAsyncBegin, async_id, args...);
}

// Write an Async Instant Event using the given Writer
//
// An Async Instant Event marks a moment within an operation that may span
// threads. Must appear between async begin event and async end event using the
// same async correlation id.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#async-instant-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteAsyncInstantEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    uint64_t async_id, Argument<arg_types, arg_name_types, arg_val_types>... args) {
  return internal::WriteOneWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                           EventType::kAsyncInstant, async_id, args...);
}

// Write an Async End Event using the given Writer
//
// An Async End event marks the end of an operation that may span
// threads.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#async-end-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteAsyncEndEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    uint64_t async_id, Argument<arg_types, arg_name_types, arg_val_types>... args) {
  return internal::WriteOneWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                           EventType::kAsyncEnd, async_id, args...);
}

// Write a Flow Begin Event to the given Writer
//
// A Flow Begin Event marks the beginning of an operation, which results in a
// sequence of actions that may span multiple threads or abstraction layers.
// Must be matched by a flow end event using the same flow correlation id. This
// can be envisioned as an arrow between duration events. The beginning of the
// flow is associated with the enclosing duration event for this thread; it
// begins where the enclosing duration event ends.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#flow-begin-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteFlowBeginEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    uint64_t flow_id, Argument<arg_types, arg_name_types, arg_val_types>... args) {
  return internal::WriteOneWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                           EventType::kFlowBegin, flow_id, args...);
}

// Write a Flow Step Event to the given Writer
//
// Marks a point within a flow. The step is associated with the enclosing
// duration event for this thread; the flow resumes where the enclosing
// duration event begins then is suspended at the point where the enclosing
// duration event event ends.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#flow-step-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteFlowStepEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    uint64_t flow_id, Argument<arg_types, arg_name_types, arg_val_types>... args) {
  return internal::WriteOneWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                           EventType::kFlowStep, flow_id, args...);
}

// Write a Flow End Event to the given Writer
//
// Marks the end of a flow. The end of the flow is associated with the
// enclosing duration event for this thread; the flow resumes where the
// enclosing duration event begins.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#flow-end-event
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, RefType category_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteFlowEndEventRecord(
    Writer* writer, uint64_t event_time, const ThreadRef<thread_type>& thread_ref,
    const StringRef<category_type>& category_ref, const StringRef<name_type>& name_ref,
    uint64_t flow_id, Argument<arg_types, arg_name_types, arg_val_types>... args) {
  return internal::WriteOneWordEventRecord(writer, event_time, thread_ref, category_ref, name_ref,
                                           EventType::kFlowEnd, flow_id, args...);
}

// Write Block Record to the given Writer
//
// Provides uninterpreted bulk data to be included in the trace. This can be
// useful for embedding captured trace data in other formats.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#blob-record
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType name_type>
constexpr zx_status_t WriteBlobRecord(Writer* writer, const StringRef<name_type>& blob_name,
                                      BlobType type, const void* bytes, size_t num_bytes) {
  const WordSize record_size =
      WordSize(1) + blob_name.PayloadSize() + WordSize::FromBytes(num_bytes);
  const uint64_t header = MakeHeader(RecordType::kBlob, record_size) |
                          BlobRecordFields::NameStringRef::Make(blob_name.HeaderEntry()) |
                          BlobRecordFields::BlobSize::Make(num_bytes) |
                          BlobRecordFields::BlobType::Make(ToUnderlyingType(type));

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    blob_name.Write(*res);
    res->WriteBytes(bytes, num_bytes);
    res->Commit();
  }
  return res.status_value();
}

// Write a Userspace Object Record to the given Writer
//
// Describes a userspace object, assigns it a label, and optionally associates
// key/value data with it as arguments. Information about the object is added
// to a per-process userspace object table.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#userspace-object-record
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type,
          RefType name_type, ArgumentType... arg_types, RefType... arg_name_types,
          RefType... arg_val_types>
constexpr zx_status_t WriteUserspaceObjectRecord(
    Writer* writer, uintptr_t pointer, const ThreadRef<thread_type>& thread_arg,
    const StringRef<name_type>& name_arg,
    const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  const WordSize record_size = WordSize(1) /*header*/ + WordSize(1) /*pointer*/ +
                               internal::TotalPayloadSize(thread_arg, name_arg, args...);
  const uint64_t header =
      MakeHeader(RecordType::kUserspaceObject, record_size) |
      UserspaceObjectRecordFields::ProcessThreadRef::Make(thread_arg.HeaderEntry()) |
      UserspaceObjectRecordFields::NameStringRef::Make(name_arg.HeaderEntry()) |
      UserspaceObjectRecordFields::ArgumentCount::Make(sizeof...(args));

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(pointer);
    internal::WriteElements(*res, thread_arg, name_arg, args...);
    res->Commit();
  }
  return res.status_value();
}

// Write a Kernel Object Record using the given Writer
//
// Describes a kernel object, assigns it a label, and optionally associates
// key/value data with it as arguments. Information about the object is added
// to a global kernel object table.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#kernel-object-record
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType name_type,
          ArgumentType... arg_types, RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteKernelObjectRecord(
    Writer* writer, Koid koid, zx_obj_type_t obj_type, const StringRef<name_type>& name_arg,
    const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  const WordSize record_size =
      WordSize(1) /*header*/ + WordSize(1) /*koid*/ + internal::TotalPayloadSize(name_arg, args...);
  const uint64_t header = MakeHeader(RecordType::kKernelObject, record_size) |
                          KernelObjectRecordFields::ObjectType::Make(obj_type) |
                          KernelObjectRecordFields::NameStringRef::Make(name_arg.HeaderEntry()) |
                          KernelObjectRecordFields::ArgumentCount::Make(sizeof...(args));

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(koid.koid);
    internal::WriteElements(*res, name_arg, args...);
    res->Commit();
  }
  return res.status_value();
}

// Write a Legacy Context Switch Record using the given Writer
//
// Describes a context switch during which a CPU handed off control from an
// outgoing thread to an incoming thread that resumes execution.
//
// This format is deprecated in favor of the more flexible context switch format.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#scheduling-record
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType outgoing_type,
          RefType incoming_type>
constexpr zx_status_t WriteContextSwitchRecord(Writer* writer, uint64_t event_time,
                                               uint8_t cpu_number,
                                               zx_thread_state_t outgoing_thread_state,
                                               const ThreadRef<outgoing_type>& outgoing_thread,
                                               const ThreadRef<incoming_type>& incoming_thread,
                                               uint8_t outgoing_thread_priority,
                                               uint8_t incoming_thread_priority) {
  const WordSize record_size = WordSize(1) /*header*/ + WordSize(1) /*timestamp*/ +
                               outgoing_thread.PayloadSize() + incoming_thread.PayloadSize();
  const uint64_t header =
      MakeHeader(RecordType::kScheduler, record_size) |
      LegacyContextSwitchRecordFields::CpuNumber::Make(cpu_number) |
      LegacyContextSwitchRecordFields::OutgoingThreadState::Make(outgoing_thread_state) |
      LegacyContextSwitchRecordFields::OutgoingThreadRef::Make(outgoing_thread.HeaderEntry()) |
      LegacyContextSwitchRecordFields::IncomingThreadRef::Make(incoming_thread.HeaderEntry()) |
      LegacyContextSwitchRecordFields::OutgoingThreadPriority::Make(outgoing_thread_priority) |
      LegacyContextSwitchRecordFields::IncomingThreadPriority::Make(incoming_thread_priority) |
      LegacyContextSwitchRecordFields::EventType::Make(
          ToUnderlyingType(SchedulerEventType::kLegacyContextSwitch));

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(event_time);
    outgoing_thread.Write(*res);
    incoming_thread.Write(*res);
    res->Commit();
  }
  return res.status_value();
}

// Write a Context Switch Record using the given Writer
//
// Describes a context switch during which a CPU handed off control from an
// outgoing thread to an incoming thread that resumes execution.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#scheduling-record
template <typename Writer, ArgumentType... arg_types, RefType... arg_name_types,
          RefType... arg_val_types, internal::EnableIfWriter<Writer> = 0>
constexpr zx_status_t WriteContextSwitchRecord(
    Writer* writer, uint64_t event_time, uint16_t cpu_number, uint32_t outgoing_thread_state,
    const ThreadRef<RefType::kInline>& outgoing_thread,
    const ThreadRef<RefType::kInline>& incoming_thread,
    const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  const WordSize record_size = WordSize(1) /*header*/ + WordSize(1) /*timestamp*/ +
                               WordSize(1) /*outgoing tid*/ + WordSize(1) /*incoming tid*/
                               + internal::TotalPayloadSize(args...);
  const uint64_t header = MakeHeader(RecordType::kScheduler, record_size) |
                          ContextSwitchRecordFields::ArgumentCount::Make(sizeof...(args)) |
                          ContextSwitchRecordFields::CpuNumber::Make(cpu_number) |
                          ContextSwitchRecordFields::ThreadState::Make(outgoing_thread_state) |
                          ContextSwitchRecordFields::EventType::Make(
                              ToUnderlyingType(SchedulerEventType::kContextSwitch));

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(event_time);
    res->WriteWord(outgoing_thread.thread().koid);
    res->WriteWord(incoming_thread.thread().koid);
    internal::WriteElements(*res, args...);
    res->Commit();
  }
  return res.status_value();
}

// Write a Thread Wakeup Record using the given Writer
//
// Describes a thread waking up on a CPU.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#scheduling-record
template <typename Writer, ArgumentType... arg_types, RefType... arg_name_types,
          RefType... arg_val_types, internal::EnableIfWriter<Writer> = 0>
constexpr zx_status_t WriteThreadWakeupRecord(
    Writer* writer, uint64_t event_time, uint16_t cpu_number,
    const ThreadRef<RefType::kInline>& incoming_thread,
    const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  const WordSize record_size = WordSize(1) /*header*/ + WordSize(1) /*timestamp*/ +
                               WordSize(1) /*incoming tid*/ + internal::TotalPayloadSize(args...);
  const uint64_t header = MakeHeader(RecordType::kScheduler, record_size) |
                          ThreadWakeupRecordFields::ArgumentCount::Make(sizeof...(args)) |
                          ThreadWakeupRecordFields::CpuNumber::Make(cpu_number) |
                          ThreadWakeupRecordFields::EventType::Make(
                              ToUnderlyingType(SchedulerEventType::kThreadWakeup));

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(event_time);
    res->WriteWord(incoming_thread.thread().koid);
    internal::WriteElements(*res, args...);
    res->Commit();
  }
  return res.status_value();
}

// Write a Log Record using the given Writer
//
// Describes a message written to the log at a particular moment in time.
//
// https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#log-record
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType thread_type>
constexpr zx_status_t WriteLogRecord(Writer* writer, uint64_t event_time,
                                     const ThreadRef<thread_type>& thread_arg,
                                     const char* log_message, size_t log_message_length) {
  const WordSize record_size = WordSize(1) /*header*/ + WordSize(1) /*timestamp*/ +
                               thread_arg.PayloadSize() + WordSize::FromBytes(log_message_length);
  const uint64_t header = MakeHeader(RecordType::kLog, record_size) |
                          LogRecordFields::LogMessageLength::Make(log_message_length) |
                          LogRecordFields::ThreadRef::Make(thread_arg.HeaderEntry());

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(event_time);
    thread_arg.Write(*res);
    res->WriteBytes(log_message, log_message_length);
    res->Commit();
  }
  return res.status_value();
}

// Write a Large BLOB Record with Metadata using the given Writer
//
// This type contains the blob data and metadata within the record itself. The
// metadata includes a timestamp, thread/process information, and arguments, in
// addition to a category and name. The name should be sufficient to identify
// the type of data contained within the blob.
//
// https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#in_band_large_blob_record_with_metadata_blob_format_0
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType category_type,
          RefType name_type, RefType thread_type, ArgumentType... arg_types,
          RefType... arg_name_types, RefType... arg_val_types>
constexpr zx_status_t WriteLargeBlobRecordWithMetadata(
    Writer* writer, uint64_t timestamp, const StringRef<category_type>& category_ref,
    const StringRef<name_type>& name_ref, const ThreadRef<thread_type>& thread_ref,
    const void* data, size_t num_bytes,
    const Argument<arg_types, arg_name_types, arg_val_types>&... args) {
  const WordSize record_size = WordSize(1) /*header*/ + WordSize(1) /*metadata word*/ +
                               WordSize(1) /*timestamp*/ +
                               internal::TotalPayloadSize(category_ref, name_ref, thread_ref) +
                               /*blob size*/ WordSize(1) + WordSize::FromBytes(num_bytes) +
                               internal::TotalPayloadSize(args...);
  const uint64_t header =
      MakeLargeHeader(LargeRecordType::kBlob, record_size) |
      LargeBlobFields::BlobFormat::Make(ToUnderlyingType(LargeBlobFormat::kMetadata));
  const uint64_t blob_header =
      BlobFormatEventFields::CategoryStringRef::Make(category_ref.HeaderEntry()) |
      BlobFormatEventFields::NameStringRef::Make(name_ref.HeaderEntry()) |
      BlobFormatEventFields::ArgumentCount::Make(sizeof...(args)) |
      BlobFormatEventFields::ThreadRef::Make(thread_ref.HeaderEntry());

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(blob_header);
    category_ref.Write(*res);
    name_ref.Write(*res);
    res->WriteWord(timestamp);
    internal::WriteElements(*res, thread_ref, args...);
    res->WriteWord(num_bytes);
    res->WriteBytes(data, num_bytes);
    res->Commit();
  }
  return res.status_value();
}

// Write a Large BLOB Record without Metadata using the given Writer
//
// This type contains the blob data within the record itself, but does not
// include metadata. The record only contains a category and name.
// The name should be sufficient to identify the type of data contained within the blob.
//
// https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#in_band_large_blob_record_no_metadata_blob_format_1
template <typename Writer, internal::EnableIfWriter<Writer> = 0, RefType category_type,
          RefType name_type>
constexpr zx_status_t WriteLargeBlobRecordWithNoMetadata(
    Writer* writer, const StringRef<category_type>& category_ref,
    const StringRef<name_type>& name_ref, const void* data, size_t num_bytes) {
  const WordSize record_size = /*header*/ WordSize(1) + /*blob header*/ WordSize(1) +
                               internal::TotalPayloadSize(category_ref, name_ref) +
                               /*blob size*/ WordSize(1) + WordSize::FromBytes(num_bytes);

  const uint64_t header =
      MakeLargeHeader(LargeRecordType::kBlob, record_size) |
      LargeBlobFields::BlobFormat::Make(ToUnderlyingType(LargeBlobFormat::kNoMetadata));
  const uint64_t blob_header =
      BlobFormatAttachmentFields::CategoryStringRef::Make(category_ref.HeaderEntry()) |
      BlobFormatAttachmentFields::NameStringRef::Make(name_ref.HeaderEntry());

  zx::result<typename internal::WriterTraits<Writer>::Reservation> res = writer->Reserve(header);
  if (res.is_ok()) {
    res->WriteWord(blob_header);
    category_ref.Write(*res);
    name_ref.Write(*res);
    res->WriteWord(num_bytes);
    res->WriteBytes(data, num_bytes);
    res->Commit();
  }
  return res.status_value();
}

}  // namespace fxt

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_SERIALIZER_H_
