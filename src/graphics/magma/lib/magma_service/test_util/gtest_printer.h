// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_TEST_UTIL_GTEST_PRINTER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_TEST_UTIL_GTEST_PRINTER_H_

#include <gtest/gtest.h>

namespace magma {

class GtestPrinter : public testing::EmptyTestEventListener {
  void OnTestStart(const testing::TestInfo& test_info) override;
  void OnTestDisabled(const testing::TestInfo& test_info) override;
  void OnTestPartResult(const testing::TestPartResult& result) override;
  void OnTestEnd(const testing::TestInfo& test_info) override;
  void OnTestIterationEnd(const testing::UnitTest& unit_test, int /*iteration*/) override;
};

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_TEST_UTIL_GTEST_PRINTER_H_
