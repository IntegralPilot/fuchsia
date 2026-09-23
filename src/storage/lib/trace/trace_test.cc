// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/trace/trace.h"

#include <zircon/types.h>

#include <cstdint>
#include <string>

#include <fbl/string.h>
#include <gtest/gtest.h>

namespace storage::trace {
namespace {

TEST(StorageTraceTest, TraceDurationCompiles) {
  TRACE_DURATION("category", "name");
  TRACE_DURATION("category", "name", "arg1", 5);
  int trace_only_var = 2;
  TRACE_DURATION("category", "name", "arg1", 1, "arg2", trace_only_var);
}

TEST(StorageTraceTest, TraceDurationBeginCompiles) {
  TRACE_DURATION_BEGIN("category", "name");
  TRACE_DURATION_BEGIN("category", "name", "arg1", 5);
  int trace_only_var = 2;
  TRACE_DURATION_BEGIN("category", "name", "arg1", 1, "arg2", trace_only_var);
}

TEST(StorageTraceTest, TraceDurationEndCompiles) {
  TRACE_DURATION_END("category", "name");
  TRACE_DURATION_END("category", "name", "arg1", 5);
  int trace_only_var = 2;
  TRACE_DURATION_END("category", "name", "arg1", 1, "arg2", trace_only_var);
}

TEST(StorageTraceTest, TraceFlowBeginCompiles) {
  TRACE_FLOW_BEGIN("category", "name", 9);
  TRACE_FLOW_BEGIN("category", "name", 9, "arg1", 5);
  int trace_only_var = 2;
  TRACE_FLOW_BEGIN("category", "name", 9, "arg1", 1, "arg2", trace_only_var);
}

TEST(StorageTraceTest, TraceFlowStepCompiles) {
  TRACE_FLOW_STEP("category", "name", 9);
  TRACE_FLOW_STEP("category", "name", 9, "arg1", 5);
  int trace_only_var = 2;
  TRACE_FLOW_STEP("category", "name", 9, "arg1", 1, "arg2", trace_only_var);
}

TEST(StorageTraceTest, TraceFlowEndCompiles) {
  TRACE_FLOW_END("category", "name", 9);
  TRACE_FLOW_END("category", "name", 9, "arg1", 5);
  int trace_only_var = 2;
  TRACE_FLOW_END("category", "name", 9, "arg1", 1, "arg2", trace_only_var);
}

TEST(StorageTraceTest, GenerateTraceIdIsUnique) {
  uint64_t trace_id1 = storage::trace::GenerateTraceId();
  if (trace_id1 == 0) {
    GTEST_SKIP();
  }
  uint64_t trace_id2 = storage::trace::GenerateTraceId();
  ASSERT_NE(trace_id1, trace_id2);
}

TEST(StorageTraceTest, TraceArgTypesCompile) {
  TRACE_DURATION("category", "name", "int8_t", (int8_t)5);
  TRACE_DURATION("category", "name", "uint8_t", (uint8_t)5);
  TRACE_DURATION("category", "name", "int16_t", (int16_t)5);
  TRACE_DURATION("category", "name", "uint16_t", (uint16_t)5);
  TRACE_DURATION("category", "name", "int32_t", (int32_t)5);
  TRACE_DURATION("category", "name", "uint32_t", (uint32_t)5);
  TRACE_DURATION("category", "name", "int64_t", (int64_t)5);
  TRACE_DURATION("category", "name", "uint64_t", (uint64_t)5);
  TRACE_DURATION("category", "name", "float", (float)5);
  TRACE_DURATION("category", "name", "double", (double)5);
  TRACE_DURATION("category", "name", "bool", true);
  TRACE_DURATION("category", "name", "cstr", "cstr");
  TRACE_DURATION("category", "name", "string", std::string("string"));
  TRACE_DURATION("category", "name", "string_view", std::string("string_view"));
  TRACE_DURATION("category", "name", "fblString", fbl::String("fblString"));
  TRACE_DURATION("category", "name", "nullptr", nullptr);

  enum Enum { kA };
  TRACE_DURATION("category", "name", "enum", Enum::kA);

  zx_handle_t handle = ZX_HANDLE_INVALID;
  TRACE_DURATION("category", "name", "handle", handle);

  zx_koid_t koid = 10;
  TRACE_DURATION("category", "name", "koid", koid);

  void* ptr = nullptr;
  TRACE_DURATION("category", "name", "ptr", ptr);
}

}  // namespace
}  // namespace storage::trace
