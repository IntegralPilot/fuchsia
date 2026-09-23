// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace2json/convert.h"

#include <lib/syslog/cpp/macros.h>
#include <unistd.h>

#include <cstddef>
#include <fstream>
#include <iostream>
#include <iterator>
#include <vector>

#include <trace-reader/reader.h>

#include "src/performance/trace2json/trace_parser.h"

namespace {

const char kLittleEndianMagicRecord[8] = {0x10, 0x00, 0x04, 0x46, 0x78, 0x54, 0x16, 0x00};

constexpr size_t kMagicSize = std::size(kLittleEndianMagicRecord);
constexpr uint64_t kMagicRecord = 0x0016547846040010;

bool CompareMagic(const char* magic1, const char* magic2) {
  for (size_t i = 0; i < kMagicSize; i++) {
    if (magic1[i] != magic2[i]) {
      return false;
    }
  }
  return true;
}

}  // namespace

bool ConvertTrace(const ConvertSettings& settings) {
  const uint64_t host_magic = kMagicRecord;
  if (!CompareMagic(reinterpret_cast<const char*>(&host_magic), kLittleEndianMagicRecord)) {
    FX_LOGS(ERROR) << "Detected big endian host. Aborting.";
    return false;
  }

  std::ifstream input_file_stream(settings.input_file_name.c_str(),
                                  std::ios_base::in | std::ios_base::binary);
  if (!input_file_stream.is_open()) {
    FX_LOGS(ERROR) << "Error opening input file.";
    return false;
  }
  std::istream* in_stream = &input_file_stream;

  // Look for the magic number record at the start of the trace file and bail
  // before opening (and thus truncating) the output file if we don't find it.
  char initial_bytes[kMagicSize];
  if (in_stream->read(initial_bytes, kMagicSize).gcount() != kMagicSize) {
    FX_LOGS(ERROR) << "Failed to read magic number.";
    return false;
  }
  if (!CompareMagic(initial_bytes, kLittleEndianMagicRecord)) {
    FX_LOGS(ERROR) << "Input file does not start with Fuchsia Trace magic "
                      "number. Aborting.";
    return false;
  }

  tracing::FuchsiaTraceParser parser(settings.output_file_name,
                                     settings.system_event_output_file_name, settings.patterns,
                                     settings.categories);
  return parser.ParseComplete(in_stream);
}
