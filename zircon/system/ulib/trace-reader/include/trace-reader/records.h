// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TRACE_READER_RECORDS_H_
#define TRACE_READER_RECORDS_H_

#include <lib/trace-engine/types.h>
#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <cstdint>
#include <optional>
#include <span>
#include <string>
#include <type_traits>
#include <utility>
#include <variant>
#include <vector>

namespace trace {

// Holds a process koid and thread koid as a pair.
// Sorts by process koid then by thread koid.
class ProcessThread final {
 public:
  constexpr ProcessThread() : process_koid_(ZX_KOID_INVALID), thread_koid_(ZX_KOID_INVALID) {}
  constexpr explicit ProcessThread(zx_koid_t process_koid, zx_koid_t thread_koid)
      : process_koid_(process_koid), thread_koid_(thread_koid) {}
  constexpr ProcessThread(const ProcessThread& other)
      : process_koid_(other.process_koid_), thread_koid_(other.thread_koid_) {}

  constexpr explicit operator bool() const { return thread_koid_ != 0u || process_koid_ != 0u; }

  constexpr bool operator==(const ProcessThread& other) const {
    return process_koid_ == other.process_koid_ && thread_koid_ == other.thread_koid_;
  }

  constexpr bool operator!=(const ProcessThread& other) const { return !(*this == other); }

  constexpr bool operator<(const ProcessThread& other) const {
    if (process_koid_ != other.process_koid_) {
      return process_koid_ < other.process_koid_;
    }
    return thread_koid_ < other.thread_koid_;
  }

  constexpr zx_koid_t process_koid() const { return process_koid_; }
  constexpr zx_koid_t thread_koid() const { return thread_koid_; }

  ProcessThread& operator=(const ProcessThread& other) {
    process_koid_ = other.process_koid_;
    thread_koid_ = other.thread_koid_;
    return *this;
  }

  std::string ToString() const;

 private:
  zx_koid_t process_koid_;
  zx_koid_t thread_koid_;
};

// A typed argument value.
class ArgumentValue final {
 public:
  static ArgumentValue MakeNull() { return ArgumentValue(); }
  static ArgumentValue MakeBool(bool value) { return ArgumentValue(Bool{value}); }
  static ArgumentValue MakeInt32(int32_t value) { return ArgumentValue(value); }
  static ArgumentValue MakeUint32(uint32_t value) { return ArgumentValue(value); }
  static ArgumentValue MakeInt64(int64_t value) { return ArgumentValue(value); }
  static ArgumentValue MakeUint64(uint64_t value) { return ArgumentValue(value); }
  static ArgumentValue MakeDouble(double value) { return ArgumentValue(value); }
  static ArgumentValue MakeString(std::string value) { return ArgumentValue(std::move(value)); }
  static ArgumentValue MakePointer(uint64_t value) { return ArgumentValue(Pointer{value}); }
  static ArgumentValue MakeKoid(zx_koid_t value) { return ArgumentValue(Koid{value}); }
  static ArgumentValue MakeBlob(std::span<const uint8_t> value) {
    return ArgumentValue(std::vector<uint8_t>(value.begin(), value.end()));
  }

  ~ArgumentValue() = default;

  ArgumentValue(const ArgumentValue&) = default;
  ArgumentValue& operator=(const ArgumentValue&) = default;

  ArgumentValue(ArgumentValue&& other) noexcept { *this = std::move(other); }
  ArgumentValue& operator=(ArgumentValue&& other) noexcept {
    if (this != &other) {
      value_ = std::move(other.value_);
      other.value_.emplace<Null>();
    }
    return *this;
  }

  ArgumentType type() const {
    return std::visit(
        [](auto&& arg) -> ArgumentType {
          using T = std::decay_t<decltype(arg)>;
          if constexpr (std::is_same_v<T, Null>) {
            return ArgumentType::kNull;
          } else if constexpr (std::is_same_v<T, Bool>) {
            return ArgumentType::kBool;
          } else if constexpr (std::is_same_v<T, int32_t>) {
            return ArgumentType::kInt32;
          } else if constexpr (std::is_same_v<T, uint32_t>) {
            return ArgumentType::kUint32;
          } else if constexpr (std::is_same_v<T, int64_t>) {
            return ArgumentType::kInt64;
          } else if constexpr (std::is_same_v<T, uint64_t>) {
            return ArgumentType::kUint64;
          } else if constexpr (std::is_same_v<T, double>) {
            return ArgumentType::kDouble;
          } else if constexpr (std::is_same_v<T, std::string>) {
            return ArgumentType::kString;
          } else if constexpr (std::is_same_v<T, Pointer>) {
            return ArgumentType::kPointer;
          } else if constexpr (std::is_same_v<T, Koid>) {
            return ArgumentType::kKoid;
          } else if constexpr (std::is_same_v<T, std::vector<uint8_t>>) {
            return ArgumentType::kBlob;
          }
        },
        value_);
  }

  uint32_t GetBool() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kBool);
    return std::get<Bool>(value_).value;
  }
  int32_t GetInt32() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kInt32);
    return std::get<int32_t>(value_);
  }
  uint32_t GetUint32() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kUint32);
    return std::get<uint32_t>(value_);
  }
  int64_t GetInt64() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kInt64);
    return std::get<int64_t>(value_);
  }
  uint64_t GetUint64() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kUint64);
    return std::get<uint64_t>(value_);
  }
  double GetDouble() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kDouble);
    return std::get<double>(value_);
  }
  const std::string& GetString() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kString);
    return std::get<std::string>(value_);
  }
  uint64_t GetPointer() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kPointer);
    return std::get<Pointer>(value_).value;
  }
  zx_koid_t GetKoid() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kKoid);
    return std::get<Koid>(value_).value;
  }

  std::string ToString() const;

  const std::vector<uint8_t>& GetBlob() const {
    ZX_DEBUG_ASSERT(type() == ArgumentType::kBlob);
    return std::get<std::vector<uint8_t>>(value_);
  }

 private:
  // Strong wrapper types for argument types that have no value or have ambiguous implicit
  // conversions.
  struct Null {};
  struct Bool {
    bool value;
  };
  struct Pointer {
    uint64_t value;
  };
  struct Koid {
    zx_koid_t value;
  };

  ArgumentValue() : value_{Null{}} {}
  explicit ArgumentValue(Bool b) : value_(b) {}
  explicit ArgumentValue(int32_t int32) : value_(int32) {}
  explicit ArgumentValue(uint32_t uint32) : value_(uint32) {}
  explicit ArgumentValue(int64_t int64) : value_(int64) {}
  explicit ArgumentValue(uint64_t uint64) : value_(uint64) {}
  explicit ArgumentValue(double d) : value_(d) {}
  explicit ArgumentValue(std::string string) : value_(std::move(string)) {}
  explicit ArgumentValue(Pointer pointer) : value_(pointer) {}
  explicit ArgumentValue(Koid koid) : value_(koid) {}
  explicit ArgumentValue(std::vector<uint8_t> blob) : value_(blob) {}

  using Variant = std::variant<Null, int32_t, uint32_t, int64_t, uint64_t, double, std::string,
                               Pointer, Koid, Bool, std::vector<uint8_t>>;
  Variant value_;
};

// Named argument and value.
class Argument final {
 public:
  explicit Argument(std::string name, ArgumentValue value)
      : name_(std::move(name)), value_(std::move(value)) {}

  Argument(const Argument&) = default;
  Argument& operator=(const Argument&) = default;

  Argument(Argument&&) = default;
  Argument& operator=(Argument&&) = default;

  const std::string& name() const { return name_; }
  const ArgumentValue& value() const { return value_; }
  ArgumentType type() const { return value_.type(); }

  std::string ToString() const;

  static const Argument* Find(const std::string& name, const std::vector<Argument>& arguments) {
    for (const Argument& argument : arguments) {
      if (argument.name() == name) {
        return &argument;
      }
    }
    return nullptr;
  }

 private:
  std::string name_;
  ArgumentValue value_;
};

// Trace Info type specific data
class TraceInfoContent final {
 public:
  // Magic number record data
  struct MagicNumberInfo {
    uint32_t magic_value;
  };

  explicit TraceInfoContent(MagicNumberInfo magic_number_info) : value_(magic_number_info) {}

  const MagicNumberInfo& GetMagicNumberInfo() const {
    ZX_DEBUG_ASSERT(type() == TraceInfoType::kMagicNumber);
    return std::get<MagicNumberInfo>(value_);
  }

  TraceInfoContent(const TraceInfoContent&) = default;
  TraceInfoContent& operator=(const TraceInfoContent&) = default;
  TraceInfoContent(TraceInfoContent&&) = default;
  TraceInfoContent& operator=(TraceInfoContent&&) = default;
  ~TraceInfoContent() = default;

  TraceInfoType type() const {
    return std::visit(
        [](auto&& arg) -> TraceInfoType {
          using T = std::decay_t<decltype(arg)>;
          if constexpr (std::is_same_v<T, MagicNumberInfo>) {
            return TraceInfoType::kMagicNumber;
          }
        },
        value_);
  }

  std::string ToString() const;

 private:
  using Variant = std::variant<MagicNumberInfo>;
  Variant value_;
};

// Metadata type specific data.
class MetadataContent final {
 public:
  // Provider info event data.
  struct ProviderInfo {
    ProviderId id;
    std::string name;
  };

  // Provider section event data.
  struct ProviderSection {
    ProviderId id;
  };

  // Provider event event data.
  struct ProviderEvent {
    ProviderId id;
    ProviderEventType event;
  };

  // Trace info record data
  struct TraceInfo {
    TraceInfoType type() const { return content.type(); }
    TraceInfoContent content;
  };

  explicit MetadataContent(ProviderInfo provider_info) : value_(std::move(provider_info)) {}

  explicit MetadataContent(ProviderSection provider_section) : value_(provider_section) {}

  explicit MetadataContent(ProviderEvent provider_event) : value_(provider_event) {}

  explicit MetadataContent(TraceInfo trace_info) : value_(trace_info) {}

  const ProviderInfo& GetProviderInfo() const {
    ZX_DEBUG_ASSERT(type() == MetadataType::kProviderInfo);
    return std::get<ProviderInfo>(value_);
  }

  const ProviderSection& GetProviderSection() const {
    ZX_DEBUG_ASSERT(type() == MetadataType::kProviderSection);
    return std::get<ProviderSection>(value_);
  }

  const ProviderEvent& GetProviderEvent() const {
    ZX_DEBUG_ASSERT(type() == MetadataType::kProviderEvent);
    return std::get<ProviderEvent>(value_);
  }

  const TraceInfo& GetTraceInfo() const {
    ZX_DEBUG_ASSERT(type() == MetadataType::kTraceInfo);
    return std::get<TraceInfo>(value_);
  }

  MetadataContent(const MetadataContent&) = default;
  MetadataContent& operator=(const MetadataContent&) = default;
  MetadataContent(MetadataContent&&) = default;
  MetadataContent& operator=(MetadataContent&&) = default;
  ~MetadataContent() = default;

  MetadataType type() const {
    return std::visit(
        [](auto&& arg) -> MetadataType {
          using T = std::decay_t<decltype(arg)>;
          if constexpr (std::is_same_v<T, std::monostate>) {
            return static_cast<MetadataType>(0);
          } else if constexpr (std::is_same_v<T, ProviderInfo>) {
            return MetadataType::kProviderInfo;
          } else if constexpr (std::is_same_v<T, ProviderSection>) {
            return MetadataType::kProviderSection;
          } else if constexpr (std::is_same_v<T, ProviderEvent>) {
            return MetadataType::kProviderEvent;
          } else if constexpr (std::is_same_v<T, TraceInfo>) {
            return MetadataType::kTraceInfo;
          }
        },
        value_);
  }

  std::string ToString() const;

 private:
  using Variant =
      std::variant<std::monostate, ProviderInfo, ProviderSection, ProviderEvent, TraceInfo>;
  Variant value_;
};

// Event type specific data.
class EventData final {
 public:
  // Instant event data.
  struct Instant {
    EventScope scope;
  };

  // Counter event data.
  struct Counter {
    trace_counter_id_t id;
  };

  // Duration begin event data.
  struct DurationBegin {};

  // Duration end event data.
  struct DurationEnd {};

  // Duration complete event data.
  struct DurationComplete {
    trace_ticks_t end_time;
  };

  // Async begin event data.
  struct AsyncBegin {
    trace_async_id_t id;
  };

  // Async instant event data.
  struct AsyncInstant {
    trace_async_id_t id;
  };

  // Async end event data.
  struct AsyncEnd {
    trace_async_id_t id;
  };

  // Flow begin event data.
  struct FlowBegin {
    trace_flow_id_t id;
  };

  // Flow step event data.
  struct FlowStep {
    trace_flow_id_t id;
  };

  // Flow end event data.
  struct FlowEnd {
    trace_flow_id_t id;
  };

  explicit EventData(Instant instant) : value_(instant) {}
  explicit EventData(Counter counter) : value_(counter) {}
  explicit EventData(DurationBegin duration_begin) : value_(duration_begin) {}
  explicit EventData(DurationEnd duration_end) : value_(duration_end) {}
  explicit EventData(DurationComplete duration_complete) : value_(duration_complete) {}
  explicit EventData(AsyncBegin async_begin) : value_(async_begin) {}
  explicit EventData(AsyncInstant async_instant) : value_(async_instant) {}
  explicit EventData(AsyncEnd async_end) : value_(async_end) {}
  explicit EventData(FlowBegin flow_begin) : value_(flow_begin) {}
  explicit EventData(FlowStep flow_step) : value_(flow_step) {}
  explicit EventData(FlowEnd flow_end) : value_(flow_end) {}

  EventData(const EventData&) = default;
  EventData& operator=(const EventData&) = default;
  EventData(EventData&&) = default;
  EventData& operator=(EventData&&) = default;
  ~EventData() = default;

  const Instant& GetInstant() const {
    ZX_DEBUG_ASSERT(type() == EventType::kInstant);
    return std::get<Instant>(value_);
  }

  const Counter& GetCounter() const {
    ZX_DEBUG_ASSERT(type() == EventType::kCounter);
    return std::get<Counter>(value_);
  }

  const DurationBegin& GetDurationBegin() const {
    ZX_DEBUG_ASSERT(type() == EventType::kDurationBegin);
    return std::get<DurationBegin>(value_);
  }

  const DurationEnd& GetDurationEnd() const {
    ZX_DEBUG_ASSERT(type() == EventType::kDurationEnd);
    return std::get<DurationEnd>(value_);
  }

  const DurationComplete& GetDurationComplete() const {
    ZX_DEBUG_ASSERT(type() == EventType::kDurationComplete);
    return std::get<DurationComplete>(value_);
  }

  const AsyncBegin& GetAsyncBegin() const {
    ZX_DEBUG_ASSERT(type() == EventType::kAsyncBegin);
    return std::get<AsyncBegin>(value_);
  }

  const AsyncInstant& GetAsyncInstant() const {
    ZX_DEBUG_ASSERT(type() == EventType::kAsyncInstant);
    return std::get<AsyncInstant>(value_);
  }

  const AsyncEnd& GetAsyncEnd() const {
    ZX_DEBUG_ASSERT(type() == EventType::kAsyncEnd);
    return std::get<AsyncEnd>(value_);
  }

  const FlowBegin& GetFlowBegin() const {
    ZX_DEBUG_ASSERT(type() == EventType::kFlowBegin);
    return std::get<FlowBegin>(value_);
  }

  const FlowStep& GetFlowStep() const {
    ZX_DEBUG_ASSERT(type() == EventType::kFlowStep);
    return std::get<FlowStep>(value_);
  }

  const FlowEnd& GetFlowEnd() const {
    ZX_DEBUG_ASSERT(type() == EventType::kFlowEnd);
    return std::get<FlowEnd>(value_);
  }

  EventType type() const {
    return std::visit(
        [](auto&& arg) -> EventType {
          using T = std::decay_t<decltype(arg)>;
          if constexpr (std::is_same_v<T, Instant>) {
            return EventType::kInstant;
          } else if constexpr (std::is_same_v<T, Counter>) {
            return EventType::kCounter;
          } else if constexpr (std::is_same_v<T, DurationBegin>) {
            return EventType::kDurationBegin;
          } else if constexpr (std::is_same_v<T, DurationEnd>) {
            return EventType::kDurationEnd;
          } else if constexpr (std::is_same_v<T, DurationComplete>) {
            return EventType::kDurationComplete;
          } else if constexpr (std::is_same_v<T, AsyncBegin>) {
            return EventType::kAsyncBegin;
          } else if constexpr (std::is_same_v<T, AsyncInstant>) {
            return EventType::kAsyncInstant;
          } else if constexpr (std::is_same_v<T, AsyncEnd>) {
            return EventType::kAsyncEnd;
          } else if constexpr (std::is_same_v<T, FlowBegin>) {
            return EventType::kFlowBegin;
          } else if constexpr (std::is_same_v<T, FlowStep>) {
            return EventType::kFlowStep;
          } else if constexpr (std::is_same_v<T, FlowEnd>) {
            return EventType::kFlowEnd;
          }
        },
        value_);
  }

  std::string ToString() const;

 private:
  using Variant = std::variant<Instant, Counter, DurationBegin, DurationEnd, DurationComplete,
                               AsyncBegin, AsyncInstant, AsyncEnd, FlowBegin, FlowStep, FlowEnd>;
  Variant value_;
};

// Large record specific data
class LargeRecordData final {
 public:
  struct BlobEvent {
    std::string category;
    std::string name;
    trace_ticks_t timestamp;
    ProcessThread process_thread;
    std::vector<Argument> arguments;

    const void* blob;
    uint64_t blob_size;
  };

  struct BlobAttachment {
    std::string category;
    std::string name;

    const void* blob;
    uint64_t blob_size;
  };

  // Large blob record data.
  // The blob data pointer is actually just a pointer into the trace
  // reader's buffer. As such, the record consumer should not attempt
  // to free it. The record consumer must finish processing the
  // blob data within the callback, as the pointer may not be valid
  // after the completion of that callback.
  using Blob = std::variant<BlobEvent, BlobAttachment>;

  explicit LargeRecordData(Blob blob) : value_(std::move(blob)) {}

  const Blob& GetBlob() const {
    ZX_DEBUG_ASSERT(type() == LargeRecordType::kBlob);
    return std::get<Blob>(value_);
  }

  LargeRecordData(const LargeRecordData&) = default;
  LargeRecordData& operator=(const LargeRecordData&) = default;
  LargeRecordData(LargeRecordData&&) = default;
  LargeRecordData& operator=(LargeRecordData&&) = default;
  ~LargeRecordData() = default;

  LargeRecordType type() const {
    return std::visit(
        [](auto&& arg) -> LargeRecordType {
          using T = std::decay_t<decltype(arg)>;
          if constexpr (std::is_same_v<T, Blob>) {
            return LargeRecordType::kBlob;
          }
        },
        value_);
  }

  std::string ToString() const;

 private:
  using Variant = std::variant<Blob>;
  Variant value_;
};

// A decoded record.
// See docs/reference/tracing/trace-format.md#record_types
class Record final {
 public:
  // Metadata record data.
  struct Metadata {
    MetadataType type() const { return content.type(); }
    MetadataContent content;
  };

  // Initialization record data.
  struct Initialization {
    trace_ticks_t ticks_per_second;
  };

  // String record data.
  struct String {
    trace_string_index_t index;
    std::string string;
  };

  // Thread record data.
  struct Thread {
    trace_thread_index_t index;
    ProcessThread process_thread;
  };

  // Event record data.
  struct Event {
    EventType type() const { return data.type(); }
    trace_ticks_t timestamp;
    ProcessThread process_thread;
    std::string category;
    std::string name;
    std::vector<Argument> arguments;
    EventData data;
  };

  // Blob record data.
  // Since blobs can be rather large we avoid unnecessary copying of them.
  // This then means that the consumer must process the blob's payload
  // before the next record is read.
  struct Blob {
    trace_blob_type_t type;
    std::string name;
    const void* blob;
    size_t blob_size;
  };

  // Kernel Object record data.
  struct KernelObject {
    zx_koid_t koid;
    zx_obj_type_t object_type;
    std::string name;
    std::vector<Argument> arguments;
  };

  // Scheduler Event record data.
  struct SchedulerEvent {
    struct LegacyContextSwitch {
      trace_ticks_t timestamp;
      trace_cpu_number_t cpu_number;
      ThreadState outgoing_thread_state;
      ProcessThread outgoing_thread;
      ProcessThread incoming_thread;
      trace_thread_priority_t outgoing_thread_priority;
      trace_thread_priority_t incoming_thread_priority;
    };
    struct ContextSwitch {
      trace_ticks_t timestamp;
      trace_cpu_number_t cpu_number;
      ThreadState outgoing_thread_state;
      zx_koid_t outgoing_tid;
      zx_koid_t incoming_tid;
      std::vector<Argument> arguments;

      const Argument* FindArgument(const std::string& name) const {
        return Argument::Find(name, arguments);
      }
    };
    struct ThreadWakeup {
      trace_ticks_t timestamp;
      trace_cpu_number_t cpu_number;
      zx_koid_t incoming_tid;
      std::vector<Argument> arguments;

      const Argument* FindArgument(const std::string& name) const {
        return Argument::Find(name, arguments);
      }
    };

    explicit SchedulerEvent(LegacyContextSwitch record)
        : event_type{SchedulerEventType::kLegacyContextSwitch}, event{std::move(record)} {}
    explicit SchedulerEvent(ContextSwitch record)
        : event_type{SchedulerEventType::kContextSwitch}, event{std::move(record)} {}
    explicit SchedulerEvent(ThreadWakeup record)
        : event_type{SchedulerEventType::kThreadWakeup}, event{std::move(record)} {}

    SchedulerEvent(const SchedulerEvent&) = default;
    SchedulerEvent& operator=(const SchedulerEvent&) = default;
    SchedulerEvent(SchedulerEvent&&) = default;
    SchedulerEvent& operator=(SchedulerEvent&&) = default;

    SchedulerEventType type() const { return event_type; }

    const LegacyContextSwitch& legacy_context_switch() const {
      ZX_DEBUG_ASSERT(event_type == SchedulerEventType::kLegacyContextSwitch);
      return std::get<LegacyContextSwitch>(event);
    }
    const ContextSwitch& context_switch() const {
      ZX_DEBUG_ASSERT(event_type == SchedulerEventType::kContextSwitch);
      return std::get<ContextSwitch>(event);
    }
    const ThreadWakeup& thread_wakeup() const {
      ZX_DEBUG_ASSERT(event_type == SchedulerEventType::kThreadWakeup);
      return std::get<ThreadWakeup>(event);
    }

    SchedulerEventType event_type;
    std::variant<LegacyContextSwitch, ContextSwitch, ThreadWakeup> event;
  };

  // Log record data.
  struct Log {
    trace_ticks_t timestamp;
    ProcessThread process_thread;
    std::string message;
  };

  // Large record data.
  using Large = LargeRecordData;

  // Profiler record data.
  struct Profiler {
    struct Module {
      uint16_t module_id;
      ProcessThread process_thread;
      trace_ticks_t timestamp;
      std::string name;
      std::vector<std::byte> build_id;
    };
    struct Mmap {
      uint16_t module_id;
      ProcessThread process_thread;
      trace_ticks_t timestamp;
      uint64_t start_address;
      uint64_t address_range;
      uint64_t vaddr;
      uint8_t flags;
    };
    struct Backtrace {
      ProcessThread process_thread;
      trace_ticks_t timestamp;
      std::vector<uint64_t> backtrace;
    };

    explicit Profiler(Module record)
        : profiler_type{ProfilerRecordType::kModule}, event{std::move(record)} {}
    explicit Profiler(Mmap record)
        : profiler_type{ProfilerRecordType::kMmap}, event{std::move(record)} {}
    explicit Profiler(Backtrace record)
        : profiler_type{ProfilerRecordType::kBacktrace}, event{std::move(record)} {}

    Profiler(const Profiler&) = default;
    Profiler& operator=(const Profiler&) = default;
    Profiler(Profiler&&) = default;
    Profiler& operator=(Profiler&&) = default;

    ProfilerRecordType type() const { return profiler_type; }

    const Module& module() const {
      ZX_DEBUG_ASSERT(profiler_type == ProfilerRecordType::kModule);
      return std::get<Module>(event);
    }
    const Mmap& mmap() const {
      ZX_DEBUG_ASSERT(profiler_type == ProfilerRecordType::kMmap);
      return std::get<Mmap>(event);
    }
    const Backtrace& backtrace() const {
      ZX_DEBUG_ASSERT(profiler_type == ProfilerRecordType::kBacktrace);
      return std::get<Backtrace>(event);
    }

    ProfilerRecordType profiler_type;
    std::variant<Module, Mmap, Backtrace> event;
  };

  explicit Record(Metadata record) : value_(std::move(record)) {}
  explicit Record(Initialization record) : value_(record) {}
  explicit Record(String record) : value_(std::move(record)) {}
  explicit Record(Thread record) : value_(std::move(record)) {}
  explicit Record(Event record) : value_(std::move(record)) {}
  explicit Record(Blob record) : value_(std::move(record)) {}
  explicit Record(KernelObject record) : value_(std::move(record)) {}
  explicit Record(SchedulerEvent record) : value_(std::move(record)) {}
  explicit Record(Log record) : value_(std::move(record)) {}
  explicit Record(Large record) : value_(std::move(record)) {}
  explicit Record(Profiler record) : value_(std::move(record)) {}

  Record(const Record&) = default;
  Record& operator=(const Record&) = default;
  Record(Record&&) = default;
  Record& operator=(Record&&) = default;
  ~Record() = default;

  const Metadata& GetMetadata() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kMetadata);
    return std::get<Metadata>(value_);
  }

  const Initialization& GetInitialization() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kInitialization);
    return std::get<Initialization>(value_);
  }

  const String& GetString() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kString);
    return std::get<String>(value_);
  }

  const Thread& GetThread() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kThread);
    return std::get<Thread>(value_);
  }

  const Event& GetEvent() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kEvent);
    return std::get<Event>(value_);
  }

  const Blob& GetBlob() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kBlob);
    return std::get<Blob>(value_);
  }

  const KernelObject& GetKernelObject() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kKernelObject);
    return std::get<KernelObject>(value_);
  }

  const SchedulerEvent& GetSchedulerEvent() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kScheduler);
    return std::get<SchedulerEvent>(value_);
  }

  const Log& GetLog() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kLog);
    return std::get<Log>(value_);
  }

  const Large& GetLargeRecord() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kLargeRecord);
    return std::get<Large>(value_);
  }

  const Profiler& GetProfiler() const {
    ZX_DEBUG_ASSERT(type() == RecordType::kProfiler);
    return std::get<Profiler>(value_);
  }

  RecordType type() const {
    return std::visit(
        [](auto&& arg) -> RecordType {
          using T = std::decay_t<decltype(arg)>;
          if constexpr (std::is_same_v<T, Metadata>) {
            return RecordType::kMetadata;
          } else if constexpr (std::is_same_v<T, Initialization>) {
            return RecordType::kInitialization;
          } else if constexpr (std::is_same_v<T, String>) {
            return RecordType::kString;
          } else if constexpr (std::is_same_v<T, Thread>) {
            return RecordType::kThread;
          } else if constexpr (std::is_same_v<T, Event>) {
            return RecordType::kEvent;
          } else if constexpr (std::is_same_v<T, Blob>) {
            return RecordType::kBlob;
          } else if constexpr (std::is_same_v<T, KernelObject>) {
            return RecordType::kKernelObject;
          } else if constexpr (std::is_same_v<T, SchedulerEvent>) {
            return RecordType::kScheduler;
          } else if constexpr (std::is_same_v<T, Log>) {
            return RecordType::kLog;
          } else if constexpr (std::is_same_v<T, Large>) {
            return RecordType::kLargeRecord;
          } else if constexpr (std::is_same_v<T, Profiler>) {
            return RecordType::kProfiler;
          }
        },
        value_);
  }

  std::string ToString() const;

  std::optional<std::string> GetName() const;

 private:
  using Variant = std::variant<Metadata, Initialization, String, Thread, Event, Blob, KernelObject,
                               SchedulerEvent, Log, Large, Profiler>;
  Variant value_;
};

}  // namespace trace

#endif  // TRACE_READER_RECORDS_H_
