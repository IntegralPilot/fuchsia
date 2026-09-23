// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <inttypes.h>
#include <string.h>

#include <iomanip>
#include <sstream>
#include <utility>

#include <trace-reader/records.h>

namespace trace {
namespace {
const char* EventScopeToString(EventScope scope) {
  switch (scope) {
    case EventScope::kGlobal:
      return "global";
    case EventScope::kProcess:
      return "process";
    case EventScope::kThread:
      return "thread";
  }
  return "???";
}

const char* ThreadStateToString(ThreadState state) {
  switch (state) {
    case ThreadState::kNew:
      return "new";
    case ThreadState::kRunning:
      return "running";
    case ThreadState::kSuspended:
      return "suspended";
    case ThreadState::kBlocked:
      return "blocked";
    case ThreadState::kDying:
      return "dying";
    case ThreadState::kDead:
      return "dead";
  }
  return "???";
}

const char* ObjectTypeToString(zx_obj_type_t type) {
  switch (type) {
    case ZX_OBJ_TYPE_PROCESS:
      return "process";
    case ZX_OBJ_TYPE_THREAD:
      return "thread";
    case ZX_OBJ_TYPE_VMO:
      return "vmo";
    case ZX_OBJ_TYPE_CHANNEL:
      return "channel";
    case ZX_OBJ_TYPE_EVENT:
      return "event";
    case ZX_OBJ_TYPE_PORT:
      return "port";
    case ZX_OBJ_TYPE_INTERRUPT:
      return "interrupt";
    case ZX_OBJ_TYPE_LOG:
      return "log";
    case ZX_OBJ_TYPE_SOCKET:
      return "socket";
    case ZX_OBJ_TYPE_RESOURCE:
      return "resource";
    case ZX_OBJ_TYPE_EVENTPAIR:
      return "event-pair";
    case ZX_OBJ_TYPE_JOB:
      return "job";
    case ZX_OBJ_TYPE_VMAR:
      return "vmar";
    case ZX_OBJ_TYPE_FIFO:
      return "fifo";
    case ZX_OBJ_TYPE_GUEST:
      return "guest";
    case ZX_OBJ_TYPE_VCPU:
      return "vcpu";
    case ZX_OBJ_TYPE_TIMER:
      return "timer";
    case ZX_OBJ_TYPE_IOMMU:
      return "iommu";
    case ZX_OBJ_TYPE_BTI:
      return "bti";
    case ZX_OBJ_TYPE_PROFILE:
      return "profile";
    case ZX_OBJ_TYPE_PMT:
      return "pmt";
    case ZX_OBJ_TYPE_SUSPEND_TOKEN:
      return "suspend-token";
    case ZX_OBJ_TYPE_PAGER:
      return "pager";
    case ZX_OBJ_TYPE_EXCEPTION:
      return "exception";
    default:
      return "???";
  }
}

template <size_t max_preview_start, size_t max_preview_end>
std::string PreviewBlobData(const void* blob, size_t blob_size) {
  static_assert((max_preview_start + max_preview_end) > 0);

  auto blob_data = (const unsigned char*)blob;
  std::string result;
  result.reserve((3 * (max_preview_start + max_preview_end)) + 128);

  size_t num_leading_bytes;
  size_t num_trailing_bytes;
  if (blob_size <= (max_preview_start + max_preview_end)) {
    num_leading_bytes = blob_size;
    num_trailing_bytes = 0;
  } else {
    num_leading_bytes = max_preview_start;
    num_trailing_bytes = max_preview_end;
  }

  char buf[3];  // Shared buffer into which to format hex data byte by byte.
  result.append("<");
  for (size_t i = 0; i < num_leading_bytes; i++) {
    if (i > 0)
      result.append(" ");
    snprintf(buf, sizeof(buf), "%02x", blob_data[i]);
    result.append(buf);
  }
  if (num_trailing_bytes)
    result.append(" ...");
  for (size_t i = blob_size - num_trailing_bytes; i < blob_size; i++) {
    result.append(" ");
    snprintf(buf, sizeof(buf), "%02x", blob_data[i]);
    result.append(buf);
  }
  result.append(">");

  result.shrink_to_fit();
  return result;
}

std::string FormatArgumentList(const std::vector<trace::Argument>& args) {
  std::string result;
  result.reserve(1024);

  result.append("{");
  for (size_t i = 0; i < args.size(); i++) {
    if (i != 0)
      result.append(", ");
    result.append(args[i].ToString());
  }
  result.append("}");

  result.shrink_to_fit();
  return result;
}
}  // namespace

std::string ProcessThread::ToString() const {
  return (std::stringstream() << process_koid_ << "/" << thread_koid_).str();
}

std::string ArgumentValue::ToString() const {
  switch (type()) {
    case ArgumentType::kNull:
      return "null";
    case ArgumentType::kBool: {
      std::stringstream ss;
      ss << std::boolalpha << "bool(" << std::get<Bool>(value_).value << ")";
      return ss.str();
    }
    case ArgumentType::kInt32:
      return (std::stringstream() << "int32(" << std::get<int32_t>(value_) << ")").str();
    case ArgumentType::kUint32:
      return (std::stringstream() << "uint32(" << std::get<uint32_t>(value_) << ")").str();
    case ArgumentType::kInt64:
      return (std::stringstream() << "int64(" << std::get<int64_t>(value_) << ")").str();
    case ArgumentType::kUint64:
      return (std::stringstream() << "uint64(" << std::get<uint64_t>(value_) << ")").str();
    case ArgumentType::kDouble: {
      std::stringstream ss;
      ss << std::fixed << std::setprecision(6) << "double(" << std::get<double>(value_) << ")";
      return ss.str();
    }
    case ArgumentType::kString:
      return (std::stringstream() << "string(\"" << std::get<std::string>(value_) << "\")").str();
    case ArgumentType::kPointer: {
      auto p = std::get<Pointer>(value_).value;
      return (std::stringstream() << std::hex << "pointer(" << (p ? "0x" : "") << p << ")").str();
    }
    case ArgumentType::kKoid:
      return (std::stringstream() << "koid(" << std::get<Koid>(value_).value << ")").str();
    case ArgumentType::kBlob:
      return (std::stringstream() << "blob(length=" << std::get<std::vector<uint8_t>>(value_).size()
                                  << ")")
          .str();
  }
  ZX_ASSERT(false);
}

std::string Argument::ToString() const {
  return (std::stringstream() << name_ << ": " << value_.ToString()).str();
}

std::string TraceInfoContent::ToString() const {
  switch (type()) {
    case TraceInfoType::kMagicNumber: {
      std::stringstream ss;
      ss << "MagicNumberInfo(magic_value: 0x" << std::hex << GetMagicNumberInfo().magic_value
         << ")";
      return ss.str();
    }
  }
  ZX_ASSERT(false);
}

std::string MetadataContent::ToString() const {
  std::stringstream ss;
  switch (type()) {
    case MetadataType::kProviderInfo:
      ss << "ProviderInfo(id: " << GetProviderInfo().id << ", name: \"" << GetProviderInfo().name
         << "\")";
      return ss.str();
    case MetadataType::kProviderSection:
      ss << "ProviderSection(id: " << GetProviderSection().id << ")";
      return ss.str();
    case MetadataType::kProviderEvent: {
      ss << "ProviderEvent(id: " << GetProviderEvent().id << ", ";
      ProviderEventType type = GetProviderEvent().event;
      switch (type) {
        case ProviderEventType::kBufferOverflow:
          ss << "buffer overflow";
          break;
        default: {
          ss << "unknown(" << static_cast<unsigned>(type) << ")";
          break;
        }
      }
      ss << ")";
      return ss.str();
    }
    case MetadataType::kTraceInfo: {
      ss << "TraceInfo(content: " << GetTraceInfo().content.ToString() << ")";
      return ss.str();
    }
  }
  ZX_ASSERT(false);
}

std::string EventData::ToString() const {
  switch (type()) {
    case EventType::kInstant: {
      std::stringstream ss;
      ss << "Instant(scope: " << EventScopeToString(GetInstant().scope) << ")";
      return ss.str();
    }
    case EventType::kCounter:
      return (std::stringstream() << "Counter(id: " << GetCounter().id << ")").str();
    case EventType::kDurationBegin:
      return "DurationBegin";
    case EventType::kDurationEnd:
      return "DurationEnd";
    case EventType::kDurationComplete: {
      std::stringstream ss;
      ss << "DurationComplete(end_ts: " << GetDurationComplete().end_time << ")";
      return ss.str();
    }
    case EventType::kAsyncBegin:
      return (std::stringstream() << "AsyncBegin(id: " << GetAsyncBegin().id << ")").str();
    case EventType::kAsyncInstant:
      return (std::stringstream() << "AsyncInstant(id: " << GetAsyncInstant().id << ")").str();
    case EventType::kAsyncEnd:
      return (std::stringstream() << "AsyncEnd(id: " << GetAsyncEnd().id << ")").str();
    case EventType::kFlowBegin:
      return (std::stringstream() << "FlowBegin(id: " << GetFlowBegin().id << ")").str();
    case EventType::kFlowStep:
      return (std::stringstream() << "FlowStep(id: " << GetFlowStep().id << ")").str();
    case EventType::kFlowEnd:
      return (std::stringstream() << "FlowEnd(id: " << GetFlowEnd().id << ")").str();
  }
  ZX_ASSERT(false);
}

std::string LargeRecordData::ToString() const {
  std::stringstream ss;
  switch (type()) {
    case LargeRecordType::kBlob:
      if (std::holds_alternative<BlobEvent>(GetBlob())) {
        const auto& data = std::get<BlobEvent>(GetBlob());
        ss << "Blob(format: blob_event, category: \"" << data.category << "\""
           << ", name: \"" << data.name << "\""
           << ", ts: " << data.timestamp << ", pt: " << data.process_thread.ToString() << ", "
           << FormatArgumentList(data.arguments) << ", size: " << data.blob_size
           << ", preview: " << PreviewBlobData<8, 8>(data.blob, data.blob_size) << ")";
        return ss.str();
      } else if (std::holds_alternative<BlobAttachment>(GetBlob())) {
        const auto& data = std::get<BlobAttachment>(GetBlob());
        ss << "Blob(format: blob_attachment, category: \"" << data.category << "\""
           << ", name: \"" << data.name << "\""
           << ", size: " << data.blob_size
           << ", preview: " << PreviewBlobData<8, 8>(data.blob, data.blob_size) << ")";
        return ss.str();
      }
      break;
  }
  ZX_ASSERT(false);
}

std::string Record::ToString() const {
  std::stringstream ss;
  switch (type()) {
    case RecordType::kMetadata:
      ss << "Metadata(content: " << GetMetadata().content.ToString() << ")";
      return ss.str();
    case RecordType::kInitialization:
      ss << "Initialization(ticks_per_second: " << GetInitialization().ticks_per_second << ")";
      return ss.str();
    case RecordType::kString:
      ss << "String(index: " << GetString().index << ", \"" << GetString().string << "\")";
      return ss.str();
    case RecordType::kThread:
      ss << "Thread(index: " << GetThread().index << ", " << GetThread().process_thread.ToString()
         << ")";
      return ss.str();
    case RecordType::kEvent:
      ss << "Event(ts: " << GetEvent().timestamp << ", pt: " << GetEvent().process_thread.ToString()
         << ", category: \"" << GetEvent().category << "\", name: \"" << GetEvent().name << "\", "
         << GetEvent().data.ToString() << ", " << FormatArgumentList(GetEvent().arguments) << ")";
      return ss.str();
    case RecordType::kBlob:
      ss << "Blob(name: " << GetBlob().name << ", size: " << GetBlob().blob_size
         << ", preview: " << PreviewBlobData<8, 8>(GetBlob().blob, GetBlob().blob_size) << ")";
      return ss.str();
    case RecordType::kKernelObject:
      ss << "KernelObject(koid: " << GetKernelObject().koid
         << ", type: " << ObjectTypeToString(GetKernelObject().object_type) << ", name: \""
         << GetKernelObject().name << "\", " << FormatArgumentList(GetKernelObject().arguments)
         << ")";
      return ss.str();
    case RecordType::kScheduler:
      if (GetSchedulerEvent().type() == SchedulerEventType::kLegacyContextSwitch) {
        auto& context_switch = GetSchedulerEvent().legacy_context_switch();
        ss << "ContextSwitch(ts: " << context_switch.timestamp
           << ", cpu: " << context_switch.cpu_number
           << ", os: " << ThreadStateToString(context_switch.outgoing_thread_state)
           << ", opt: " << context_switch.outgoing_thread.ToString()
           << ", ipt: " << context_switch.incoming_thread.ToString()
           << ", oprio: " << context_switch.outgoing_thread_priority
           << ", iprio: " << context_switch.incoming_thread_priority << ")";
      } else if (GetSchedulerEvent().type() == SchedulerEventType::kContextSwitch) {
        auto& context_switch = GetSchedulerEvent().context_switch();
        ss << "ContextSwitch(ts: " << context_switch.timestamp
           << ", cpu: " << context_switch.cpu_number
           << ", os: " << ThreadStateToString(context_switch.outgoing_thread_state)
           << ", ot: " << context_switch.outgoing_tid << ", it: " << context_switch.incoming_tid
           << ", " << FormatArgumentList(context_switch.arguments) << ")";
      } else if (GetSchedulerEvent().type() == SchedulerEventType::kThreadWakeup) {
        auto& thread_wakeup = GetSchedulerEvent().thread_wakeup();
        ss << "ThreadWakeup(ts: " << thread_wakeup.timestamp
           << ", cpu: " << thread_wakeup.cpu_number << ", it: " << thread_wakeup.incoming_tid
           << ", " << FormatArgumentList(thread_wakeup.arguments) << ")";
      } else {
        ss << "UnknownSchedulerEvent(type: " << static_cast<int>(GetSchedulerEvent().type()) << ")";
      }
      return ss.str();
    case RecordType::kLog:
      ss << "Log(ts: " << GetLog().timestamp << ", pt: " << GetLog().process_thread.ToString()
         << ", \"" << GetLog().message << "\")";
      return ss.str();
    case RecordType::kLargeRecord:
      ss << "LargeRecord(" << GetLargeRecord().ToString() << ")";
      return ss.str();
    case RecordType::kProfiler:
      if (GetProfiler().type() == ProfilerRecordType::kModule) {
        auto& module = GetProfiler().module();
        ss << "ProfilerModule(ts: " << module.timestamp
           << ", pt: " << module.process_thread.ToString() << ", module_id: " << module.module_id
           << ", name: \"" << module.name << "\", build_id: \"";
        for (const auto& byte : module.build_id) {
          ss << std::hex << std::setw(2) << std::setfill('0') << static_cast<uint32_t>(byte);
        }
        ss << "\")";
      } else if (GetProfiler().type() == ProfilerRecordType::kMmap) {
        auto& mmap = GetProfiler().mmap();
        ss << "ProfilerMmap(ts: " << mmap.timestamp << ", pt: " << mmap.process_thread.ToString()
           << ", module_id: " << mmap.module_id << ", start_address: 0x" << std::hex
           << mmap.start_address << ", address_range: 0x" << std::hex << mmap.address_range
           << ", vaddr: 0x" << std::hex << mmap.vaddr << ", flags: " << (int)mmap.flags << ")";
      } else if (GetProfiler().type() == ProfilerRecordType::kBacktrace) {
        auto& backtrace = GetProfiler().backtrace();
        ss << "ProfilerBacktrace(ts: " << backtrace.timestamp
           << ", pt: " << backtrace.process_thread.ToString()
           << ", backtrace_count: " << backtrace.backtrace.size();
        for (const auto& addr : backtrace.backtrace) {
          ss << ", addr: 0x" << std::hex << addr;
        }
        ss << ")";
      } else {
        ss << "UnknownProfilerRecord(type: " << static_cast<int>(GetProfiler().type()) << ")";
      }
      return ss.str();
  }
  ZX_ASSERT(false);
}

std::optional<std::string> Record::GetName() const {
  switch (type()) {
    // Do not have a namefield
    case RecordType::kMetadata:
    case RecordType::kInitialization:
    case RecordType::kString:
    case RecordType::kThread:
    case RecordType::kScheduler:
    case RecordType::kLog:
    case RecordType::kLargeRecord:
    case RecordType::kProfiler:
      return std::nullopt;
    case RecordType::kEvent:
      return {GetEvent().name};
    case RecordType::kBlob:
      return {GetBlob().name};
    case RecordType::kKernelObject:
      return {GetKernelObject().name};
  }
}

}  // namespace trace
