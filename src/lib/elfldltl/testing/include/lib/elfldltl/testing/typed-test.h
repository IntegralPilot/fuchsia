// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_TYPED_TEST_H_
#define SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_TYPED_TEST_H_

#include <lib/elfldltl/layout.h>

#include <gtest/gtest.h>

namespace elfldltl::testing {

using AllFormatsTypedTest = elfldltl::AllFormats<::testing::Types>;
using AllNativeFormatsTypedTest = elfldltl::AllNativeFormats<::testing::Types>;

template <class ElfLayout>
struct FormatTypedTest : public ::testing::Test {
  using Elf = ElfLayout;
  static constexpr std::string_view kName = Elf::kUpperName;
};

struct TestNames {
  template <typename T>
  static std::string GetName(int i) {
    return std::string(T::kName);
  }
};

#define FORMAT_TYPED_TEST_SUITE(Name)                   \
  template <class Elf>                                  \
  using Name = elfldltl::testing::FormatTypedTest<Elf>; \
  TYPED_TEST_SUITE(Name, elfldltl::testing::AllFormatsTypedTest, elfldltl::testing::TestNames)

#define NATIVE_FORMAT_TYPED_TEST_SUITE(Name)            \
  template <class Elf>                                  \
  using Name = elfldltl::testing::FormatTypedTest<Elf>; \
  TYPED_TEST_SUITE(Name, elfldltl::testing::AllNativeFormatsTypedTest, elfldltl::testing::TestNames)

}  // namespace elfldltl::testing

#endif  // SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_TYPED_TEST_H_
