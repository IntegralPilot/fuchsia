// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_E2E_TESTS_SCRIPT_TEST_H_
#define SRC_DEVELOPER_DEBUG_E2E_TESTS_SCRIPT_TEST_H_

#include <cstdint>
#include <filesystem>
#include <fstream>
#include <string>
#include <string_view>
#include <utility>

#include "src/developer/debug/e2e_tests/e2e_test.h"
#include "src/developer/debug/zxdb/console/script_runner.h"

namespace zxdb {

class ScriptTest : public E2eTest {
 public:
  explicit ScriptTest(std::string path) : script_path_(std::move(path)) {}

  void TestBody() override;

  // Scan the directory and register all script tests.
  static void RegisterScriptTests();

  void OnTestExited(const std::string& url) override;

 private:
  std::string script_path_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_E2E_TESTS_SCRIPT_TEST_H_
