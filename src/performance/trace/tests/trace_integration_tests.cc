// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/time.h>
#include <zircon/status.h>

#include <gtest/gtest.h>

#include "src/lib/fxl/log_settings_command_line.h"
#include "src/performance/lib/test_utils/run_program.h"
#include "src/performance/trace/tests/integration_test_utils.h"
#include "src/performance/trace/tests/run_test.h"

// Defined in gtest_main.cc
fxl::LogSettings g_log_settings;

namespace tracing::test {

namespace {

// Path to basic integration test binary (for fill-buffer, fill-buffer-and-alert, and simple).
const char kBasicIntegrationTestPath[] = "/pkg/bin/basic_integration_test_app";
// Path to two-providers-two-engines integration test binary.
const char kTwoProvidersTwoEnginesTestPath[] = "/pkg/bin/two_providers_two_engines_test_app";

void RunAndVerify(const std::string& app_path, const std::string& test_name,
                  const std::string& categories, size_t buffer_size_in_mb,
                  const std::string& buffering_mode,
                  std::initializer_list<std::string> additional_arguments = {}) {
  ASSERT_TRUE(RunIntegrationTest(app_path, test_name, categories, buffer_size_in_mb, buffering_mode,
                                 additional_arguments, kRelativeOutputFilePath));
  ASSERT_TRUE(VerifyIntegrationTest(app_path, test_name, buffer_size_in_mb, buffering_mode,
                                    kRelativeOutputFilePath));
}

TEST(Oneshot, FillBuffer) {
  RunAndVerify(kBasicIntegrationTestPath, "fill-buffer", "trace:test", 1, "oneshot");
}

TEST(Circular, FillBuffer) {
  RunAndVerify(kBasicIntegrationTestPath, "fill-buffer", "trace:test", 1, "circular");
}

TEST(CircularWithTrigger, FillBufferAndAlert) {
  RunAndVerify(kBasicIntegrationTestPath, "fill-buffer-and-alert", "trace:test", 1, "circular",
               {"--trigger=alert:stop"});
}

TEST(Streaming, FillBuffer) {
  RunAndVerify(kBasicIntegrationTestPath, "fill-buffer", "trace:test", 1, "streaming");
}

// A class for adding an extra provider to the test.

class ExtraProvider : public ::testing::Test {
 protected:
  // Path of the program that starts two providers using one engine.
  virtual const char* GetProgramPath() = 0;

  const zx::process& provider_process() const { return provider_process_; }

  void SetUp() override {
    zx::job job{};  // -> default job
    argv_.push_back(GetProgramPath());
    AppendLoggingArgs(&argv_, "", g_log_settings);
    zx::eventpair their_event;
    auto status = zx::eventpair::create(0u, &our_event_, &their_event);
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Error creating event pair: " << zx_status_get_string(status);
      return;
    }
    status = SpawnProgram(job, argv_, their_event.release(), &provider_process_);
    if (status != ZX_OK) {
      TearDown();
      return;
    }
    // Wait for the provider to be ready.
    zx_wait_item_t wait_items[2] = {
        {
            .handle = provider_process_.get(),
            .waitfor = ZX_PROCESS_TERMINATED,
            .pending = 0,
        },
        {
            .handle = our_event_.get(),
            .waitfor = ZX_EVENTPAIR_SIGNALED | ZX_EVENTPAIR_PEER_CLOSED,
            .pending = 0,
        },
    };
    // Leave it to the test harness to provide a timeout. If it doesn't that's
    // its bug.
    status = our_event_.wait_many(&wait_items[0], 2, zx::time::infinite());
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed waiting for provider process to start: "
                     << zx_status_get_string(status);
      TearDown();
    }
    FX_LOGS(INFO) << GetProgramPath() << " started";
  }

  void TearDown() override {
    if (provider_process_.is_valid()) {
      our_event_.reset();
      int64_t return_code;
      bool wait_success = WaitAndGetReturnCode(argv_[0], provider_process_, &return_code);
      EXPECT_TRUE(wait_success);
      if (wait_success) {
        EXPECT_EQ(return_code, 0);
      }
      provider_process_.reset();
      FX_LOGS(INFO) << GetProgramPath() << " terminated";
    }
  }

 private:
  zx::eventpair our_event_;
  zx::process provider_process_;
  std::vector<std::string> argv_;
};

TEST(TwoProvidersTwoEngines, Test) {
  RunAndVerify(kTwoProvidersTwoEnginesTestPath, "two-providers-two-engines", "trace:test", 1,
               "oneshot");
}

}  // namespace

}  // namespace tracing::test
