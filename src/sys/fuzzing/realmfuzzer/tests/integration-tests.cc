// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/tests/integration-tests.h"

#include <fuchsia/fuzzer/cpp/fidl.h>

namespace fuzzing {

// Test fixtures.

class RealmFuzzerTestIntegrationTest : public EngineIntegrationTest {
 protected:
  void SetUp() override {
    EngineIntegrationTest::SetUp();
    AddArg("bin/realmfuzzer_engine");
    AddArg(kFakeFuzzerUrl);
    AddArg("data/corpus");
  }
};

class RealmFuzzerIntegrationTest : public RealmFuzzerTestIntegrationTest {
 protected:
  void SetUp() override {
    RealmFuzzerTestIntegrationTest::SetUp();
    AddArg(fuchsia::fuzzer::FUZZ_MODE);
  }
};

// Integration tests.

#define ENGINE_INTEGRATION_TEST RealmFuzzerIntegrationTest
#include "src/sys/fuzzing/common/tests/fuzzer-integration-tests.inc"
#undef ENGINE_INTEGRATION_TEST

#define ENGINE_INTEGRATION_TEST RealmFuzzerTestIntegrationTest
#include "src/sys/fuzzing/common/tests/fuzzer-test-integration-tests.inc"
#undef ENGINE_INTEGRATION_TEST

}  // namespace fuzzing
