// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_TRACE_READER_TEST_READER_TESTS_H_
#define ZIRCON_SYSTEM_ULIB_TRACE_READER_TEST_READER_TESTS_H_

#include <stdint.h>

#include <string_view>
#include <utility>
#include <vector>

#include <trace-reader/reader.h>
#include <zxtest/zxtest.h>

namespace trace {
namespace test {

template <typename T>
uint64_t ToWord(const T& value) {
  return *reinterpret_cast<const uint64_t*>(&value);
}

static inline trace::TraceReader::RecordConsumer MakeRecordConsumer(
    std::vector<trace::Record>* out_records) {
  return [out_records](trace::Record record) { out_records->emplace_back(std::move(record)); };
}

static inline trace::TraceReader::ErrorHandler MakeErrorHandler(std::string_view* out_error) {
  return [out_error](std::string_view error) { *out_error = error; };
}

}  // namespace test
}  // namespace trace

#endif  // ZIRCON_SYSTEM_ULIB_TRACE_READER_TEST_READER_TESTS_H_
