// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZXTEST_BASE_JSON_REPORTER_H_
#define ZXTEST_BASE_JSON_REPORTER_H_

#include <memory>
#include <vector>

#include <rapidjson/prettywriter.h>
#include <zxtest/base/log-sink.h>
#include <zxtest/base/observer.h>
#include <zxtest/base/timer.h>

namespace zxtest::internal {

// Opaque type for writing json output.
class JsonWriter;

// LifecycleObserver implementation that writes observed events to a JSON output file.
class JsonReporter final : public LifecycleObserver {
 public:
  explicit JsonReporter(std::unique_ptr<FileLogSink> sink);
  ~JsonReporter();

  void OnProgramStart(const Runner& runner) override;
  void OnTestCaseStart(const TestCase& test_case) override;
  void OnTestStart(const TestCase& test_case, const TestInfo& test) override;
  void OnMessage(const Message& message) override;
  void OnAssertion(const Assertion& assertion) override;
  void OnTestSkip(const TestCase& test_case, const TestInfo& test) override;
  void OnTestFailure(const TestCase& test_case, const TestInfo& test) override;
  void OnTestSuccess(const TestCase& test_case, const TestInfo& test) override;
  void OnTestCaseEnd(const TestCase& test_case) override;
  void OnProgramEnd(const Runner& runner) override;

  void set_list_mode(bool list_mode) { list_mode_ = list_mode; }

 private:
  void ResetSuite() {
    suite_test_count = suite_failure_count = suite_skipped_count = 0;
    timers_.test_suite.Reset();
  }
  void ResetCase() {
    timers_.test.Reset();
    test_messages_.clear();
    test_has_assertion = false;
  }

  bool list_mode_ = false;

  size_t run_test_count = 0;
  size_t run_failure_count = 0;
  size_t run_skipped_count = 0;

  size_t suite_test_count = 0;
  size_t suite_failure_count = 0;
  size_t suite_skipped_count = 0;
  bool suite_entered = false;

  std::vector<std::string> test_messages_;
  bool test_has_assertion = false;
  bool test_entered = false;

  struct {
    internal::Timer program;
    internal::Timer test_suite;
    internal::Timer test;
  } timers_;

  JsonWriter* json_writer_;
};

}  // namespace zxtest::internal

#endif  // ZXTEST_BASE_JSON_REPORTER_H_
