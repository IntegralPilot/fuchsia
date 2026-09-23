// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_TESTING_RUNNER_H_
#define SRC_SYS_FUZZING_COMMON_TESTING_RUNNER_H_

#include <fuchsia/fuzzer/cpp/fidl.h>
#include <zircon/compiler.h>

#include <vector>

#include "src/lib/fxl/macros.h"
#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/runner.h"

namespace fuzzing {

// Fake command line argument recognized by this runner.
extern const char* kFakeRunnerFlag;

// This class implements |Runner| without actually running anything. For the fuzzing workflows, it
// simply returns whatever results are preloaded by a unit test.
class FakeRunner final : public Runner {
 public:
  ~FakeRunner() override = default;

  // Factory method.
  static RunnerPtr MakePtr(ExecutorPtr executor);

  static Input valid_dictionary() { return Input("key1=\"value\"\n"); }
  static Input invalid_dictionary() { return Input("invalid"); }

  bool has_flag() const { return flag_; }
  const std::vector<Input>& get_inputs() const { return inputs_; }

  void set_validation_error(zx_status_t error) { validation_error_ = error; }
  void set_error(zx_status_t error) { error_ = error; }
  void set_status(Status status) { status_ = std::move(status); }

  const std::vector<Input>& seed_corpus() const { return seed_corpus_; }
  const std::vector<Input>& live_corpus() const { return live_corpus_; }
  void set_seed_corpus(std::vector<Input>&& seed_corpus) { seed_corpus_ = std::move(seed_corpus); }
  void set_live_corpus(std::vector<Input>&& live_corpus) { live_corpus_ = std::move(live_corpus); }

  void set_result(FuzzResult result) { result_ = result; }
  void set_result_input(const Input& input) { result_input_ = input.Duplicate(); }

  // |Runner| methods. Since this runner does not have a "real" fuzzer engine, these use the
  // object's local variables to simulate the responses for the various `fuchsia.fuzzer.Controller`
  // methods, e.g. |TryOne| returns whatever was passed to |set_result|.
  ZxPromise<> Initialize(std::string pkg_dir, std::vector<std::string> args) override;
  __WARN_UNUSED_RESULT zx_status_t AddToCorpus(CorpusType corpus_type, Input input) override;
  std::vector<Input> GetCorpus(CorpusType corpus_type) override;
  __WARN_UNUSED_RESULT zx_status_t ParseDictionary(const Input& input) override;
  Input GetDictionaryAsInput() const override;
  using Runner::UpdateMonitors;

  ZxPromise<Artifact> Fuzz() override;

  ZxPromise<Artifact> TryEach(std::vector<Input> inputs) override;

  ZxPromise<Artifact> ValidateMinimize(Input input) override;
  ZxPromise<Artifact> Minimize(Artifact artifact) override;

  ZxPromise<Artifact> Cleanse(Input input) override;

  ZxPromise<> ValidateMerge() override;
  ZxPromise<Artifact> Merge() override;

  Status CollectStatus() override;

  ZxPromise<> Stop() override;
  Promise<> AwaitStop();

 private:
  explicit FakeRunner(ExecutorPtr executor);
  ZxPromise<Artifact> Run();

  bool flag_ = false;
  zx_status_t validation_error_ = ZX_OK;
  zx_status_t error_ = ZX_OK;
  std::vector<Input> inputs_;
  FuzzResult result_ = FuzzResult::NO_ERRORS;
  Input result_input_;
  Status status_;
  std::vector<Input> seed_corpus_;
  std::vector<Input> live_corpus_;
  Input dictionary_;
  Completer<> completer_;
  Consumer<> consumer_;
  Workflow workflow_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(FakeRunner);
};

// Makes a packaged seed corpus suitable for testing.

// Creates a directory at `pkg_path`. For each input in `inputs`, creates a file under `pkg_path`
// with name and contents matching that input, and adds a corresponding `Input` to `out`. The
// returned inputs are guaranteed to be sorted and unique. This should be called as part of a test
// using `ASSERT_NO_FATAL_FAILURES`, e.g.
//
//  std::vector<Input> corpus;
//  ASSERT_NO_FATAL_FAILURE(MakeCorpus("/tmp/my-test/corpus", {"foo", "bar"}, &corpus));
//
void MakeCorpus(const std::string& pkg_path, std::initializer_list<const char*> inputs,
                std::vector<Input>* out);

// Makes a packaged input file suitable for testing.
//
// Writes `contents` to a file at `pkg_path`, creating any intermediary directories in the
// process. This should be called as part of a test using `ASSERT_NO_FATAL_FAILURES`, e.g.
//
//  ASSERT_NO_FATAL_FAILURE(WriteInput("/tmp/my-test/dictionary", Input("key=\"val\")));
//
void WriteInput(const std::string& pkg_path, Input contents);

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_COMMON_TESTING_RUNNER_H_
