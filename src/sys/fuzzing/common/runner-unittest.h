// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_RUNNER_UNITTEST_H_
#define SRC_SYS_FUZZING_COMMON_RUNNER_UNITTEST_H_

#include <fuchsia/fuzzer/cpp/fidl.h>
#include <lib/zx/time.h>

#include <memory>
#include <unordered_map>

#include <gtest/gtest.h>

#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/input.h"
#include "src/sys/fuzzing/common/options.h"
#include "src/sys/fuzzing/common/runner.h"
#include "src/sys/fuzzing/common/testing/async-test.h"
#include "src/sys/fuzzing/common/testing/module.h"

namespace fuzzing {

// Just as |Runner| is the base class for specific runner implementations, this class contains
// generic runner unit tests that can be used as the basis for the specific implementations' unit
// tests.
//
// To use these tests for, e.g. a "DerivedRunner" class and a "DerivedRunnerTest" test fixture,
// include code like the following:
//
//   #define RUNNER_TYPE DerivedRunner
//   #define RUNNER_TEST DerivedRunnerTest
//   #include "src/sys/fuzzing/controller/runner-unittest.inc"
//   #undef RUNNER_TEST
//   #undef RUNNER_TYPE
//
class RunnerTest : public AsyncTest {
 protected:
  //////////////////////////////////////
  // Test fixtures.
  void SetUp() override;

  virtual const RunnerPtr& runner() const = 0;

  const OptionsPtr& options() { return options_; }

  // Gets or sets the arguments supplied a component manifest, e.g. the engine's. The exact manifest
  // represented depends on the runner.
  virtual std::vector<std::string> GetParameters() const = 0;
  virtual void SetParameters(std::vector<std::string> parameters) = 0;

  // Adds test-related |options| (e.g. PRNG seed) and configures the |runner|.
  virtual void Configure(const OptionsPtr& options);

  // Tests may set fake coverage to be "produced" during calls to |RunOne| with the given |input|.
  void SetCoverage(const Input& input, Coverage coverage);

  // Tests may provide a |handler| that determines the fuzz result for a given |input|.
  using FuzzResultHandler = fit::function<FuzzResult(const Input&)>;
  void SetFuzzResultHandler(FuzzResultHandler handler);

  // Tests may indicate if all inputs will simulate leaking memory.
  void SetLeak(bool has_leak);

  // These methods correspond to those above, but with a given |hex| string representing the input.
  // Additionally, the |hex| strings may contain "x" as a wildcard. A hex string from an input
  // matches a wildcarded string if they are the same length and every character matches or is "x".
  // In the cases of multiple matches, the longest non-wildcarded prefix wins.
  void SetCoverage(const std::string& hex, Coverage coverage);
  void SetFuzzResult(const std::string& hex, FuzzResult fuzz_result);
  void SetLeak(const std::string& hex, bool has_leak);

  // Fakes the interactions needed with the runner to perform a single fuzzing run.
  Promise<Input> RunOne();

  // Like |RunOne()|, but the given parameters overrides any values set by the corresponding
  // |Set...| methods.
  Promise<Input> RunOne(FuzzResult result);
  Promise<Input> RunOne(Coverage coverage);
  Promise<Input> RunOne(bool leak);

  // Fakes the interactions needed with the runner to perform a sequence of fuzzing runs until the
  // given |promise| completes.
  ZxResult<Artifact> RunUntil(ZxPromise<Artifact> promise);
  ZxResult<> RunUntil(ZxPromise<> promise);
  void RunUntil(Promise<> promise);

  // Returns the test input for the next run. This must not be called unless |HasTestInput| returns
  // true.
  virtual ZxPromise<Input> GetTestInput() = 0;

  // Sets the feedback for the next run.
  virtual ZxPromise<> SetFeedback(Coverage coverage, FuzzResult result, bool leak) = 0;

  void TearDown() override;

  //////////////////////////////////////
  // Unit tests, organized by fuzzing workflow.
  void InitializeCorpus();
  void InitializeDictionary();

  void TryOneNoError();
  void TryOneWithError();
  void TryOneWithLeak();

  void MinimizeNoError();
  void MinimizeEmpty();
  void MinimizeOneByte();
  void MinimizeReduceByTwo();
  void MinimizeNewError();

  void CleanseNoReplacement();
  void CleanseAlreadyClean();
  void CleanseTwoBytes();

  void FuzzUntilError();
  void FuzzUntilRuns();
  void FuzzUntilTime();
  void MergeSeedError();

  // The |Merge| unit test has an extra parameters, |keeps_errors|, which indicates whether the
  // runner includes error-causing inputs in the final corpus. Because of this parameter, this test
  // is not included in runner-unittest.inc and should be added directly, e.g.:
  //
  //   TEST_F(DerivedRunnerTest, Merge) {
  //     MergeSeedError(/* keep_errors= */ true);
  //   }
  //
  void Merge(bool keeps_errors);

  void Stop();

 private:
  // Like |RunOne| above, but allows callers to provide a function to |set_feedback| based on the
  // current |Input|.
  Promise<Input> RunOne(fit::function<ZxPromise<>(const Input&)> set_feedback);

  OptionsPtr options_;
  std::unordered_map<std::string, Coverage> coverage_;
  FuzzResultHandler handler_;
  bool has_leak_ = false;

  // Calls to |RunOne| use this bridge consumer to ensure they run sequentially and in order.
  fpromise::consumer<> previous_run_;

  Scope scope_;
};

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_COMMON_RUNNER_UNITTEST_H_
