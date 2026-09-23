// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/engine.h"

#include <gtest/gtest.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/testing/component-context.h"
#include "src/sys/fuzzing/common/testing/registrar.h"
#include "src/sys/fuzzing/common/testing/runner.h"

namespace fuzzing {

// Test fixtures

const char* kTestEngineBin = "/pkg/bin/test-engine";

using ::fuchsia::fuzzer::FUZZ_MODE;

class TestEngine final : public Engine {
 public:
  static TestEngine Create(const std::string& pkg_dir) {
    auto context = ComponentContextForTest::Create();
    auto registrar = std::make_unique<FakeRegistrar>(context->executor());
    auto* context_for_test = static_cast<ComponentContextForTest*>(context.get());
    context_for_test->PutChannel(0, registrar->NewBinding().TakeChannel());
    return TestEngine(std::move(context), std::move(registrar), pkg_dir);
  }

  using Engine::args;
  using Engine::fuzzing;
  using Engine::Initialize;
  using Engine::url;

  const ExecutorPtr& executor() const { return context().executor(); }

  // Converts `args` to `argc`/`argv` before invoking `Engine::Run`.
  zx_status_t Run(const std::vector<std::string>& args, RunnerPtr runner) {
    auto argc = args.size() + 1;
    std::vector<char*> argv;
    argv.reserve(argc);
    argv.push_back(const_cast<char*>(kTestEngineBin));
    for (const auto& arg : args) {
      argv.push_back(const_cast<char*>(arg.c_str()));
    }
    return Engine::Run(static_cast<int>(argc), argv.data(), std::move(runner));
  }

 private:
  TestEngine(ComponentContextPtr context, std::unique_ptr<FakeRegistrar> registrar,
             const std::string& pkg_dir)
      : Engine(std::move(context), pkg_dir), registrar_(std::move(registrar)) {}

  std::unique_ptr<FakeRegistrar> registrar_;
};

class EngineTest : public ::testing::Test {
 protected:
  void TearDown() override {
    // Clear temporary files.
    std::vector<std::string> paths;
    if (files::ReadDirContents("/tmp", &paths)) {
      for (const auto& path : paths) {
        files::DeletePath(files::JoinPath("/tmp", path), /* recursive */ true);
      }
    }
    ::testing::Test::TearDown();
  }
};

// Unit tests

TEST_F(EngineTest, InitializeUrl) {
  auto engine = TestEngine::Create("/tmp/initialize-url-test");

  // URL is required.
  std::vector<std::string> args;
  EXPECT_EQ(engine.Initialize(std::move(args)), ZX_ERR_INVALID_ARGS);

  // Other arguments are optional.
  args = std::vector<std::string>({kFakeFuzzerUrl});
  EXPECT_EQ(engine.Initialize(std::move(args)), ZX_OK);
  EXPECT_EQ(engine.url(), kFakeFuzzerUrl);
  EXPECT_FALSE(engine.fuzzing());
  EXPECT_TRUE(engine.args().empty());
}

TEST_F(EngineTest, InitializeFlags) {
  auto engine = TestEngine::Create("/tmp/initialize-flags-test");

  // `fuchsia.fuzzer.FUZZ_MODE` flag.
  std::vector<std::string> args({kFakeFuzzerUrl, FUZZ_MODE});
  EXPECT_EQ(engine.Initialize(std::move(args)), ZX_OK);
  EXPECT_TRUE(engine.fuzzing());
  EXPECT_TRUE(engine.args().empty());

  // Other flags are passed through.
  args = std::vector<std::string>({"-libfuzzer=flag", kFakeFuzzerUrl, "--other"});
  EXPECT_EQ(engine.Initialize(std::move(args)), ZX_OK);
  EXPECT_FALSE(engine.fuzzing());
  EXPECT_EQ(engine.args(), std::vector<std::string>({"-libfuzzer=flag", "--other"}));

  // Order is flexible.
  args = std::vector<std::string>({FUZZ_MODE, kFakeFuzzerUrl, "-libfuzzer=flag"});
  EXPECT_EQ(engine.Initialize(std::move(args)), ZX_OK);
  EXPECT_TRUE(engine.fuzzing());
  EXPECT_EQ(engine.args(), std::vector<std::string>({"-libfuzzer=flag"}));

  // '--' preserves following arguments.
  args = std::vector<std::string>({kFakeFuzzerUrl, "--", "-libfuzzer=flag", "--", FUZZ_MODE});
  EXPECT_EQ(engine.Initialize(std::move(args)), ZX_OK);
  EXPECT_FALSE(engine.fuzzing());
  EXPECT_EQ(engine.args(), std::vector<std::string>({"--", "-libfuzzer=flag", "--", FUZZ_MODE}));
}

TEST_F(EngineTest, RunFuzzer) {
  constexpr const char* kPkgDir = "/tmp/run-fuzzer-test";
  auto engine = TestEngine::Create(kPkgDir);
  std::vector<std::string> args({
      kFakeFuzzerUrl,
      "data/corpus1",
      "data/corpus2",
      "data/dictionary",
      FUZZ_MODE,
      kFakeRunnerFlag,
  });
  std::vector<Input> expected;

  auto pkg_path = files::JoinPath(kPkgDir, args[1]);
  ASSERT_NO_FATAL_FAILURE(MakeCorpus(pkg_path, {"foo", "bar"}, &expected));

  pkg_path = files::JoinPath(kPkgDir, args[2]);
  ASSERT_NO_FATAL_FAILURE(MakeCorpus(pkg_path, {"baz", "qux", "quux"}, &expected));

  pkg_path = files::JoinPath(kPkgDir, args[3]);
  ASSERT_NO_FATAL_FAILURE(WriteInput(pkg_path, FakeRunner::valid_dictionary()));

  auto runner = FakeRunner::MakePtr(engine.executor());
  auto fake_runner = std::static_pointer_cast<FakeRunner>(runner);
  EXPECT_EQ(engine.Run(args, runner), ZX_OK);
  EXPECT_EQ(engine.url(), kFakeFuzzerUrl);
  EXPECT_TRUE(engine.fuzzing());
  EXPECT_TRUE(fake_runner->has_flag());

  // `Runner::GetCorpus` returns sorted inputs.
  EXPECT_EQ(runner->GetCorpus(CorpusType::SEED), expected);
}

TEST_F(EngineTest, RunTest) {
  constexpr const char* kPkgDir = "/tmp/run-test-test";
  auto engine = TestEngine::Create(kPkgDir);
  std::vector<std::string> args({
      kFakeFuzzerUrl,
      "data/corpus1",
      "data/corpus2",
      "data/dictionary",
      kFakeRunnerFlag,
  });
  std::vector<Input> expected;

  auto pkg_path = files::JoinPath(kPkgDir, args[1]);
  ASSERT_NO_FATAL_FAILURE(MakeCorpus(pkg_path, {"foo", "bar"}, &expected));

  pkg_path = files::JoinPath(kPkgDir, args[2]);
  ASSERT_NO_FATAL_FAILURE(MakeCorpus(pkg_path, {"baz", "qux", "quux"}, &expected));

  pkg_path = files::JoinPath(kPkgDir, args[3]);
  ASSERT_NO_FATAL_FAILURE(WriteInput(pkg_path, FakeRunner::valid_dictionary()));

  auto runner = FakeRunner::MakePtr(engine.executor());
  auto fake_runner = std::static_pointer_cast<FakeRunner>(runner);
  EXPECT_EQ(engine.Run(args, runner), ZX_OK);
  EXPECT_EQ(engine.url(), kFakeFuzzerUrl);
  EXPECT_FALSE(engine.fuzzing());
  EXPECT_TRUE(fake_runner->has_flag());

  // `FakeRunner::get_inputs` does not sort the inputs it returns.
  std::vector<Input> actual;
  for (const auto& input : fake_runner->get_inputs()) {
    if (input.size() != 0) {
      actual.push_back(input.Duplicate());
    }
  }
  std::sort(actual.begin(), actual.end());
  EXPECT_EQ(actual, expected);
}

}  // namespace fuzzing
