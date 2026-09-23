// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/e2e_tests/script_test.h"

#include <cstdint>
#include <cstdlib>
#include <fstream>
#include <string>
#include <string_view>

#include <gtest/gtest.h>

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/common/host_util.h"
#include "src/lib/fxl/strings/trim.h"

namespace zxdb {

namespace {

constexpr uint64_t kDefaultTimeout = 3;  // in seconds
constexpr std::string_view kBuildType = ZXDB_E2E_TESTS_BUILD_TYPE;

}  // namespace

void ScriptTest::TestBody() {
  std::ifstream script_file(script_path_);
  ASSERT_TRUE(script_file) << "Fail to open " << script_path_;

  // Process directives first.
  uint64_t timeout = kDefaultTimeout;
  std::string line;
  while (std::getline(script_file, line)) {
    if (line.empty())
      continue;
    if (debug::StringStartsWith(line, "##")) {
      std::string directive = std::string(fxl::TrimString(line.substr(2), " "));
      if (debug::StringStartsWith(directive, "require ")) {
        std::string requirement = directive.substr(8);
        if (!debug::StringContains(kBuildType, requirement)) {
          GTEST_SKIP() << "Skipped because of unmet requirement " << requirement;
        }
      } else if (debug::StringStartsWith(directive, "set timeout ")) {
        timeout = std::stoul(directive.substr(12));
      } else {
        GTEST_FAIL() << "Unknown directive: " << directive;
      }
    } else if (debug::StringStartsWith(line, "#")) {
      continue;
    } else {
      break;
    }
  }
  script_file.close();

  // Adjust timeout when running on bots so we're less likely to flake.
  if (std::getenv("BUILDBUCKET_ID")) {
    timeout *= 5;
  }

  ScriptRunner runner(&session(), &console());
  runner.set_timeout_s(timeout);

  bool script_success = false;
  runner.Run(script_path_, [&](bool success) {
    script_success = success;
    loop().QuitNow();
  });

  loop().Run();

  if (!script_success) {
    FAIL() << "Script execution failed: " << script_path_ << "\n"
           << runner.GetFailureContext().AsString();
  }
}

void ScriptTest::OnTestExited(const std::string& url) {
  // Insert a definitive marker for a test component being completed. Scripts that use `run-test`
  // will want to depend on this output so we remain listening for test_runner messages until it has
  // completely shutdown.
  loop().PostTask(FROM_HERE, [this, url]() { console().Output("Test Done: " + url, false); });
}

void ScriptTest::RegisterScriptTests() {
  std::filesystem::path test_scripts_dir =
      (std::filesystem::path(GetSelfPath()).parent_path() / ZXDB_E2E_TESTS_SCRIPTS_DIR)
          .lexically_normal();

  for (const auto& entry : std::filesystem::directory_iterator(test_scripts_dir)) {
    if (entry.path().extension() == ".script") {
      ::testing::RegisterTest("ScriptTest", entry.path().stem().c_str(), nullptr, nullptr,
                              entry.path().c_str(), 0,
                              [=]() { return new ScriptTest(entry.path()); });
    }
  }
}

}  // namespace zxdb
