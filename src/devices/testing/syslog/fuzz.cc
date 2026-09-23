// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include <cstdlib>
#include <sstream>

#include <fuzzer/FuzzedDataProvider.h>

// use -f to get printf output from this test.

// Parses an input stream from libFuzzer and executes arbitrary
// logging commands to fuzz the structured logging backend.

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
  FuzzedDataProvider provider(data, size);
  enum class OP : uint8_t {
    kStringField,
    kSignedIntField,
    kUnsignedIntField,
    kDoubleField,
    kMaxValue = kDoubleField,
    kBooleanField,
  };
  auto severity = provider.ConsumeIntegral<fuchsia_logging::RawLogSeverity>();
  // Fatal crashes...
  if (severity == fuchsia_logging::LogSeverity::Fatal) {
    severity = fuchsia_logging::LogSeverity::Error;
  }
  auto file = provider.ConsumeRandomLengthString();
  auto line = provider.ConsumeIntegral<unsigned int>();
  auto msg = provider.ConsumeRandomLengthString();
  auto condition = provider.ConsumeRandomLengthString();
  auto builder = fuchsia_logging::LogBufferBuilder(severity);
  auto buffer = builder.WithCondition(condition).WithMsg(msg).WithFile(file, line).Build();
  while (provider.remaining_bytes()) {
    auto op = provider.ConsumeEnum<OP>();
    auto key = provider.ConsumeRandomLengthString();
    switch (op) {
      case OP::kDoubleField:
        buffer.WriteKeyValue(key, provider.ConsumeFloatingPoint<double>());
        break;
      case OP::kSignedIntField: {
        int64_t value;
        if (provider.remaining_bytes() < sizeof(value)) {
          return 0;
        }
        value = provider.ConsumeIntegral<int64_t>();
        buffer.WriteKeyValue(key, value);
      } break;
      case OP::kUnsignedIntField: {
        uint64_t value;
        if (provider.remaining_bytes() < sizeof(value)) {
          return 0;
        }
        value = provider.ConsumeIntegral<uint64_t>();
        buffer.WriteKeyValue(key, value);
      } break;
      case OP::kStringField: {
        auto value = provider.ConsumeRandomLengthString();
        buffer.WriteKeyValue(key, value);
      } break;
      case OP::kBooleanField: {
        buffer.WriteKeyValue(key, provider.ConsumeBool());
      } break;
    }
  }
  [[maybe_unused]] zx::result<> result = fuchsia_logging::FlushToGlobalLogger(buffer);
  return 0;
}
