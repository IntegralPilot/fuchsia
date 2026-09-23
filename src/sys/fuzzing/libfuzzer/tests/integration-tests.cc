// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/tests/integration-tests.h"

namespace fuzzing {

// Test fixtures.

// libFuzzer normally attaches to itself as a debugger to catch crashes; but can be prevented from
// doing so when another debugger like zxdb is needed to investigate failed tests.
#define LIBFUZZER_ALLOW_DEBUG 0

class LibFuzzerTestIntegrationTest : public EngineIntegrationTest {
 protected:
  void SetUp() override {
    EngineIntegrationTest::SetUp();
    options()->set_debug(LIBFUZZER_ALLOW_DEBUG);
    AddArg("bin/libfuzzer_engine");
    AddArg(kFakeFuzzerUrl);
    AddArg("bin/libfuzzer_test_fuzzer");
    AddArg("data/corpus");
  }
};

#undef LIBFUZZER_ALLOW_DEBUG

class LibFuzzerIntegrationTest : public LibFuzzerTestIntegrationTest {
 protected:
  void SetUp() override {
    LibFuzzerTestIntegrationTest::SetUp();
    AddArg(fuchsia::fuzzer::FUZZ_MODE);
  }
};

// Integration tests.

#define ENGINE_INTEGRATION_TEST LibFuzzerIntegrationTest
#include "src/sys/fuzzing/common/tests/fuzzer-integration-tests.inc"
#undef ENGINE_INTEGRATION_TEST

#define ENGINE_INTEGRATION_TEST LibFuzzerTestIntegrationTest
#include "src/sys/fuzzing/common/tests/fuzzer-test-integration-tests.inc"
#undef ENGINE_INTEGRATION_TEST

}  // namespace fuzzing
