// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_TEST_TEST_DATA_H_
#define SRC_LIB_ELFLDLTL_TEST_TEST_DATA_H_

#include <stddef.h>

#include <string_view>

extern "C" int Return24();

extern "C" [[gnu::visibility("default")]] const int foo;

extern "C" [[gnu::visibility("default")]] int BasicSymbol();
extern "C" [[gnu::visibility("default")]] int NeedsPlt();
extern "C" [[gnu::visibility("default")]] int NeedsGot();

struct TestData {
  const int* rodata;
  int* data;
  int* bss;
};

constexpr size_t kBiggerThanPageSize = 0x10000;
struct RelroData {
  std::byte before_padding[kBiggerThanPageSize];
  const int* relocated;
  std::byte after_padding[kBiggerThanPageSize - sizeof(int*)];
};

constexpr size_t kSmallDataCount = 2;
constexpr size_t kLargeDataCount = 65538;
constexpr size_t kLargeBssCount = 65536;

constexpr std::string_view kRet24 = "elfldltl-test-ret24.so";
constexpr std::string_view kRelro = "elfldltl-test-relro.so";
constexpr std::string_view kNoXSegment = "elfldltest-no-execute-data-0-bss-1.so";
constexpr std::string_view kNoXSegmentLargeData = "elfldltest-no-execute-data-65536-bss-1.so";
constexpr std::string_view kNoXSegmentLargeBss = "elfldltest-no-execute-data-0-bss-65536.so";
constexpr std::string_view kSymbolic = "elfldltl-test-symbolic.so";

#endif  // SRC_LIB_ELFLDLTL_TEST_TEST_DATA_H_
