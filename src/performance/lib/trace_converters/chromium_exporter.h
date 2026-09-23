// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_TRACE_CONVERTERS_CHROMIUM_EXPORTER_H_
#define SRC_PERFORMANCE_LIB_TRACE_CONVERTERS_CHROMIUM_EXPORTER_H_

#include <filesystem>
#include <memory>
#include <string>
#include <unordered_map>
#include <vector>

#include <trace-reader/reader.h>

#include "rapidjson/filewritestream.h"
#include "rapidjson/writer.h"

namespace tracing {

class ChromiumExporter {
 public:
  class SystemEventsWriter;

  explicit ChromiumExporter(const std::filesystem::path& out_path);
  ChromiumExporter(const std::filesystem::path& out_path,
                   const std::filesystem::path& system_trace_out_path);
  ~ChromiumExporter();

  void ExportRecord(const trace::Record& record);
  void StartSchedulerPass();
  bool OnSchedulerPass() { return pass_ == Pass::kScheduler; }

 private:
  enum class Pass : uint8_t {
    // First pass: read all records except scheduler events.
    kMain,
    // Second pass: read only scheduler events.
    kScheduler,
  };

  void Start();
  void Stop();
  void ExportEvent(const trace::Record::Event& event);
  void ExportKernelObject(const trace::Record::KernelObject& kernel_object);
  void ExportLog(const trace::Record::Log& log);
  void ExportMetadata(const trace::Record::Metadata& metadata);
  void ExportSchedulerEvent(const trace::Record::SchedulerEvent& scheduler_event);
  void ExportBlob(const trace::LargeRecordData::Blob& data);
  void ExportFidlBlob(const trace::LargeRecordData::BlobEvent& blob);

  // Writes argument data. Assumes it is already within an
  // "args" key object.
  void WriteArgs(const std::vector<trace::Argument>& arguments);

  Pass pass_ = Pass::kMain;
  static constexpr size_t WRITE_BUFFER_SIZE_IN_BYTES = 65536;
  char write_buffer_[WRITE_BUFFER_SIZE_IN_BYTES];
  FILE* fp_;
  rapidjson::FileWriteStream wrapper_;
  rapidjson::Writer<rapidjson::FileWriteStream> writer_;

  std::unique_ptr<SystemEventsWriter> system_events_writer_;

  // Scale factor to get to microseconds.
  // By default ticks are in nanoseconds.
  double tick_scale_ = 0.001;

  std::unordered_map<zx_koid_t, std::string> processes_;
  // Virtual threads mean the same thread id can appear in different processes.
  // Organize threads by process to cope with this.
  std::unordered_map<zx_koid_t /* process id */,
                     std::unordered_map<zx_koid_t /* thread id */, std::string /* thread name */>>
      threads_;
};

}  // namespace tracing

#endif  // SRC_PERFORMANCE_LIB_TRACE_CONVERTERS_CHROMIUM_EXPORTER_H_
