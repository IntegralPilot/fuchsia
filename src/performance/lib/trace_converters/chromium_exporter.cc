// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/lib/trace_converters/chromium_exporter.h"

#include <inttypes.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/types.h>
#include <simdutf.h>

#include <filesystem>
#include <memory>
#include <string_view>
#include <utility>
#include <variant>

#include <trace-reader/reader.h>

#include "rapidjson/writer.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/fxl/strings/utf_codecs.h"

namespace tracing {
namespace {

constexpr char kProcessArgKey[] = "process";
constexpr zx_koid_t kNoProcess = 0u;
constexpr uint32_t kUnicodeReplacementCharacter = 0xFFFD;

bool IsEventTypeSupported(trace::EventType type) {
  switch (type) {
    case trace::EventType::kInstant:
    case trace::EventType::kCounter:
    case trace::EventType::kDurationBegin:
    case trace::EventType::kDurationEnd:
    case trace::EventType::kDurationComplete:
    case trace::EventType::kAsyncBegin:
    case trace::EventType::kAsyncInstant:
    case trace::EventType::kAsyncEnd:
    case trace::EventType::kFlowBegin:
    case trace::EventType::kFlowStep:
    case trace::EventType::kFlowEnd:
      return true;
    default:
      break;
  }

  return false;
}

const trace::ArgumentValue* GetArgumentValue(const std::vector<trace::Argument>& arguments,
                                             const char* name) {
  for (const auto& arg : arguments) {
    if (arg.name() == name)
      return &arg.value();
  }
  return nullptr;
}

// The JSON specification requires that the JSON is valid unicode. This function
// replaces any invalid unicode sequences with the replacement character, so
// that the output will be valid UTF-8, even if a trace provider gives us
// invalid UTF-8 in a string.
std::string CleanString(std::string_view str) {
  std::string result;
  const char* data = str.data();
  const size_t len = str.length();
  size_t char_index = 0;

  while (char_index < len) {
    uint32_t code_point;
    if (!fxl::ReadUnicodeCharacter(data, len, &char_index, &code_point)) {
      static bool logged_once = false;
      if (!logged_once) {
        FX_LOGS(WARNING) << "Invalid unicode present in trace";
        logged_once = true;
      }
      code_point = kUnicodeReplacementCharacter;
    }
    fxl::WriteUnicodeCharacter(code_point, &result);
    char_index++;
  }

  return result;
}

inline std::string Base64Encode(std::string_view bytes) {
  std::string result(simdutf::base64_length_from_binary(bytes.size()), '\0');
  size_t written = simdutf::binary_to_base64(bytes.data(), bytes.size(), result.data());
  result.resize(written);
  return result;
}

template <typename WriteObject>
void WriteProcessInfo(WriteObject& writer, zx_koid_t process_koid, const std::string& name) {
  writer.StartObject();
  writer.Key("ph");
  writer.String("p");
  writer.Key("pid");
  writer.Uint64(process_koid);
  writer.Key("name");
  writer.String(CleanString(name));

  if (process_koid == kNoProcess) {
    writer.Key("sort_index");
    writer.Int64(-1);
  }
  writer.EndObject();
}

template <typename WriteObject>
void WriteThreadInfo(WriteObject& writer, zx_koid_t process_koid, zx_koid_t thread_koid,
                     const std::string& name) {
  writer.StartObject();
  writer.Key("ph");
  writer.String("t");
  writer.Key("pid");
  writer.Uint64(process_koid);
  writer.Key("tid");
  writer.Uint64(thread_koid);
  writer.Key("name");
  writer.String(CleanString(name));
  writer.EndObject();
}

}  // namespace

class ChromiumExporter::SystemEventsWriter {
 public:
  virtual ~SystemEventsWriter() = default;
  virtual void Start() = 0;
  virtual void Stop() = 0;
  virtual void ExportProcessInfo(zx_koid_t process_koid, const std::string& name) = 0;
  virtual void ExportThreadInfo(zx_koid_t process_koid, zx_koid_t thread_koid,
                                const std::string& name) = 0;
  virtual void ExportSchedulerEvent(const trace::Record::SchedulerEvent& scheduler_event,
                                    double tick_scale) = 0;

 protected:
  SystemEventsWriter() = default;
};

// Supports conversion to the standard Chromium JSON trace format, in which scheduler, process and
// thread information is under a top-level field named `systemTraceEvents`.
class InlineSystemEventsWriter : public ChromiumExporter::SystemEventsWriter {
 public:
  explicit InlineSystemEventsWriter(rapidjson::Writer<rapidjson::FileWriteStream>& writer)
      : writer_(writer) {}
  ~InlineSystemEventsWriter() override = default;

  void Start() override {
    writer_.Key("systemTraceEvents");
    writer_.StartObject();
    writer_.Key("type");
    writer_.String("fuchsia");
    writer_.Key("events");
    writer_.StartArray();
  }

  void Stop() override {
    writer_.EndArray();
    writer_.EndObject();  // Finishes systemTraceEvents
  }

  void ExportProcessInfo(zx_koid_t process_koid, const std::string& name) override {
    WriteProcessInfo(writer_, process_koid, name);
  }

  void ExportThreadInfo(zx_koid_t process_koid, zx_koid_t thread_koid,
                        const std::string& name) override {
    WriteThreadInfo(writer_, process_koid, thread_koid, name);
  }
  void ExportSchedulerEvent(const trace::Record::SchedulerEvent& scheduler_event,
                            double tick_scale) override;

 private:
  rapidjson::Writer<rapidjson::FileWriteStream>& writer_;
};

// Emits scheduler, process and thread information into a separate, jsonlines-formatted file.
class SplitSystemEventsWriter : public ChromiumExporter::SystemEventsWriter {
 public:
  explicit SplitSystemEventsWriter(const std::filesystem::path& out_path)
      : system_trace_events_fp_(std::fopen(out_path.c_str(), "wb")) {}
  ~SplitSystemEventsWriter() override { std::fclose(system_trace_events_fp_); }
  void Start() override {}
  void Stop() override {}

  void ExportProcessInfo(zx_koid_t process_koid, const std::string& name) override {
    rapidjson::StringBuffer buffer;
    rapidjson::Writer<rapidjson::StringBuffer> writer(buffer);
    WriteProcessInfo(writer, process_koid, name);
    fprintf(system_trace_events_fp_, "%s\n", buffer.GetString());
  }

  void ExportThreadInfo(zx_koid_t process_koid, zx_koid_t thread_koid,
                        const std::string& name) override {
    rapidjson::StringBuffer buffer;
    rapidjson::Writer<rapidjson::StringBuffer> writer(buffer);
    WriteThreadInfo(writer, process_koid, thread_koid, name);
    fprintf(system_trace_events_fp_, "%s\n", buffer.GetString());
  }

  // Defined below so it can reference WriteSchedulerEvent().
  void ExportSchedulerEvent(const trace::Record::SchedulerEvent& scheduler_event,
                            double tick_scale) override;

 private:
  FILE* system_trace_events_fp_;
};

ChromiumExporter::ChromiumExporter(const std::filesystem::path& out_path)
    : fp_(std::fopen(out_path.c_str(), "wb")),
      wrapper_(fp_, write_buffer_, sizeof(write_buffer_)),
      writer_(wrapper_),
      system_events_writer_(std::make_unique<InlineSystemEventsWriter>(writer_)) {
  Start();
}

// This constructor doesn't delegate to the single-argument version, because doing so would mean
// that, during Start(), `system_events_writer_` would point to the wrong file. That's fine _today_,
// as no system trace event data is emitted during Start(), but nothing enforces that.
ChromiumExporter::ChromiumExporter(const std::filesystem::path& out_path,
                                   const std::filesystem::path& system_trace_out_path)
    : fp_(std::fopen(out_path.c_str(), "wb")),
      wrapper_(fp_, write_buffer_, sizeof(write_buffer_)),
      writer_(wrapper_),
      system_events_writer_(std::make_unique<SplitSystemEventsWriter>(system_trace_out_path)) {
  Start();
}

ChromiumExporter::~ChromiumExporter() {
  Stop();
  std::fclose(fp_);
}

void ChromiumExporter::Start() {
  writer_.StartObject();
  writer_.Key("displayTimeUnit");
  writer_.String("ns");
  writer_.Key("traceEvents");
  writer_.StartArray();
}

void ChromiumExporter::StartSchedulerPass() {
  writer_.EndArray();
  system_events_writer_->Start();

  for (const auto& pair : processes_) {
    system_events_writer_->ExportProcessInfo(pair.first, pair.second);
  }

  for (const auto& process_threads : threads_) {
    const zx_koid_t process_koid = process_threads.first;
    for (const auto& thread : process_threads.second) {
      system_events_writer_->ExportThreadInfo(process_koid, thread.first, thread.second);
    }
  }
  pass_ = Pass::kScheduler;
}

void ChromiumExporter::Stop() {
  if (!OnSchedulerPass()) {
    ChromiumExporter::StartSchedulerPass();
  }
  system_events_writer_->Stop();  // Finishes systemTraceEvents
  writer_.EndObject();            // Finishes StartObject() begun in Start()
}

void ChromiumExporter::ExportRecord(const trace::Record& record) {
  if (OnSchedulerPass()) {
    if (record.type() == trace::RecordType::kScheduler) {
      ExportSchedulerEvent(record.GetSchedulerEvent());
    }
    return;
  }
  switch (record.type()) {
    case trace::RecordType::kMetadata:
      ExportMetadata(record.GetMetadata());
      break;
    case trace::RecordType::kInitialization:
      // Compute scale factor for ticks to microseconds.
      // Microseconds is the unit for the "ts" field.
      tick_scale_ = 1'000'000.0 / static_cast<double>(record.GetInitialization().ticks_per_second);
      break;
    case trace::RecordType::kEvent:
      ExportEvent(record.GetEvent());
      break;
    case trace::RecordType::kKernelObject:
      ExportKernelObject(record.GetKernelObject());
      break;
    case trace::RecordType::kBlob: {
      const auto& blob = record.GetBlob();
      // Drop the record.
      FX_LOGS(INFO) << "Dropping blob record: "
                    << "name " << blob.name.c_str() << " of size " << blob.blob_size;
      break;
    }
    case trace::RecordType::kLog:
      ExportLog(record.GetLog());
      break;
    // The Chromium trace view does not support profiler records.
    case trace::RecordType::kProfiler:
    case trace::RecordType::kScheduler:
    case trace::RecordType::kString:
    case trace::RecordType::kThread:
      // We can ignore these, trace::TraceReader consumes them and maintains
      // tables for future lookup.
      break;
    case trace::RecordType::kLargeRecord:
      switch (record.GetLargeRecord().type()) {
        case trace::LargeRecordType::kBlob:
          ExportBlob(record.GetLargeRecord().GetBlob());
          break;
        default:
          break;
      }
      break;
  }
}

void ChromiumExporter::ExportEvent(const trace::Record::Event& event) {
  if (!IsEventTypeSupported(event.type()))
    return;

  writer_.StartObject();

  writer_.Key("cat");
  writer_.String(CleanString(event.category));
  writer_.Key("name");
  writer_.String(CleanString(event.name));
  writer_.Key("ts");
  writer_.Double(static_cast<double>(event.timestamp) * tick_scale_);
  writer_.Key("pid");
  writer_.Uint64(event.process_thread.process_koid());
  writer_.Key("tid");
  writer_.Uint64(event.process_thread.thread_koid());

  switch (event.type()) {
    case trace::EventType::kInstant:
      writer_.Key("ph");
      writer_.String("i");
      writer_.Key("s");
      switch (event.data.GetInstant().scope) {
        case trace::EventScope::kGlobal:
          writer_.String("g");
          break;
        case trace::EventScope::kProcess:
          writer_.String("p");
          break;
        case trace::EventScope::kThread:
        default:
          writer_.String("t");
          break;
      }
      break;
    case trace::EventType::kCounter:
      writer_.Key("ph");
      writer_.String("C");
      if (event.data.GetCounter().id) {
        writer_.Key("id");
        writer_.String(fxl::StringPrintf("0x%" PRIx64, event.data.GetCounter().id).c_str());
      }
      break;
    case trace::EventType::kDurationBegin:
      writer_.Key("ph");
      writer_.String("B");
      break;
    case trace::EventType::kDurationEnd:
      writer_.Key("ph");
      writer_.String("E");
      break;
    case trace::EventType::kDurationComplete:
      writer_.Key("ph");
      writer_.String("X");
      writer_.Key("dur");
      writer_.Double(
          static_cast<double>(event.data.GetDurationComplete().end_time - event.timestamp) *
          tick_scale_);
      break;
    case trace::EventType::kAsyncBegin:
      writer_.Key("ph");
      writer_.String("b");
      writer_.Key("id");
      writer_.Uint64(event.data.GetAsyncBegin().id);
      break;
    case trace::EventType::kAsyncInstant:
      writer_.Key("ph");
      writer_.String("n");
      writer_.Key("id");
      writer_.Uint64(event.data.GetAsyncInstant().id);
      break;
    case trace::EventType::kAsyncEnd:
      writer_.Key("ph");
      writer_.String("e");
      writer_.Key("id");
      writer_.Uint64(event.data.GetAsyncEnd().id);
      break;
    case trace::EventType::kFlowBegin:
      writer_.Key("ph");
      writer_.String("s");
      writer_.Key("id");
      writer_.String(std::to_string(event.data.GetFlowBegin().id));
      break;
    case trace::EventType::kFlowStep:
      writer_.Key("ph");
      writer_.String("t");
      writer_.Key("id");
      writer_.String(std::to_string(event.data.GetFlowStep().id));
      break;
    case trace::EventType::kFlowEnd:
      writer_.Key("ph");
      writer_.String("f");
      writer_.Key("bp");
      writer_.String("e");
      writer_.Key("id");
      writer_.String(std::to_string(event.data.GetFlowEnd().id));
      break;
    default:
      break;
  }

  if (event.arguments.size() > 0) {
    writer_.Key("args");
    writer_.StartObject();
    WriteArgs(event.arguments);
    writer_.EndObject();
  }

  writer_.EndObject();
}

void ChromiumExporter::ExportKernelObject(const trace::Record::KernelObject& kernel_object) {
  // The same kernel objects may appear repeatedly within the trace as
  // they are logged by multiple trace providers.  Stash the best quality
  // information to be output at the end of the trace.  In particular, note
  // that the ktrace provider may truncate names, so we try to pick the
  // longest one to preserve.
  switch (kernel_object.object_type) {
    case ZX_OBJ_TYPE_PROCESS: {
      auto it = processes_.find(kernel_object.koid);
      if (it == processes_.end()) {
        processes_.emplace(kernel_object.koid, kernel_object.name);
      } else if (kernel_object.name.size() > it->second.size()) {
        it->second = kernel_object.name;
      }
      break;
    }
    case ZX_OBJ_TYPE_THREAD: {
      const trace::ArgumentValue* process_arg =
          GetArgumentValue(kernel_object.arguments, kProcessArgKey);
      if (!process_arg || process_arg->type() != trace::ArgumentType::kKoid)
        break;
      const zx_koid_t process_koid = process_arg->GetKoid();
      auto process_it = threads_.find(process_koid);
      if (process_it == threads_.end()) {
        process_it = threads_
                         .emplace(std::piecewise_construct, std::forward_as_tuple(process_koid),
                                  std::forward_as_tuple())
                         .first;
      }
      auto& threads = process_it->second;
      auto thread_it = threads.find(kernel_object.koid);
      if (thread_it == threads.end()) {
        threads.emplace(kernel_object.koid, kernel_object.name);
      } else if (kernel_object.name.size() > thread_it->second.size()) {
        thread_it->second = kernel_object.name;
      }
      break;
    }
  }
}

void ChromiumExporter::ExportLog(const trace::Record::Log& log) {
  writer_.StartObject();
  writer_.Key("name");
  writer_.String("log");
  writer_.Key("ph");
  writer_.String("i");
  writer_.Key("ts");
  writer_.Double(static_cast<double>(log.timestamp) * tick_scale_);
  writer_.Key("pid");
  writer_.Uint64(log.process_thread.process_koid());
  writer_.Key("tid");
  writer_.Uint64(log.process_thread.thread_koid());
  writer_.Key("s");
  writer_.String("g");
  writer_.Key("args");
  writer_.StartObject();
  writer_.Key("message");
  writer_.String(CleanString(log.message));
  writer_.EndObject();
  writer_.EndObject();
}

void ChromiumExporter::ExportMetadata(const trace::Record::Metadata& metadata) {
  switch (metadata.type()) {
    case trace::MetadataType::kProviderInfo:
    case trace::MetadataType::kProviderSection:
    case trace::MetadataType::kTraceInfo:
      // These are handled elsewhere.
      break;
    case trace::MetadataType::kProviderEvent: {
      const auto& event = metadata.content.GetProviderEvent();
      const auto& id = event.id;
      if (event.event == trace::ProviderEventType::kBufferOverflow) {
        // TODO(dje): Need to get provider name.
        FX_LOGS(WARNING) << "#" << id << " buffer overflowed,"
                         << " records were likely dropped";
      }
      break;
    }
  }
}

namespace {
template <typename WriteObject>
void WriteSchedulerEvent(WriteObject& writer, const trace::Record::SchedulerEvent& scheduler_event,
                         double tick_scale) {
  switch (scheduler_event.type()) {
    case trace::SchedulerEventType::kLegacyContextSwitch: {
      auto& context_switch = scheduler_event.legacy_context_switch();
      writer.StartObject();
      writer.Key("ph");
      writer.String("k");
      writer.Key("ts");
      writer.Double(static_cast<double>(context_switch.timestamp) * tick_scale);
      writer.Key("cpu");
      writer.Uint(context_switch.cpu_number);
      writer.Key("out");
      writer.StartObject();
      writer.Key("pid");
      writer.Uint64(context_switch.outgoing_thread.process_koid());
      writer.Key("tid");
      writer.Uint64(context_switch.outgoing_thread.thread_koid());
      writer.Key("state");
      writer.Uint(static_cast<uint32_t>(context_switch.outgoing_thread_state));
      writer.Key("prio");
      writer.Uint(static_cast<uint32_t>(context_switch.outgoing_thread_priority));
      writer.EndObject();
      writer.Key("in");
      writer.StartObject();
      writer.Key("pid");
      writer.Uint64(context_switch.incoming_thread.process_koid());
      writer.Key("tid");
      writer.Uint64(context_switch.incoming_thread.thread_koid());
      writer.Key("prio");
      writer.Uint(static_cast<uint32_t>(context_switch.incoming_thread_priority));
      writer.EndObject();
      writer.EndObject();
      break;
    }
    case trace::SchedulerEventType::kContextSwitch: {
      auto& context_switch = scheduler_event.context_switch();
      writer.StartObject();
      writer.Key("ph");
      writer.String("k");
      writer.Key("ts");
      writer.Double(static_cast<double>(context_switch.timestamp) * tick_scale);
      writer.Key("cpu");
      writer.Uint(context_switch.cpu_number);
      writer.Key("out");
      writer.StartObject();
      writer.Key("tid");
      writer.Uint64(context_switch.outgoing_tid);
      writer.Key("state");
      writer.Uint(static_cast<uint32_t>(context_switch.outgoing_thread_state));

      if (const trace::Argument* outgoing_weight =
              trace::Argument::Find("outgoing_weight", context_switch.arguments);
          outgoing_weight != nullptr) {
        const trace::ArgumentValue& value = outgoing_weight->value();
        switch (value.type()) {
          case trace::ArgumentType::kInt32:
            writer.Key("prio");
            writer.Int(value.GetInt32());
            break;
          case trace::ArgumentType::kUint32:
            writer.Key("prio");
            writer.Uint(value.GetUint32());
            break;
          case trace::ArgumentType::kInt64:
            writer.Key("prio");
            writer.Int64(value.GetInt64());
            break;
          case trace::ArgumentType::kUint64:
            writer.Key("prio");
            writer.Uint64(value.GetUint64());
            break;
          default:
            break;
        }
      }

      writer.EndObject();
      writer.Key("in");
      writer.StartObject();
      writer.Key("tid");
      writer.Uint64(context_switch.incoming_tid);

      if (const trace::Argument* incoming_weight =
              trace::Argument::Find("incoming_weight", context_switch.arguments);
          incoming_weight != nullptr) {
        const trace::ArgumentValue& value = incoming_weight->value();
        switch (value.type()) {
          case trace::ArgumentType::kInt32:
            writer.Key("prio");
            writer.Int(value.GetInt32());
            break;
          case trace::ArgumentType::kUint32:
            writer.Key("prio");
            writer.Uint(value.GetUint32());
            break;
          case trace::ArgumentType::kInt64:
            writer.Key("prio");
            writer.Int64(value.GetInt64());
            break;
          case trace::ArgumentType::kUint64:
            writer.Key("prio");
            writer.Uint64(value.GetUint64());
            break;
          default:
            break;
        }
      }

      writer.EndObject();
      writer.EndObject();
      break;
    }
    case trace::SchedulerEventType::kThreadWakeup: {
      auto& thread_wakeup = scheduler_event.thread_wakeup();
      writer.StartObject();
      writer.Key("ph");
      writer.String("w");
      writer.Key("ts");
      writer.Double(static_cast<double>(thread_wakeup.timestamp) * tick_scale);
      writer.Key("cpu");
      writer.Uint(thread_wakeup.cpu_number);
      writer.Key("tid");
      writer.Uint64(thread_wakeup.incoming_tid);

      if (const trace::Argument* weight = trace::Argument::Find("weight", thread_wakeup.arguments);
          weight != nullptr) {
        const trace::ArgumentValue& value = weight->value();
        switch (value.type()) {
          case trace::ArgumentType::kInt32:
            writer.Key("prio");
            writer.Int(value.GetInt32());
            break;
          case trace::ArgumentType::kUint32:
            writer.Key("prio");
            writer.Uint(value.GetUint32());
            break;
          case trace::ArgumentType::kInt64:
            writer.Key("prio");
            writer.Int64(value.GetInt64());
            break;
          case trace::ArgumentType::kUint64:
            writer.Key("prio");
            writer.Uint64(value.GetUint64());
            break;
          default:
            break;
        }
      }

      writer.EndObject();
      break;
    }
  }
}
}  // namespace

void InlineSystemEventsWriter::ExportSchedulerEvent(
    const trace::Record::SchedulerEvent& scheduler_event, double tick_scale) {
  WriteSchedulerEvent(writer_, scheduler_event, tick_scale);
}

void SplitSystemEventsWriter::ExportSchedulerEvent(
    const trace::Record::SchedulerEvent& scheduler_event, double tick_scale) {
  rapidjson::StringBuffer buffer;
  rapidjson::Writer<rapidjson::StringBuffer> writer(buffer);
  WriteSchedulerEvent(writer, scheduler_event, tick_scale);
  fprintf(system_trace_events_fp_, "%s\n", buffer.GetString());
}

void ChromiumExporter::ExportSchedulerEvent(const trace::Record::SchedulerEvent& scheduler_event) {
  system_events_writer_->ExportSchedulerEvent(scheduler_event, tick_scale_);
}

void ChromiumExporter::ExportBlob(const trace::LargeRecordData::Blob& data) {
  if (std::holds_alternative<trace::LargeRecordData::BlobEvent>(data)) {
    const auto& blob = std::get<trace::LargeRecordData::BlobEvent>(data);

    if (blob.category == "fidl:blob") {
      ExportFidlBlob(blob);
      return;
    }

    // Drop blob event record.
    FX_LOGS(INFO) << "Dropping large blob event record: "
                  << "name " << blob.name.c_str() << " of size " << blob.blob_size;
  } else if (std::holds_alternative<trace::LargeRecordData::BlobAttachment>(data)) {
    const auto& blob = std::get<trace::LargeRecordData::BlobAttachment>(data);

    // Drop blob attachment record.
    FX_LOGS(INFO) << "Dropping large blob attachment record: "
                  << "name " << blob.name.c_str() << " of size " << blob.blob_size;
  }
}

void ChromiumExporter::ExportFidlBlob(const trace::LargeRecordData::BlobEvent& blob) {
  writer_.StartObject();
  writer_.Key("ph");
  writer_.String("O");
  writer_.Key("id");
  writer_.String("");
  writer_.Key("cat");
  writer_.String(CleanString(blob.category));
  writer_.Key("name");
  writer_.String(CleanString(blob.name));
  writer_.Key("ts");
  writer_.Double(static_cast<double>(blob.timestamp) * tick_scale_);
  writer_.Key("pid");
  writer_.Uint64(blob.process_thread.process_koid());
  writer_.Key("tid");
  writer_.Uint64(blob.process_thread.thread_koid());
  writer_.Key("blob");
  auto blob_str_base64 =
      Base64Encode(std::string_view(static_cast<const char*>(blob.blob), blob.blob_size));
  writer_.String(blob_str_base64);
  writer_.EndObject();
}

void ChromiumExporter::WriteArgs(const std::vector<trace::Argument>& arguments) {
  for (const auto& arg : arguments) {
    switch (arg.value().type()) {
      case trace::ArgumentType::kBool:
        writer_.Key(CleanString(arg.name()));
        writer_.Bool(arg.value().GetBool());
        break;
      case trace::ArgumentType::kInt32:
        writer_.Key(CleanString(arg.name()));
        writer_.Int(arg.value().GetInt32());
        break;
      case trace::ArgumentType::kUint32:
        writer_.Key(CleanString(arg.name()));
        writer_.Uint(arg.value().GetUint32());
        break;
      case trace::ArgumentType::kInt64:
        writer_.Key(CleanString(arg.name()));
        writer_.Int64(arg.value().GetInt64());
        break;
      case trace::ArgumentType::kUint64:
        writer_.Key(CleanString(arg.name()));
        writer_.Uint64(arg.value().GetUint64());
        break;
      case trace::ArgumentType::kDouble:
        writer_.Key(CleanString(arg.name()));
        writer_.Double(arg.value().GetDouble());
        break;
      case trace::ArgumentType::kString:
        writer_.Key(CleanString(arg.name()));
        writer_.String(CleanString(arg.value().GetString()));
        break;
      case trace::ArgumentType::kPointer:
        writer_.Key(CleanString(arg.name()));
        writer_.String(fxl::StringPrintf("0x%" PRIx64, arg.value().GetPointer()).c_str());
        break;
      case trace::ArgumentType::kKoid:
        writer_.Key(CleanString(arg.name()));
        writer_.String(fxl::StringPrintf("#%" PRIu64, arg.value().GetKoid()).c_str());
        break;
      case trace::ArgumentType::kBlob:
        writer_.Key(CleanString(arg.name()));
        {
          const auto& blob = arg.value().GetBlob();
          writer_.String(Base64Encode(
              std::string_view(reinterpret_cast<const char*>(blob.data()), blob.size())));
        }
        break;
      default:
        break;
    }
  }
}

}  // namespace tracing
