// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/runner-unittest.h"

#include <zircon/status.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/sys/fuzzing/common/testing/monitor.h"
#include "src/sys/fuzzing/common/testing/runner.h"

namespace fuzzing {

// |Cleanse| tries to replace bytes with 0x20 or 0xff.
static constexpr size_t kNumReplacements = 2;

// |Merge| uses real OOMs.
static constexpr size_t kSmallOomLimit = 1ULL << 26;  // 64 MB

// Test fixtures.

void RunnerTest::SetUp() {
  AsyncTest::SetUp();
  handler_ = [](const Input& input) { return FuzzResult::NO_ERRORS; };
}

void RunnerTest::Configure(const OptionsPtr& overrides) {
  auto runner = this->runner();
  options_ = runner->options();
  SetOptions(options_.get(), *overrides);
  options_->set_seed(1);
  FUZZING_EXPECT_OK(runner->Configure());
  RunUntilIdle();
}

void RunnerTest::SetCoverage(const Input& input, Coverage coverage) {
  coverage_[input.ToHex()] = std::move(coverage);
}

void RunnerTest::SetFuzzResultHandler(FuzzResultHandler handler) { handler_ = std::move(handler); }

void RunnerTest::SetLeak(bool has_leak) { has_leak_ = has_leak; }

Promise<Input> RunnerTest::RunOne() {
  return RunOne([this](const Input& input) {
    return SetFeedback(Coverage(coverage_[input.ToHex()]), handler_(input), has_leak_);
  });
}

Promise<Input> RunnerTest::RunOne(Coverage coverage) {
  return RunOne([this, coverage = std::move(coverage)](const Input& input) {
    return SetFeedback(std::move(coverage), handler_(input), has_leak_);
  });
}

Promise<Input> RunnerTest::RunOne(FuzzResult fuzz_result) {
  return RunOne([this, fuzz_result](const Input& input) {
    return SetFeedback(Coverage(coverage_[input.ToHex()]), fuzz_result, has_leak_);
  });
}

Promise<Input> RunnerTest::RunOne(bool has_leak) {
  return RunOne([this, has_leak](const Input& input) {
    return SetFeedback(Coverage(coverage_[input.ToHex()]), handler_(input), has_leak);
  });
}

Promise<Input> RunnerTest::RunOne(fit::function<ZxPromise<>(const Input&)> set_feedback) {
  Bridge<> bridge;
  auto task = fpromise::make_promise([]() -> ZxResult<> { return fpromise::ok(); })
                  .and_then(GetTestInput())
                  .and_then([set_feedback = std::move(set_feedback)](Input& input) mutable {
                    return set_feedback(input).and_then([input = std::move(input)]() mutable {
                      return fpromise::ok(std::move(input));
                    });
                  })
                  .or_else([](const zx_status_t& status) {
                    // Target may close before returning test input.
                    EXPECT_EQ(status, ZX_ERR_PEER_CLOSED) << zx_status_get_string(status);
                    return fpromise::error();
                  })
                  .then([completer = std::move(bridge.completer)](Result<Input>& result) mutable {
                    if (result.is_ok()) {
                      completer.complete_ok();
                    } else {
                      completer.complete_error();
                    }
                    return std::move(result);
                  });
  auto consumer = std::move(previous_run_);
  previous_run_ = std::move(bridge.consumer);
  return consumer ? consumer.promise().and_then(std::move(task)).box() : task.box();
}

ZxResult<Artifact> RunnerTest::RunUntil(ZxPromise<Artifact> promise) {
  ZxResult<Artifact> out;
  RunUntil(promise.then([&out](ZxResult<Artifact>& result) { out = std::move(result); }));
  return out;
}

ZxResult<> RunnerTest::RunUntil(ZxPromise<> promise) {
  ZxResult<> out;
  RunUntil(promise.then([&out](ZxResult<>& result) { out = std::move(result); }));
  return out;
}

void RunnerTest::RunUntil(Promise<> promise) {
  Barrier barrier;
  Schedule(promise.wrap_with(barrier));
  auto task =
      fpromise::make_promise([this, run = Future<Input>(), until = Future<>(barrier.sync())](
                                 Context& context) mutable -> Result<> {
        // while (result.is_ok() && !until(context)) {
        while (!until(context)) {
          if (!run) {
            run = RunOne();
          }
          if (!run(context)) {
            return fpromise::pending();
          }
          if (run.is_error()) {
            // Ignore errors; they were checked in |RunOne|.
            return fpromise::ok();
          }
          run = Future<Input>();
        }
        // A call to |RunOne| was dropped, and the |previous_run_|'s completer will not be called.
        previous_run_ = Consumer<>();
        return fpromise::ok();
      }).wrap_with(scope_);
  FUZZING_EXPECT_OK(std::move(task));
  RunUntilIdle();
}

void RunnerTest::TearDown() {
  // Clear temporary files.
  std::vector<std::string> paths;
  if (files::ReadDirContents("/tmp", &paths)) {
    for (const auto& path : paths) {
      files::DeletePath(files::JoinPath("/tmp", path), /* recursive */ true);
    }
  }
  AsyncTest::TearDown();
}

// Unit tests.

void RunnerTest::InitializeCorpus() {
  constexpr const char* kPkgDir = "/tmp/initialize-corpus-test";
  auto runner = this->runner();

  // Only "data/..." directories are considered corpora.
  auto pkg_path = files::JoinPath(kPkgDir, "non-data/corpus");
  ASSERT_TRUE(files::CreateDirectory(pkg_path));
  SetParameters({"non-data/corpus"});
  FUZZING_EXPECT_ERROR(runner->Initialize(kPkgDir, GetParameters()), ZX_ERR_INVALID_ARGS);
  RunUntilIdle();

  // Directory must exist.
  SetParameters({"data/invalid-corpus"});
  FUZZING_EXPECT_ERROR(runner->Initialize(kPkgDir, GetParameters()), ZX_ERR_NOT_FOUND);
  RunUntilIdle();

  // Directory contents are added.
  pkg_path = files::JoinPath(kPkgDir, "data/corpus1");
  std::vector<Input> expected;
  ASSERT_NO_FATAL_FAILURE(MakeCorpus(pkg_path, {"foo", "bar"}, &expected));
  SetParameters({"data/corpus1"});
  FUZZING_EXPECT_OK(runner->Initialize(kPkgDir, GetParameters()));
  RunUntilIdle();
  auto actual = runner->GetCorpus(CorpusType::SEED);
  std::sort(actual.begin(), actual.end());
  EXPECT_EQ(actual, expected);

  // Multiple corpora can be added.
  pkg_path = files::JoinPath(kPkgDir, "data/corpus2");
  ASSERT_NO_FATAL_FAILURE(MakeCorpus(pkg_path, {"baz", "qux", "quux"}, &expected));
  SetParameters({"data/corpus1", "data/corpus2"});
  FUZZING_EXPECT_OK(runner->Initialize(kPkgDir, GetParameters()));
  RunUntilIdle();
  actual = runner->GetCorpus(CorpusType::SEED);
  std::sort(actual.begin(), actual.end());
  EXPECT_EQ(actual, expected);
}

void RunnerTest::InitializeDictionary() {
  constexpr const char* kPkgDir = "/tmp/initialize-dictionary-test";
  auto runner = this->runner();

  // Only "data/..." files are considered dictionaries.
  auto pkg_path = files::JoinPath(kPkgDir, "non-data/some-file");
  ASSERT_NO_FATAL_FAILURE(WriteInput(pkg_path, FakeRunner::valid_dictionary()));
  SetParameters({"non-data/some-file"});
  FUZZING_EXPECT_ERROR(runner->Initialize(kPkgDir, GetParameters()), ZX_ERR_INVALID_ARGS);
  RunUntilIdle();

  // File must exist.
  SetParameters({"data/invalid-dictionary"});
  FUZZING_EXPECT_ERROR(runner->Initialize(kPkgDir, GetParameters()), ZX_ERR_NOT_FOUND);
  RunUntilIdle();

  // Dictionary must have a valid format.
  pkg_path = files::JoinPath(kPkgDir, "data/invalid-dictionary");
  ASSERT_NO_FATAL_FAILURE(WriteInput(pkg_path, FakeRunner::invalid_dictionary()));
  FUZZING_EXPECT_ERROR(runner->Initialize(kPkgDir, GetParameters()), ZX_ERR_INVALID_ARGS);
  RunUntilIdle();

  // Valid.
  pkg_path = files::JoinPath(kPkgDir, "data/dictionary1");
  ASSERT_NO_FATAL_FAILURE(WriteInput(pkg_path, FakeRunner::valid_dictionary()));
  SetParameters({"data/dictionary1"});
  FUZZING_EXPECT_OK(runner->Initialize(kPkgDir, GetParameters()));
  RunUntilIdle();
  EXPECT_EQ(runner->GetDictionaryAsInput(), FakeRunner::valid_dictionary());

  // At most one dictionary is supported.
  pkg_path = files::JoinPath(kPkgDir, "data/dictionary2");
  ASSERT_NO_FATAL_FAILURE(WriteInput(pkg_path, FakeRunner::valid_dictionary()));
  SetParameters({"data/dictionary1", "data/dictionary2"});
  FUZZING_EXPECT_ERROR(runner->Initialize(kPkgDir, GetParameters()), ZX_ERR_INVALID_ARGS);
  RunUntilIdle();
}

void RunnerTest::FuzzUntilError() {
  auto runner = this->runner();
  auto options = MakeOptions();
  options->set_detect_exits(true);
  options->set_mutation_depth(1);
  Configure(options);

  Artifact artifact;
  FUZZING_EXPECT_OK(runner->Fuzz(), &artifact);

  // Add some corpus elements.
  std::vector<Input> inputs;
  inputs.emplace_back("foo");
  inputs.emplace_back("bar");
  inputs.emplace_back("baz");
  inputs.emplace_back("qux");
  auto last = CorpusType::LIVE;
  auto next = CorpusType::SEED;
  for (const auto& input : inputs) {
    EXPECT_EQ(runner->AddToCorpus(next, input.Duplicate()), ZX_OK);
    std::swap(next, last);
  }

  // Set some coverage for the inputs above. Some runners (e.g. libFuzzer) won't mutate an input
  // that lacks any coverage. According to the AFL bucketing scheme used by libFuzzer and others,
  // the counter must be at least 2 to map to a coverage "feature".
  for (size_t i = 0; i < inputs.size(); ++i) {
    SetCoverage(inputs[i], {{i + 1, i + 1}});
  }

  std::vector<Input> actual;
  for (size_t i = 0; i < 100; ++i) {
    FUZZING_EXPECT_OK(RunOne().then([&](Result<Input>& result) {
      if (result.is_ok()) {
        actual.push_back(result.take_value());
      }
    }));
  }

  FUZZING_EXPECT_OK(RunOne(FuzzResult::EXIT));
  RunUntilIdle();

  // Helper lambda to check if the sequence of bytes given by |needle| appears in order, but not
  // necessarily contiguously, in the sequence of bytes given by |haystack|.
  auto contains = [](const Input& haystack, const Input& needle) -> bool {
    const auto* needle_data = needle.data();
    const auto* haystack_data = haystack.data();
    size_t i = 0;
    for (size_t j = 0; i < needle.size() && j < haystack.size(); ++j) {
      if (needle_data[i] == haystack_data[j]) {
        ++i;
      }
    }
    return i == needle.size();
  };

  // Verify that each corpus element is 1) used as-is, and 2) used as the basis for mutations.
  for (auto& orig : inputs) {
    bool exact_match_found = false;
    bool near_match_found = false;
    for (auto& input : actual) {
      if (orig == input) {
        exact_match_found = true;
      } else if (contains(input, orig)) {
        near_match_found = true;
      }
      if (exact_match_found && near_match_found) {
        break;
      }
    }
    EXPECT_TRUE(exact_match_found) << "input: " << orig.ToHex();
    EXPECT_TRUE(near_match_found) << "input: " << orig.ToHex();
  }

  auto fuzz_result = artifact.fuzz_result();
  EXPECT_TRUE(fuzz_result == FuzzResult::EXIT || fuzz_result == FuzzResult::CRASH);
}

void RunnerTest::FuzzUntilRuns() {
  auto runner = this->runner();
  auto options = MakeOptions();
  const size_t kNumRuns = 10;
  options->set_runs(kNumRuns);
  Configure(options);
  std::vector<std::string> expected({""});

  // Subscribe to status updates.
  FakeMonitor monitor(executor());
  runner->AddMonitor(monitor.NewBinding());

  // Fuzz for exactly |kNumRuns|.
  Artifact artifact;
  FUZZING_EXPECT_OK(runner->Fuzz(), &artifact);

  for (size_t i = 0; i < kNumRuns; ++i) {
    FUZZING_EXPECT_OK(RunOne({{i, i}}));
  }

  // Check that we get the expected status updates.
  FUZZING_EXPECT_OK(monitor.AwaitUpdate());
  RunUntilIdle();
  EXPECT_EQ(monitor.reason(), UpdateReason::INIT);
  auto status = monitor.take_status();
  ASSERT_TRUE(status.has_running());
  EXPECT_TRUE(status.running());
  ASSERT_TRUE(status.has_runs());
  auto runs = status.runs();
  ASSERT_TRUE(status.has_elapsed());
  EXPECT_GT(status.elapsed(), 0U);
  auto elapsed = status.elapsed();
  ASSERT_TRUE(status.has_covered_pcs());
  EXPECT_GE(status.covered_pcs(), 0U);
  auto covered_pcs = status.covered_pcs();

  monitor.pop_front();
  FUZZING_EXPECT_OK(monitor.AwaitUpdate());
  RunUntilIdle();
  EXPECT_EQ(size_t(monitor.reason()), size_t(UpdateReason::NEW));
  status = monitor.take_status();
  ASSERT_TRUE(status.has_running());
  EXPECT_TRUE(status.running());
  ASSERT_TRUE(status.has_runs());
  EXPECT_GT(status.runs(), runs);
  runs = status.runs();
  ASSERT_TRUE(status.has_elapsed());
  EXPECT_GT(status.elapsed(), elapsed);
  elapsed = status.elapsed();
  ASSERT_TRUE(status.has_covered_pcs());
  EXPECT_GT(status.covered_pcs(), covered_pcs);
  covered_pcs = status.covered_pcs();

  // Skip others up to DONE.
  while (monitor.reason() != UpdateReason::DONE) {
    monitor.pop_front();
    FUZZING_EXPECT_OK(monitor.AwaitUpdate());
    RunUntilIdle();
  }
  status = monitor.take_status();
  ASSERT_TRUE(status.has_running());
  EXPECT_FALSE(status.running());
  ASSERT_TRUE(status.has_runs());
  EXPECT_GE(status.runs(), runs);
  ASSERT_TRUE(status.has_elapsed());
  EXPECT_GT(status.elapsed(), elapsed);
  ASSERT_TRUE(status.has_covered_pcs());
  EXPECT_GE(status.covered_pcs(), covered_pcs);

  // All done.
  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::NO_ERRORS);
}

void RunnerTest::FuzzUntilTime() {
  auto runner = this->runner();
  // Time is always tricky to test. As a result, this test verifies the bare minimum, namely that
  // the runner exits at least 100 ms after it started. All other verification is performed in more
  // controllable tests, such as |FuzzUntilRuns| above.
  auto options = MakeOptions();
  options->set_max_total_time(zx::msec(100).get());
  Configure(options);
  auto start = zx::clock::get_monotonic();
  auto result = RunUntil(runner->Fuzz());
  ASSERT_TRUE(result.is_ok());
  auto artifact = result.take_value();
  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::NO_ERRORS);
  auto elapsed = zx::clock::get_monotonic() - start;
  EXPECT_GE(elapsed, zx::msec(100));
}

void RunnerTest::TryOneNoError() {
  Configure(MakeOptions());
  Input input({0x01});
  Artifact artifact;
  FUZZING_EXPECT_OK(runner()->TryOne(input.Duplicate()), &artifact);
  FUZZING_EXPECT_OK(RunOne(), std::move(input));
  RunUntilIdle();
  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::NO_ERRORS);
  EXPECT_FALSE(artifact.has_input());
}

void RunnerTest::TryOneWithError() {
  Configure(MakeOptions());
  Input input({0x02});
  Artifact artifact;
  FUZZING_EXPECT_OK(runner()->TryOne(input.Duplicate()), &artifact);
  FUZZING_EXPECT_OK(RunOne(FuzzResult::BAD_MALLOC), std::move(input));
  RunUntilIdle();
  EXPECT_TRUE(artifact.fuzz_result() == FuzzResult::BAD_MALLOC ||
              artifact.fuzz_result() == FuzzResult::OOM);
}

void RunnerTest::TryOneWithLeak() {
  auto options = MakeOptions();
  options->set_detect_leaks(true);
  Configure(options);
  Input input({0x03});
  Artifact artifact;
  // Simulate a suspected leak, followed by an LSan exit. The leak detection heuristics only run
  // full leak detection when a leak is suspected based on mismatched allocations.
  SetLeak(true);
  FUZZING_EXPECT_OK(runner()->TryOne(input.Duplicate()), &artifact);
  FUZZING_EXPECT_OK(RunOne(), input.Duplicate());
  FUZZING_EXPECT_OK(RunOne(FuzzResult::LEAK), std::move(input));
  RunUntilIdle();
  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::LEAK);
}

// Simulate no error on the original input.
void RunnerTest::MinimizeNoError() {
  Configure(MakeOptions());
  Input input({0x04});
  FUZZING_EXPECT_ERROR(runner()->ValidateMinimize(input.Duplicate()), ZX_ERR_INVALID_ARGS);
  FUZZING_EXPECT_OK(RunOne(), std::move(input));
  RunUntilIdle();
}

// Empty input should exit immediately.
void RunnerTest::MinimizeEmpty() {
  Configure(MakeOptions());
  Input input;
  SetFuzzResultHandler([](const Input& input) { return FuzzResult::CRASH; });

  Artifact validated;
  FUZZING_EXPECT_OK(runner()->ValidateMinimize(input.Duplicate()), &validated);
  FUZZING_EXPECT_OK(RunOne(), input.Duplicate());
  RunUntilIdle();
  EXPECT_EQ(validated.fuzz_result(), FuzzResult::CRASH);
  ASSERT_TRUE(validated.has_input());
  EXPECT_EQ(validated.input(), input);

  auto result = RunUntil(runner()->Minimize(std::move(validated)));
  ASSERT_TRUE(result.is_ok());
  auto artifact = result.take_value();
  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::MINIMIZED);
  ASSERT_TRUE(artifact.has_input());
  EXPECT_EQ(artifact.input(), input);
}

// 1-byte input should exit immediately.
void RunnerTest::MinimizeOneByte() {
  Configure(MakeOptions());
  Input input({0x44});
  SetFuzzResultHandler([](const Input& input) { return FuzzResult::CRASH; });

  Artifact validated;
  FUZZING_EXPECT_OK(runner()->ValidateMinimize(input.Duplicate()), &validated);
  FUZZING_EXPECT_OK(RunOne(), input.Duplicate());
  RunUntilIdle();
  EXPECT_EQ(validated.fuzz_result(), FuzzResult::CRASH);
  ASSERT_TRUE(validated.has_input());
  EXPECT_EQ(validated.input(), input);

  auto result = RunUntil(runner()->Minimize(std::move(validated)));
  ASSERT_TRUE(result.is_ok());
  auto minimized = result.take_value();
  EXPECT_EQ(minimized.fuzz_result(), FuzzResult::MINIMIZED);
  ASSERT_TRUE(minimized.has_input());
  EXPECT_EQ(minimized.input(), input);
}

void RunnerTest::MinimizeReduceByTwo() {
  auto options = MakeOptions();
  constexpr size_t kRuns = 0x40;
  options->set_runs(kRuns);
  Configure(options);
  Input input({0x51, 0x52, 0x53, 0x54, 0x55, 0x56});

  Artifact validated;
  FUZZING_EXPECT_OK(runner()->ValidateMinimize(input.Duplicate()), &validated);
  FUZZING_EXPECT_OK(RunOne(FuzzResult::CRASH), input.Duplicate());
  RunUntilIdle();
  EXPECT_EQ(validated.fuzz_result(), FuzzResult::CRASH);
  ASSERT_TRUE(validated.has_input());
  EXPECT_EQ(validated.input(), input);

  // Crash until inputs are smaller than 4 bytes.
  SetFuzzResultHandler([](const Input& input) {
    return input.size() > 3 ? FuzzResult::CRASH : FuzzResult::NO_ERRORS;
  });

  auto result = RunUntil(runner()->Minimize(std::move(validated)));
  ASSERT_TRUE(result.is_ok());
  auto minimized = result.take_value();
  EXPECT_EQ(minimized.fuzz_result(), FuzzResult::MINIMIZED);
  ASSERT_TRUE(minimized.has_input());
  EXPECT_EQ(minimized.input().size(), 4U);
}

void RunnerTest::MinimizeNewError() {
  auto options = MakeOptions();
  options->set_run_limit(zx::msec(500).get());
  Configure(options);
  Input input({0x05, 0x15, 0x25, 0x35});

  Artifact validated;
  FUZZING_EXPECT_OK(runner()->ValidateMinimize(input.Duplicate()), &validated);
  FUZZING_EXPECT_OK(RunOne(FuzzResult::CRASH), input.Duplicate());
  RunUntilIdle();
  EXPECT_EQ(validated.fuzz_result(), FuzzResult::CRASH);
  ASSERT_TRUE(validated.has_input());
  EXPECT_EQ(validated.input(), input);

  // Simulate a crash on the original input, and a timeout on any smaller input.
  SetFuzzResultHandler([](const Input& input) {
    return input.size() > 3 ? FuzzResult::CRASH : FuzzResult::TIMEOUT;
  });

  auto result = RunUntil(runner()->Minimize(std::move(validated)));
  ASSERT_TRUE(result.is_ok());
  auto minimized = result.take_value();
  EXPECT_EQ(minimized.fuzz_result(), FuzzResult::MINIMIZED);
  ASSERT_TRUE(minimized.has_input());
  EXPECT_EQ(minimized.input(), input);
}

void RunnerTest::CleanseNoReplacement() {
  Configure(MakeOptions());
  Input input({0x07, 0x17, 0x27});
  Artifact artifact;
  FUZZING_EXPECT_OK(runner()->Cleanse(input.Duplicate()), &artifact);

  // Simulate no error after cleansing any byte.
  for (size_t i = 0; i < input.size(); ++i) {
    for (size_t j = 0; j < kNumReplacements; ++j) {
      FUZZING_EXPECT_OK(RunOne(FuzzResult::NO_ERRORS));
    }
  }

  RunUntilIdle();
  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::CLEANSED);
  EXPECT_EQ(artifact.input(), input);
}

void RunnerTest::CleanseAlreadyClean() {
  Configure(MakeOptions());
  Input input({' ', 0xff});
  Artifact artifact;
  FUZZING_EXPECT_OK(runner()->Cleanse(input.Duplicate()), &artifact);

  // All bytes match replacements, so this should be done.
  RunUntilIdle();
  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::CLEANSED);
  EXPECT_EQ(artifact.input(), input);
}

void RunnerTest::CleanseTwoBytes() {
  Configure(MakeOptions());

  Input input({0x08, 0x18, 0x28});
  Input inputs[] = {
      {0x20, 0x18, 0x28},  // 1st try.
      {0xff, 0x18, 0x28},  //
      {0x08, 0x20, 0x28},  //
      {0x08, 0xff, 0x28},  //
      {0x08, 0x18, 0x20},  //
      {0x08, 0x18, 0xff},  // Error on 2nd replacement of 3rd byte.
      {0x20, 0x18, 0xff},  // 2nd try. Error on 1st replacement of 1st byte.
      {0x20, 0x20, 0xff},  //
      {0x20, 0xff, 0xff},  //
      {0x20, 0x20, 0xff},  // 3rd try. No errors.
      {0x20, 0xff, 0xff},  //
  };
  SetFuzzResultHandler([](const Input& input) {
    auto hex = input.ToHex();
    return (hex == "081828" || hex == "0818ff" || hex == "2018ff") ? FuzzResult::DEATH
                                                                   : FuzzResult::NO_ERRORS;
  });

  Artifact artifact;
  FUZZING_EXPECT_OK(runner()->Cleanse(std::move(input)), &artifact);
  for (auto& input : inputs) {
    FUZZING_EXPECT_OK(RunOne(), std::move(input));
  }

  RunUntilIdle();
  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::CLEANSED);
  EXPECT_EQ(artifact.input(), Input({0x20, 0x18, 0xff}));
}

void RunnerTest::MergeSeedError() {
  auto runner = this->runner();
  auto options = MakeOptions();
  options->set_oom_limit(kSmallOomLimit);
  Configure(options);

  Input input1({0x0a});
  runner->AddToCorpus(CorpusType::SEED, input1.Duplicate());

  // Triggers error.
  Input input2({0x0b});
  SetFuzzResultHandler([](const Input& input) {
    return input.ToHex() == "0b" ? FuzzResult::OOM : FuzzResult::NO_ERRORS;
  });
  runner->AddToCorpus(CorpusType::SEED, input2.Duplicate());

  Input input3({0x0c, 0x0c});
  runner->AddToCorpus(CorpusType::SEED, input3.Duplicate());

  auto result = RunUntil(runner->ValidateMerge());
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(result.take_error(), ZX_ERR_INVALID_ARGS);
}

void RunnerTest::Merge(bool keeps_errors) {
  auto runner = this->runner();
  auto options = MakeOptions();
  options->set_oom_limit(kSmallOomLimit);
  Configure(options);
  std::vector<std::string> expected_seed;
  std::vector<std::string> expected_live;

  // Seed input => kept.
  Input input1({0x0a});
  SetCoverage(input1, {{0, 1}, {1, 2}, {2, 3}});
  runner->AddToCorpus(CorpusType::SEED, input1.Duplicate());
  expected_seed.push_back(input1.ToHex());

  // Triggers error => maybe kept.
  Input input2({0x0b});
  SetFuzzResultHandler([](const Input& input) {
    return input.ToHex() == "0b" ? FuzzResult::OOM : FuzzResult::NO_ERRORS;
  });
  runner->AddToCorpus(CorpusType::LIVE, input2.Duplicate());
  if (keeps_errors) {
    expected_live.push_back(input2.ToHex());
  }

  // Second-smallest and 2 non-seed features => kept.
  Input input3({0x0c, 0x0c});
  SetCoverage(input3, {{0, 2}, {2, 2}});
  runner->AddToCorpus(CorpusType::LIVE, input3.Duplicate());
  expected_live.push_back(input3.ToHex());

  // Larger and 1 feature not in any smaller inputs => kept.
  Input input4({0x0d, 0x0d, 0x0d});
  SetCoverage(input4, {{0, 2}, {1, 1}});
  runner->AddToCorpus(CorpusType::LIVE, input4.Duplicate());
  expected_live.push_back(input4.ToHex());

  // Second-smallest but only 1 non-seed feature above => skipped.
  Input input5({0x0e, 0x0e});
  SetCoverage(input5, {{0, 2}, {2, 3}});
  runner->AddToCorpus(CorpusType::LIVE, input5.Duplicate());

  // Smallest but features are subset of seed corpus => skipped.
  Input input6({0x0f});
  SetCoverage(input6, {{0, 1}, {2, 3}});
  runner->AddToCorpus(CorpusType::LIVE, input6.Duplicate());

  // Largest with all 3 of the new features => skipped.
  Input input7({0x10, 0x10, 0x10, 0x10});
  SetCoverage(input7, {{0, 2}, {1, 1}, {2, 2}});
  runner->AddToCorpus(CorpusType::LIVE, input7.Duplicate());
  auto result1 = RunUntil(runner->ValidateMerge());
  ASSERT_TRUE(result1.is_ok());

  auto result2 = RunUntil(runner->Merge());
  ASSERT_TRUE(result2.is_ok());
  auto merged = result2.take_value();
  EXPECT_EQ(merged.fuzz_result(), FuzzResult::MERGED);
  EXPECT_FALSE(merged.has_input());

  auto seed_corpus = runner->GetCorpus(CorpusType::SEED);
  std::vector<std::string> actual_seed;
  actual_seed.reserve(seed_corpus.size());
  for (const auto& input : seed_corpus) {
    actual_seed.emplace_back(input.ToHex());
  }
  std::sort(expected_seed.begin(), expected_seed.end());
  std::sort(actual_seed.begin(), actual_seed.end());
  EXPECT_EQ(expected_seed, actual_seed);

  auto live_corpus = runner->GetCorpus(CorpusType::LIVE);
  std::vector<std::string> actual_live;
  actual_live.reserve(live_corpus.size());
  for (const auto& input : live_corpus) {
    actual_live.emplace_back(input.ToHex());
  }
  std::sort(expected_live.begin(), expected_live.end());
  std::sort(actual_live.begin(), actual_live.end());
  EXPECT_EQ(expected_live, actual_live);
}

void RunnerTest::Stop() {
  auto runner = this->runner();
  Configure(MakeOptions());
  Artifact artifact;
  Barrier barrier;
  auto task = runner->Fuzz()
                  .and_then([&artifact](Artifact& result) {
                    artifact = std::move(result);
                    return fpromise::ok();
                  })
                  .or_else([](zx_status_t& status) -> ZxResult<> {
                    // TODO(https://fxbug.dev/42060461): fdio sometimes truncates the fuzzer output when
                    // stopping, and this leads to errors being returned. For the sake of this test,
                    // ignore those errors.
                    if (status != ZX_ERR_IO) {
                      return fpromise::error(status);
                    }
                    return fpromise::ok();
                  })
                  .wrap_with(barrier);
  FUZZING_EXPECT_OK(std::move(task));
  FUZZING_EXPECT_OK(executor()
                        ->MakeDelayedPromise(zx::msec(100))
                        .then([runner](const Result<>& result) { return runner->Stop(); })
                        .then([runner](const ZxResult<>& result) {
                          // Should be idempotent.
                          return runner->Stop();
                        }));
  RunUntil(barrier.sync());
  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::NO_ERRORS);
}

}  // namespace fuzzing
