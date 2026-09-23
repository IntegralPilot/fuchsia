// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "../asm-linkage.h"
#include "../dlfcn/dlfcn-abi.h"
#include "src/__support/common.h"
#include "src/link/dl_iterate_phdr.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

using ::testing::Return;

// While each test runs, _dlfcn_phdr_info_counts and _dlfcn_iterate_phdr will
// turn into calls on the mock().
class LibcDlfcnAbiTests : public ::testing::Test {
 public:
  struct Mock {
    MOCK_METHOD(ld::DlPhdrInfoCounts, PhdrInfoCounts, ());
    MOCK_METHOD(int, IteratePhdr, (__dl_iterate_phdr_callback_t, void*));
  };

  static void SetUpTestSuite() {
    ASSERT_EQ(_dlfcn_phdr_info_counts, MockPhdrInfoCounts);
    ASSERT_EQ(_dlfcn_iterate_phdr, MockIteratePhdr);
  }

  void SetUp() override {
    ASSERT_EQ(current_mock_, nullptr);
    current_mock_ = &mock_;
  }

  void TearDown() override {
    EXPECT_EQ(current_mock_, &mock_);
    current_mock_ = nullptr;
  }

  auto& mock() { return mock_; }

 private:
  // These have internal linkage, but use asm-linkage names to get unmangled
  // names to use in the aliases below.
  static decltype(_dlfcn_phdr_info_counts) MockPhdrInfoCounts
      LIBC_ASM_LINKAGE_DECLARE(MockPhdrInfoCounts);
  static decltype(_dlfcn_iterate_phdr) MockIteratePhdr LIBC_ASM_LINKAGE_DECLARE(MockIteratePhdr);

  inline static ::testing::StrictMock<Mock>* current_mock_ = nullptr;
  ::testing::StrictMock<Mock> mock_;
};

ld::DlPhdrInfoCounts LibcDlfcnAbiTests::MockPhdrInfoCounts() {
  return current_mock_->PhdrInfoCounts();
}

int LibcDlfcnAbiTests::MockIteratePhdr(__dl_iterate_phdr_callback_t callback, void* arg) {
  return current_mock_->IteratePhdr(callback, arg);
}

constexpr ld::DlPhdrInfoCounts kFakeCounts = {.adds = 123, .subs = 456};

// Because the global declaration from <link.h> is also in scope, all calls
// here have to be disambiguated with explicit LIBC_NAMESPACE::dl_iterate_phdr.

TEST_F(LibcDlfcnAbiTests, PhdrInfoCounts) {
  EXPECT_CALL(mock(), PhdrInfoCounts()).WillOnce(Return(kFakeCounts));
  EXPECT_EQ(LIBC_NAMESPACE::dl_iterate_phdr(
                [](dl_phdr_info* info, size_t size, void*) -> int {
                  EXPECT_EQ(info->dlpi_adds, kFakeCounts.adds);
                  EXPECT_EQ(info->dlpi_subs, kFakeCounts.subs);
                  return 42;
                },
                nullptr),
            42);
}

TEST_F(LibcDlfcnAbiTests, IteratePhdrBailEarly) {
  EXPECT_CALL(mock(), PhdrInfoCounts());
  // The mock() expects no other calls.
  auto bail_early = [](dl_phdr_info* info, size_t size, void*) { return 42; };
  EXPECT_EQ(LIBC_NAMESPACE::dl_iterate_phdr(bail_early, nullptr), 42);
}

TEST_F(LibcDlfcnAbiTests, IteratePhdr) {
  EXPECT_CALL(mock(), PhdrInfoCounts());
  auto succeed = [](dl_phdr_info* info, size_t size, void*) { return 0; };
  EXPECT_CALL(mock(), IteratePhdr(+succeed, &succeed)).WillOnce(Return(42));
  EXPECT_EQ(LIBC_NAMESPACE::dl_iterate_phdr(+succeed, &succeed), 42);
}

}  // namespace

// These are defined here so calling into LIBC_NAMESPACE::dl_iterate_phdr will
// exercise the defined-symbols case rather than the weak-undefined case that's
// exercised in the dl_iterate_phdr_test.cc tests in the main libc-unittests.
// The assertions in SetUpTestSuite ensure there wasn't some other definition
// linked into this test binary (such as libdl's).
decltype(_dlfcn_phdr_info_counts) _dlfcn_phdr_info_counts
    [[gnu::alias(LIBC_ASM_LINKAGE_STRING(MockPhdrInfoCounts))]];
decltype(_dlfcn_iterate_phdr) _dlfcn_iterate_phdr
    [[gnu::alias(LIBC_ASM_LINKAGE_STRING(MockIteratePhdr))]];

}  // namespace LIBC_NAMESPACE_DECL
