// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace2json/trace_parser.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/assert.h>

#include <algorithm>
#include <memory>
#include <utility>

#include <re2/re2.h>

#include "lib/trace-engine/types.h"
#include "src/performance/lib/trace_converters/chromium_exporter.h"

namespace tracing {
namespace {
bool ShouldPassthrough(const trace::Record& record) {
  // Need metadata and init records to have a well-formed trace.
  // Most use cases for trace files require scheduler records.
  // Thread and kernel object records get process and thread names into traces.
  return record.type() == trace::RecordType::kScheduler ||
         record.type() == trace::RecordType::kMetadata ||
         record.type() == trace::RecordType::kInitialization ||
         record.type() == trace::RecordType::kThread ||
         record.type() == trace::RecordType::kKernelObject;
}

bool MatchesAny(const std::vector<std::unique_ptr<re2::RE2>>& patterns, std::string_view string) {
  return std::ranges::any_of(
      patterns, [&](const auto& pattern) { return re2::RE2::FullMatch(string, *pattern); });
}
}  // namespace

FuchsiaTraceParser::FuchsiaTraceParser(const std::filesystem::path& out,
                                       const std::filesystem::path& system_event_output_file,
                                       const std::vector<std::string>& patterns,
                                       const std::vector<std::string>& categories)
    : exporter_(system_event_output_file.empty() ? ChromiumExporter(out)
                                                 : ChromiumExporter(out, system_event_output_file)),
      categories_(categories),
      reader_(
          [this](const trace::Record& record) {
            if (ShouldPassthrough(record)) {
              exporter_.ExportRecord(record);
              return;
            }

            if (patterns_.empty() && categories_.empty()) {
              exporter_.ExportRecord(record);
              return;
            }

            if (!patterns_.empty()) {
              auto name = record.GetName();
              if (name && MatchesAny(patterns_, *name)) {
                exporter_.ExportRecord(record);
                return;
              }
            }

            if (!categories_.empty() && record.type() == trace::RecordType::kEvent) {
              const auto& event = record.GetEvent();
              if (std::ranges::find(categories_, event.category) != categories_.end()) {
                exporter_.ExportRecord(record);
                return;
              }
            }
          },
          [](std::string_view error) { FX_LOGS(ERROR) << error; }) {
  for (const auto& pattern : patterns) {
    auto re = std::make_unique<re2::RE2>(pattern);
    if (!re->ok()) {
      FX_LOGS(ERROR) << "Failed to compile pattern '" << pattern << "': " << re->error();
    }
    patterns_.push_back(std::move(re));
  }
}

FuchsiaTraceParser::~FuchsiaTraceParser() = default;

bool FuchsiaTraceParser::ParseComplete(std::istream* in) {
  // First pass: Read all records except scheduler events.
  ZX_ASSERT(!exporter_.OnSchedulerPass());
  while (true) {
    size_t bytes_read =
        in->read(buffer_.data() + buffer_end_, buffer_.size() - buffer_end_).gcount();
    if (bytes_read == 0) {
      // End of file reached.
      break;
    }
    buffer_end_ += bytes_read;

    size_t words = buffer_end_ / sizeof(uint64_t);
    const uint64_t* data_ptr = reinterpret_cast<const uint64_t*>(buffer_.data());

    trace::Chunk main_chunk(data_ptr, words);
    if (!reader_.ReadRecords(main_chunk)) {
      FX_LOGS(ERROR) << "Error parsing trace";
      return false;
    }

    size_t offset = main_chunk.current_byte_offset();
    memmove(buffer_.data(), buffer_.data() + offset, buffer_end_ - offset);
    buffer_end_ -= offset;
  }

  if (buffer_end_ > 0) {
    FX_LOGS(ERROR) << "Trace file did not end at a record boundary.";
    return false;
  }

  // Second pass: read only scheduler events.
  // The second pass is for scheduler events. These events need to be
  // processed after all other events so that we have a complete view of
  // all the threads that existed during the trace.
  exporter_.StartSchedulerPass();
  in->clear();
  in->seekg(0, std::ios::beg);
  buffer_end_ = 0;

  while (true) {
    size_t bytes_read =
        in->read(buffer_.data() + buffer_end_, buffer_.size() - buffer_end_).gcount();
    if (bytes_read == 0) {
      // End of file reached.
      break;
    }
    buffer_end_ += bytes_read;

    size_t words = buffer_end_ / sizeof(uint64_t);
    const uint64_t* data_ptr = reinterpret_cast<const uint64_t*>(buffer_.data());

    trace::Chunk scheduler_chunk(data_ptr, words);
    if (!reader_.ReadRecords(scheduler_chunk)) {
      FX_LOGS(ERROR) << "Error parsing scheduler event in trace";
      return false;
    }

    size_t offset = scheduler_chunk.current_byte_offset();
    memmove(buffer_.data(), buffer_.data() + offset, buffer_end_ - offset);
    buffer_end_ -= offset;
  }

  if (buffer_end_ > 0) {
    FX_LOGS(ERROR) << "Trace file did not end at a record boundary when parsing scheduler event.";
    return false;
  }

  return true;
}

}  // namespace tracing
