// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE2JSON_TRACE_PARSER_H_
#define SRC_PERFORMANCE_TRACE2JSON_TRACE_PARSER_H_

#include <lib/trace-engine/fields.h>
#include <stdint.h>

#include <array>
#include <filesystem>
#include <memory>
#include <string>
#include <vector>

#include <re2/re2.h>
#include <trace-reader/reader.h>

#include "src/performance/lib/trace_converters/chromium_exporter.h"

namespace tracing {

class FuchsiaTraceParser {
 public:
  explicit FuchsiaTraceParser(const std::filesystem::path& out,
                              const std::filesystem::path& system_event_output_file,
                              const std::vector<std::string>& patterns,
                              const std::vector<std::string>& categories);
  ~FuchsiaTraceParser();

  bool ParseComplete(std::istream*);

 private:
  static constexpr size_t kReadBufferSize = trace::RecordFields::kMaxRecordSizeBytes * 4;
  ChromiumExporter exporter_;
  std::vector<std::unique_ptr<re2::RE2>> patterns_;
  std::vector<std::string> categories_;
  std::array<char, kReadBufferSize> buffer_;
  // The number of bytes of |buffer_| in use.
  size_t buffer_end_ = 0;

  trace::TraceReader reader_;
};

}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE2JSON_TRACE_PARSER_H_
