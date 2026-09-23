// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_LIBFUZZER_RUNNER_H_
#define SRC_SYS_FUZZING_LIBFUZZER_RUNNER_H_

#include <fuchsia/fuzzer/cpp/fidl.h>
#include <lib/fdio/spawn.h>
#include <lib/fit/function.h>
#include <lib/zx/process.h>
#include <zircon/compiler.h>

#include <memory>
#include <string_view>
#include <unordered_set>
#include <vector>

#include <re2/re2.h>

#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/child-process.h"
#include "src/sys/fuzzing/common/input.h"
#include "src/sys/fuzzing/common/runner.h"

namespace fuzzing {

using ::fuchsia::fuzzer::Status;

// The concrete implementation of |Runner| for the libfuzzer engine.
class LibFuzzerRunner : public Runner {
 public:
  ~LibFuzzerRunner() override = default;

  // Factory method.
  static RunnerPtr MakePtr(ExecutorPtr executor);

  void set_verbose(bool verbose) { verbose_ = verbose; }

  // |Runner| methods.
  ZxPromise<> Initialize(std::string pkg_dir, std::vector<std::string> args) override;
  __WARN_UNUSED_RESULT zx_status_t AddToCorpus(CorpusType corpus_type, Input input) override;
  std::vector<Input> GetCorpus(CorpusType corpus_type) override;
  __WARN_UNUSED_RESULT zx_status_t ParseDictionary(const Input& input) override;
  Input GetDictionaryAsInput() const override;

  ZxPromise<Artifact> Fuzz() override;

  ZxPromise<Artifact> TryEach(std::vector<Input> input) override;

  ZxPromise<Artifact> ValidateMinimize(Input input) override;
  ZxPromise<Artifact> Minimize(Artifact artifact) override;

  ZxPromise<Artifact> Cleanse(Input input) override;

  ZxPromise<> ValidateMerge() override;
  ZxPromise<Artifact> Merge() override;

  Status CollectStatus() override;
  ZxPromise<> Stop() override;

 private:
  explicit LibFuzzerRunner(ExecutorPtr executor);

  // Construct a set of libFuzzer command-line arguments for the current options and add them to
  // this object's process.
  __WARN_UNUSED_RESULT zx_status_t AddArgs();

  // Like `TryEach`, but takes filenames of saved inputs that can be passed directly to libFuzzer.
  // If the number of `input_files` is larger than what can be added to the `ChildProcess`, this
  // method will try them in batches.
  ZxPromise<Artifact> TryFiles(std::vector<std::string> input_files);

  // Returns a promise that runs a libFuzzer process asynchronously and returns the fuzzing result
  // and the input that caused it.
  ZxPromise<Artifact> RunAsync();

  // Returns a promise that reads the output of the process run by |RunAsync|. The promise will
  // update the fuzzer status and fuzzing result accordingly.
  ZxPromise<> ParseOutput();
  ZxPromise<> ParseStdout();
  ZxPromise<> ParseStderr();

  // Update the list of input files in the live corpus.
  void ReloadLiveCorpus();

  std::vector<std::string> cmdline_;
  OptionsPtr options_;

  // Immutable set of inputs. These will be kept on merge.
  std::unordered_set<std::string> seed_corpus_;

  // Dynamic set of inputs. Inputs may be added during fuzzing, and/or may be removed when merging.
  std::unordered_set<std::string> live_corpus_;

  bool has_dictionary_ = false;
  zx::time start_;

  // If true, echoes libFuzzer's stderr to this component's stderr.
  bool verbose_ = true;

  // If true along with `verbose_`, echoes both the target's stdout and stderr.
  bool print_all_ = false;

  int64_t pid_ = int64_t(-1);
  FuzzResult fuzz_result_ = FuzzResult::NO_ERRORS;

  Status status_;
  std::string result_input_pathname_;

  // Asynchronous process used to run libFuzzer instances.
  ChildProcess process_;
  Barrier barrier_;
  Workflow workflow_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(LibFuzzerRunner);
};

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_LIBFUZZER_RUNNER_H_
