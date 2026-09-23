// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/controller.h"

#include <zircon/status.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/sys/fuzzing/common/async-socket.h"
#include "src/sys/fuzzing/common/status.h"
#include "src/sys/fuzzing/common/testing/artifact.h"
#include "src/sys/fuzzing/common/testing/async-test.h"
#include "src/sys/fuzzing/common/testing/corpus-reader.h"
#include "src/sys/fuzzing/common/testing/monitor.h"
#include "src/sys/fuzzing/common/testing/runner.h"

namespace fuzzing {
namespace {

using fuchsia::fuzzer::ControllerPtr;
using fuchsia::fuzzer::ControllerSyncPtr;
using fuchsia::fuzzer::UpdateReason;

// Test fixtures.

// Base class for |Controller| unit tests.
class ControllerTest : public AsyncTest {
 protected:
  // Implicitly tests |Controller::SetRunner| and |Controller::Bind|.
  void Bind(fidl::InterfaceRequest<Controller> request) {
    runner_ = FakeRunner::MakePtr(executor());
    controller_ = std::make_unique<ControllerImpl>(runner_);
    controller_->Bind(std::move(request));
  }

  auto runner() const { return std::static_pointer_cast<FakeRunner>(runner_); }

 private:
  std::unique_ptr<ControllerImpl> controller_;
  RunnerPtr runner_;
};

// Unit tests.

TEST_F(ControllerTest, ConfigureAndGetOptions) {
  ControllerPtr controller;
  Bind(controller.NewRequest(dispatcher()));

  // GetOptions without Configure.
  Options options1;
  Bridge<Options> bridge1;
  controller->GetOptions(bridge1.completer.bind());
  FUZZING_EXPECT_OK(ConsumeBridge(bridge1), &options1);
  RunUntilIdle();

  // Configure.
  uint32_t runs = 1000;
  zx::duration max_total_time = zx::sec(300);
  uint32_t max_input_size = 1ULL << 10;
  uint16_t mutation_depth = 8;
  bool detect_exits = true;
  bool detect_leaks = false;
  zx::duration run_limit = zx::sec(20);
  options1.set_runs(runs);
  options1.set_max_total_time(max_total_time.get());
  options1.set_max_input_size(max_input_size);
  options1.set_mutation_depth(mutation_depth);
  options1.set_detect_exits(detect_exits);
  options1.set_detect_leaks(detect_leaks);
  options1.set_run_limit(run_limit.get());
  auto options2 = CopyOptions(options1);
  ZxBridge<> bridge2;
  controller->Configure(std::move(options1), ZxBind<>(std::move(bridge2.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge2));
  RunUntilIdle();

  Bridge<Options> bridge3;
  controller->GetOptions(bridge3.completer.bind());
  FUZZING_EXPECT_OK(ConsumeBridge(bridge3), &options1);
  RunUntilIdle();
  EXPECT_NE(options1.seed(), 0U);

  // Can Configure again.
  uint32_t seed = 42;
  uint64_t malloc_limit = 64ULL << 10;
  uint64_t oom_limit = 1ULL << 20;
  zx::duration purge_interval = zx::sec(10);
  int32_t malloc_exitcode = 1000;
  int32_t death_exitcode = 1001;
  int32_t leak_exitcode = 1002;
  int32_t oom_exitcode = 1003;
  zx::duration pulse_interval = zx::sec(3);
  options2.set_seed(seed);
  options2.set_malloc_limit(malloc_limit);
  options2.set_oom_limit(oom_limit);
  options2.set_purge_interval(purge_interval.get());
  options2.set_malloc_exitcode(malloc_exitcode);
  options2.set_death_exitcode(death_exitcode);
  options2.set_leak_exitcode(leak_exitcode);
  options2.set_oom_exitcode(oom_exitcode);
  options2.set_pulse_interval(pulse_interval.get());
  ZxBridge<> bridge4;
  controller->Configure(std::move(options2), ZxBind<>(std::move(bridge4.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge4));
  RunUntilIdle();

  // Changes are reflected.
  Options options3;
  Bridge<Options> bridge5;
  controller->GetOptions(bridge5.completer.bind());
  FUZZING_EXPECT_OK(ConsumeBridge(bridge5), &options3);
  RunUntilIdle();
  EXPECT_EQ(options3.runs(), runs);
  EXPECT_EQ(options3.max_total_time(), max_total_time.get());
  EXPECT_EQ(options3.seed(), seed);
  EXPECT_EQ(options3.max_input_size(), max_input_size);
  EXPECT_EQ(options3.mutation_depth(), mutation_depth);
  EXPECT_EQ(options3.detect_exits(), detect_exits);
  EXPECT_EQ(options3.detect_leaks(), detect_leaks);
  EXPECT_EQ(options3.run_limit(), run_limit.get());
  EXPECT_EQ(options3.malloc_limit(), malloc_limit);
  EXPECT_EQ(options3.oom_limit(), oom_limit);
  EXPECT_EQ(options3.purge_interval(), purge_interval.get());
  EXPECT_EQ(options3.malloc_exitcode(), malloc_exitcode);
  EXPECT_EQ(options3.death_exitcode(), death_exitcode);
  EXPECT_EQ(options3.leak_exitcode(), leak_exitcode);
  EXPECT_EQ(options3.oom_exitcode(), oom_exitcode);
  EXPECT_EQ(options3.pulse_interval(), pulse_interval.get());
}

TEST_F(ControllerTest, AddToCorpus) {
  ControllerPtr controller;
  Bind(controller.NewRequest(dispatcher()));
  Input input0;
  Input seed_input1({0xde, 0xad});
  Input seed_input2({0xbe, 0xef});
  Input live_input3({0xfe, 0xed});
  Input live_input4({0xfa, 0xce});

  // Interleave the calls.
  ZxBridge<> bridge1, bridge2, bridge3, bridge4;
  controller->AddToCorpus(CorpusType::LIVE, AsyncSocketWrite(executor(), live_input3),
                          ZxBind<>(std::move(bridge3.completer)));
  controller->AddToCorpus(CorpusType::SEED, AsyncSocketWrite(executor(), seed_input1),
                          ZxBind<>(std::move(bridge1.completer)));
  controller->AddToCorpus(CorpusType::SEED, AsyncSocketWrite(executor(), seed_input2),
                          ZxBind<>(std::move(bridge2.completer)));
  controller->AddToCorpus(CorpusType::LIVE, AsyncSocketWrite(executor(), live_input4),
                          ZxBind<>(std::move(bridge4.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge1));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge2));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge3));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge4));
  RunUntilIdle();

  auto seed_corpus = runner()->GetCorpus(CorpusType::SEED);
  auto live_corpus = runner()->GetCorpus(CorpusType::LIVE);

  // Returned corpora are sorted and do not include empty inputs.
  ASSERT_EQ(seed_corpus.size(), 2U);
  EXPECT_EQ(seed_corpus[1].ToHex(), seed_input1.ToHex());
  EXPECT_EQ(seed_corpus[0].ToHex(), seed_input2.ToHex());

  ASSERT_EQ(live_corpus.size(), 2U);
  EXPECT_EQ(live_corpus[1].ToHex(), live_input3.ToHex());
  EXPECT_EQ(live_corpus[0].ToHex(), live_input4.ToHex());
}

TEST_F(ControllerTest, ReadCorpus) {
  ControllerPtr controller;
  Bind(controller.NewRequest(dispatcher()));
  Input input0;
  Input input1({0xde, 0xad});
  Input input2({0xbe, 0xef});
  Input input3({0xfe, 0xed});
  Input input4({0xfa, 0xce});

  EXPECT_EQ(runner()->AddToCorpus(CorpusType::SEED, input1.Duplicate()), ZX_OK);
  EXPECT_EQ(runner()->AddToCorpus(CorpusType::SEED, input2.Duplicate()), ZX_OK);

  EXPECT_EQ(runner()->AddToCorpus(CorpusType::LIVE, input3.Duplicate()), ZX_OK);
  EXPECT_EQ(runner()->AddToCorpus(CorpusType::LIVE, input4.Duplicate()), ZX_OK);

  FakeCorpusReader seed_reader(executor());
  FakeCorpusReader live_reader(executor());
  Bridge<> seed_bridge;
  Bridge<> live_bridge;
  controller->ReadCorpus(CorpusType::SEED, seed_reader.NewBinding(), seed_bridge.completer.bind());
  controller->ReadCorpus(CorpusType::LIVE, live_reader.NewBinding(), live_bridge.completer.bind());
  FUZZING_EXPECT_OK(ConsumeBridge(seed_bridge));
  FUZZING_EXPECT_OK(ConsumeBridge(live_bridge));
  RunUntilIdle();

  // Returned corpora are sorted.
  const auto& seed_corpus = seed_reader.corpus();
  ASSERT_EQ(seed_corpus.size(), 3U);
  EXPECT_EQ(seed_corpus[0], input2);
  EXPECT_EQ(seed_corpus[1], input1);
  EXPECT_EQ(seed_corpus[2], input0);

  const auto& live_corpus = live_reader.corpus();
  ASSERT_EQ(live_corpus.size(), 3U);
  EXPECT_EQ(live_corpus[0], input4);
  EXPECT_EQ(live_corpus[1], input3);
  EXPECT_EQ(live_corpus[2], input0);
}

TEST_F(ControllerTest, WriteDictionary) {
  ControllerPtr controller;
  Bind(controller.NewRequest(dispatcher()));

  ZxBridge<> bridge1;
  controller->WriteDictionary(AsyncSocketWrite(executor(), FakeRunner::invalid_dictionary()),
                              ZxBind<>(std::move(bridge1.completer)));
  FUZZING_EXPECT_ERROR(ConsumeBridge(bridge1), ZX_ERR_INVALID_ARGS);

  ZxBridge<> bridge2;
  controller->WriteDictionary(AsyncSocketWrite(executor(), FakeRunner::valid_dictionary()),
                              ZxBind<>(std::move(bridge2.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge2));

  RunUntilIdle();
}

TEST_F(ControllerTest, ReadDictionary) {
  ControllerPtr controller;
  Bind(controller.NewRequest());

  auto dict = FakeRunner::valid_dictionary();
  EXPECT_EQ(runner()->ParseDictionary(dict), ZX_OK);

  Bridge<FidlInput> bridge;
  controller->ReadDictionary(bridge.completer.bind());
  FUZZING_EXPECT_OK(ConsumeBridge(bridge).and_then([&](FidlInput& fidl_input) {
    return AsyncSocketRead(executor(), std::move(fidl_input));
  }),
                    std::move(dict));
  RunUntilIdle();
}

TEST_F(ControllerTest, AddMonitor) {
  ControllerPtr controller;
  Bind(controller.NewRequest());
  FakeMonitor monitor(executor());

  Status status;
  status.set_runs(13);
  auto expected = CopyStatus(status);
  runner()->set_status(std::move(status));

  Bridge<> bridge;
  controller->AddMonitor(monitor.NewBinding(), bridge.completer.bind());
  FUZZING_EXPECT_OK(ConsumeBridge(bridge));
  RunUntilIdle();

  runner()->UpdateMonitors(UpdateReason::PULSE);
  FUZZING_EXPECT_OK(monitor.AwaitUpdate());
  RunUntilIdle();

  ASSERT_FALSE(monitor.empty());
  EXPECT_EQ(monitor.status().runs(), expected.runs());
  EXPECT_EQ(monitor.reason(), UpdateReason::PULSE);
}

TEST_F(ControllerTest, Fuzz) {
  ControllerPtr controller;
  Bind(controller.NewRequest());

  // First call to |WatchArtifact| should return immediately.
  Artifact actual;
  FUZZING_EXPECT_OK(WatchArtifact(executor(), controller), &actual);
  RunUntilIdle();
  EXPECT_TRUE(actual.is_empty());

  // Errors are reported via |WatchArtifact|.
  FUZZING_EXPECT_ERROR(WatchArtifact(executor(), controller), ZX_ERR_WRONG_TYPE);
  runner()->set_error(ZX_ERR_WRONG_TYPE);
  ZxBridge<> bridge1;
  controller->Fuzz(ZxBind<>(std::move(bridge1.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge1));
  RunUntilIdle();

  // Subsequent calls to |WatchArtifact| should block and return an artifact.
  FUZZING_EXPECT_OK(WatchArtifact(executor(), controller), &actual);
  Artifact expected(FuzzResult::CRASH, {0xde, 0xad, 0xbe, 0xef});
  runner()->set_error(ZX_OK);
  runner()->set_result(expected.fuzz_result());
  runner()->set_result_input(expected.input());
  ZxBridge<> bridge2;
  controller->Fuzz(ZxBind<>(std::move(bridge2.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge2));
  RunUntilIdle();
  ASSERT_FALSE(actual.is_empty());
  EXPECT_EQ(actual, expected);
}

TEST_F(ControllerTest, TryOne) {
  ControllerPtr controller;
  Bind(controller.NewRequest());
  Input input({0xde, 0xad, 0xbe, 0xef});

  // Errors are reported via |WatchArtifact|.
  runner()->set_error(ZX_ERR_WRONG_TYPE);
  ZxBridge<> bridge1;
  controller->TryOne(AsyncSocketWrite(executor(), input), ZxBind<>(std::move(bridge1.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge1));
  RunUntilIdle();

  FUZZING_EXPECT_ERROR(WatchArtifact(executor(), controller), ZX_ERR_WRONG_TYPE);
  RunUntilIdle();

  // Results are retrieved via |WatchArtifact|.
  Artifact actual;
  FUZZING_EXPECT_OK(WatchArtifact(executor(), controller), &actual);
  runner()->set_error(ZX_OK);
  runner()->set_result(FuzzResult::OOM);
  ZxBridge<> bridge2;
  controller->TryOne(AsyncSocketWrite(executor(), input), ZxBind<>(std::move(bridge2.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge2));
  RunUntilIdle();
  ASSERT_FALSE(actual.is_empty());
  EXPECT_EQ(actual.fuzz_result(), FuzzResult::OOM);
  EXPECT_FALSE(actual.has_input());
}

TEST_F(ControllerTest, Minimize) {
  ControllerPtr controller;
  Bind(controller.NewRequest());
  Input input({0xde, 0xad, 0xbe, 0xef});

  // Early errors are returned by the |Minimize| method directly.
  runner()->set_validation_error(ZX_ERR_INVALID_ARGS);
  ZxBridge<> bridge1;
  controller->Minimize(AsyncSocketWrite(executor(), input), ZxBind<>(std::move(bridge1.completer)));
  FUZZING_EXPECT_ERROR(ConsumeBridge(bridge1), ZX_ERR_INVALID_ARGS);
  RunUntilIdle();

  // Late errors are reported via |WatchArtifact|.
  runner()->set_validation_error(ZX_OK);
  runner()->set_error(ZX_ERR_WRONG_TYPE);
  ZxBridge<> bridge2;
  controller->Minimize(AsyncSocketWrite(executor(), input), ZxBind<>(std::move(bridge2.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge2));
  RunUntilIdle();

  FUZZING_EXPECT_ERROR(WatchArtifact(executor(), controller), ZX_ERR_WRONG_TYPE);
  RunUntilIdle();

  // Results are retrieved via |WatchArtifact|.
  Artifact actual;
  FUZZING_EXPECT_OK(WatchArtifact(executor(), controller), &actual);
  Input minimized({0xde, 0xbe});
  runner()->set_error(ZX_OK);
  runner()->set_result_input(minimized);
  ZxBridge<> bridge3;
  controller->Minimize(AsyncSocketWrite(executor(), input), ZxBind<>(std::move(bridge3.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge3));
  RunUntilIdle();
  ASSERT_FALSE(actual.is_empty());
  EXPECT_EQ(actual.fuzz_result(), FuzzResult::MINIMIZED);
  ASSERT_TRUE(actual.has_input());
  EXPECT_EQ(actual.input(), minimized);
}

TEST_F(ControllerTest, Cleanse) {
  ControllerPtr controller;
  Bind(controller.NewRequest());
  Input input({0xde, 0xad, 0xbe, 0xef});

  // Errors are reported via |WatchArtifact|.
  runner()->set_error(ZX_ERR_WRONG_TYPE);
  ZxBridge<> bridge1;
  controller->Cleanse(AsyncSocketWrite(executor(), input), ZxBind<>(std::move(bridge1.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge1));
  RunUntilIdle();

  FUZZING_EXPECT_ERROR(WatchArtifact(executor(), controller), ZX_ERR_WRONG_TYPE);
  RunUntilIdle();

  // Results are retrieved via |WatchArtifact|.
  Artifact actual;
  FUZZING_EXPECT_OK(WatchArtifact(executor(), controller), &actual);
  Input cleansed({0x20, 0x20, 0xbe, 0xff});
  runner()->set_error(ZX_OK);
  runner()->set_result_input(cleansed);
  ZxBridge<> bridge2;
  controller->Cleanse(AsyncSocketWrite(executor(), input), ZxBind<>(std::move(bridge2.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge2));
  RunUntilIdle();
  ASSERT_FALSE(actual.is_empty());
  EXPECT_EQ(actual.fuzz_result(), FuzzResult::CLEANSED);
  ASSERT_TRUE(actual.has_input());
  EXPECT_EQ(actual.input(), cleansed);
}

TEST_F(ControllerTest, Merge) {
  ControllerPtr controller;
  Bind(controller.NewRequest(dispatcher()));

  // Early errors are returned by the |Merge| method directly.
  runner()->set_validation_error(ZX_ERR_INVALID_ARGS);
  ZxBridge<> bridge1;
  controller->Merge(ZxBind<>(std::move(bridge1.completer)));
  FUZZING_EXPECT_ERROR(ConsumeBridge(bridge1), ZX_ERR_INVALID_ARGS);
  RunUntilIdle();

  // Late errors are reported via |WatchArtifact|.
  runner()->set_validation_error(ZX_OK);
  runner()->set_error(ZX_ERR_WRONG_TYPE);
  ZxBridge<> bridge2;
  controller->Merge(ZxBind<>(std::move(bridge2.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge2));
  RunUntilIdle();

  FUZZING_EXPECT_ERROR(WatchArtifact(executor(), controller), ZX_ERR_WRONG_TYPE);
  RunUntilIdle();

  // Results are retrieved via |WatchArtifact|.
  Artifact actual;
  FUZZING_EXPECT_OK(WatchArtifact(executor(), controller), &actual);
  runner()->set_error(ZX_OK);
  ZxBridge<> bridge3;
  controller->Merge(ZxBind<>(std::move(bridge3.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge3));
  RunUntilIdle();
  ASSERT_FALSE(actual.is_empty());
  EXPECT_EQ(actual.fuzz_result(), FuzzResult::MERGED);
}

TEST_F(ControllerTest, GetStatus) {
  ControllerPtr controller;
  Bind(controller.NewRequest(dispatcher()));
  Status result;

  Status status;
  status.set_running(true);
  status.set_runs(42);
  status.set_elapsed(zx::sec(15).get());
  status.set_covered_pcs(5);
  status.set_covered_features(10);
  status.set_corpus_num_inputs(15);
  status.set_corpus_total_size(25);
  auto expected = CopyStatus(status);
  runner()->set_status(std::move(status));

  Bridge<Status> bridge;
  controller->GetStatus(bridge.completer.bind());
  FUZZING_EXPECT_OK(ConsumeBridge(bridge), &result);
  RunUntilIdle();

  EXPECT_EQ(result.running(), expected.running());
  EXPECT_EQ(result.runs(), expected.runs());
  EXPECT_EQ(result.elapsed(), expected.elapsed());
  EXPECT_EQ(result.covered_pcs(), expected.covered_pcs());
  EXPECT_EQ(result.covered_features(), expected.covered_features());
  EXPECT_EQ(result.corpus_num_inputs(), expected.corpus_num_inputs());
  EXPECT_EQ(result.corpus_total_size(), expected.corpus_total_size());
}

}  // namespace
}  // namespace fuzzing
