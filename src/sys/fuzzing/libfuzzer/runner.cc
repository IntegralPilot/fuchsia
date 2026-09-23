// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/libfuzzer/runner.h"

#include <lib/fdio/spawn.h>
#include <lib/fit/defer.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/syslog/cpp/macros.h>
#include <stddef.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls/object.h>
#include <zircon/time.h>

#include <filesystem>
#include <iostream>
#include <sstream>

#include <openssl/sha.h>
#include <re2/re2.h>

#include "absl/strings/string_view.h"
#include "src/lib/files/directory.h"
#include "src/lib/files/eintr_wrapper.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/sys/fuzzing/common/dictionary.h"
#include "src/sys/fuzzing/common/status.h"
#include "src/sys/fuzzing/libfuzzer/stats.h"

namespace fuzzing {
namespace {

using fuchsia::fuzzer::ProcessStats;

const char* kTestInputPath = "/tmp/test_input";
const char* kLiveCorpusPath = "/tmp/live_corpus";
const char* kSeedCorpusPath = "/tmp/seed_corpus";
const char* kTempCorpusPath = "/tmp/temp_corpus";
const char* kDictionaryPath = "/tmp/dictionary";
const char* kResultInputPath = "/tmp/result_input";

// See libFuzzer's |fuzzer::FuzzingOptions|.
constexpr int64_t kLibFuzzerNoErrorExitCode = 0;
constexpr int64_t kLibFuzzerTimeoutExitCode = 70;
constexpr int64_t kLibFuzzerOOMExitCode = 71;

// Returns |one| if |original| is non-zero and less than |one|, otherwise returns |original|.
template <typename T>
T Clamp(T original, const T& one, const char* type, const char* unit, const char* flag) {
  if (!original) {
    return 0;
  }
  if (original < one) {
    FX_LOGS(WARNING) << "libFuzzer does not support " << type << "s of less than 1 " << unit
                     << " for '" << flag << "'.";
    return one;
  }
  return original;
}

// Converts a flag into a libFuzzer command line argument.
template <typename T>
std::string MakeArg(const std::string& flag, T value) {
  std::ostringstream oss;
  oss << "-" << flag << "=" << value;
  return oss.str();
}

void CreateDirectory(const char* pathname) {
  if (!files::CreateDirectory(pathname)) {
    FX_LOGS(FATAL) << "Failed to create '" << pathname << "': " << strerror(errno);
  }
}

// Reads a byte sequence from a file.
Input ReadInputFromFile(const std::string& pathname) {
  std::vector<uint8_t> data;
  if (!files::ReadFileToVector(pathname, &data)) {
    FX_LOGS(FATAL) << "Failed to read input from '" << pathname << "': " << strerror(errno);
  }
  return Input(data);
}

// Writes a byte sequence to a file.
void WriteInputToFile(const Input& input, const std::string& pathname) {
  FX_DCHECK(input.size() < std::numeric_limits<ssize_t>::max());
  const auto* data = reinterpret_cast<const char*>(input.data());
  auto size = static_cast<ssize_t>(input.size());
  if (!files::WriteFile(pathname, data, size)) {
    FX_LOGS(FATAL) << "Failed to write input to '" << pathname << "': " << strerror(errno);
  }
}

std::string MakeFilename(const Input& input) {
  uint8_t digest[SHA_DIGEST_LENGTH];
  SHA1(input.data(), input.size(), digest);
  std::ostringstream filename;
  filename << std::hex;
  for (size_t i = 0; i < SHA_DIGEST_LENGTH; ++i) {
    filename << std::setw(2) << std::setfill('0') << size_t(digest[i]);
  }
  return filename.str();
}

}  // namespace

RunnerPtr LibFuzzerRunner::MakePtr(ExecutorPtr executor) {
  return RunnerPtr(new LibFuzzerRunner(std::move(executor)));
}

LibFuzzerRunner::LibFuzzerRunner(ExecutorPtr executor)
    : Runner(executor), process_(executor), workflow_(this) {
  options_ = Runner::options();
  options_->set_detect_exits(true);
  CreateDirectory(kSeedCorpusPath);
  CreateDirectory(kLiveCorpusPath);
}

ZxPromise<> LibFuzzerRunner::Initialize(std::string pkg_dir, std::vector<std::string> args) {
  // First argument is the subprocess name.
  if (args.empty()) {
    return fpromise::make_promise(
        []() -> ZxResult<> { return fpromise::error(ZX_ERR_INVALID_ARGS); });
  }
  auto args_begin = args.begin();
  cmdline_.push_back(*args_begin++);

  // Rearrange args before '--' so that positional args come before flags. Keep the positional args
  // intact, and copy and erase the rest of the command line.
  auto args_end = std::find(args_begin, args.end(), "--");
  auto positional_end =
      std::partition(args_begin, args_end, [](const std::string& arg) { return arg[0] != '-'; });
  std::copy(positional_end, args.end(), std::back_inserter(cmdline_));
  args = std::vector<std::string>(args_begin, positional_end);

  return Runner::Initialize(std::move(pkg_dir), std::move(args));
}

///////////////////////////////////////////////////////////////
// Corpus-related methods.

zx_status_t LibFuzzerRunner::AddToCorpus(CorpusType corpus_type, Input input) {
  auto filename = MakeFilename(input);
  switch (corpus_type) {
    case CorpusType::SEED:
      if (auto [iter, inserted] = seed_corpus_.insert(filename); inserted) {
        WriteInputToFile(input, files::JoinPath(kSeedCorpusPath, filename));
      }
      break;
    case CorpusType::LIVE:
      if (auto [iter, inserted] = live_corpus_.insert(filename); inserted) {
        WriteInputToFile(input, files::JoinPath(kLiveCorpusPath, filename));
      }
      break;
    default:
      return ZX_ERR_INVALID_ARGS;
  }
  return ZX_OK;
}

std::vector<Input> LibFuzzerRunner::GetCorpus(CorpusType corpus_type) {
  std::vector<Input> inputs;
  switch (corpus_type) {
    case CorpusType::SEED:
      inputs.reserve(seed_corpus_.size());
      for (const auto& filename : seed_corpus_) {
        inputs.emplace_back(ReadInputFromFile(files::JoinPath(kSeedCorpusPath, filename)));
      }
      break;
    case CorpusType::LIVE:
      inputs.reserve(live_corpus_.size());
      for (const auto& filename : live_corpus_) {
        inputs.emplace_back(ReadInputFromFile(files::JoinPath(kLiveCorpusPath, filename)));
      }
      break;
    default:
      FX_NOTIMPLEMENTED();
  }
  return inputs;
}

void LibFuzzerRunner::ReloadLiveCorpus() {
  live_corpus_.clear();
  std::unordered_set<std::string> dups;
  for (const auto& dir_entry : std::filesystem::directory_iterator(kLiveCorpusPath)) {
    auto filename = dir_entry.path().filename().string();
    if (files::IsFile(files::JoinPath(kSeedCorpusPath, filename))) {
      dups.insert(files::JoinPath(kLiveCorpusPath, filename));
    } else {
      live_corpus_.insert(std::move(filename));
    }
  }
  for (const auto& dup_path : dups) {
    std::filesystem::remove(dup_path);
  }
}

///////////////////////////////////////////////////////////////
// Dictionary-related methods.

zx_status_t LibFuzzerRunner::ParseDictionary(const Input& input) {
  Dictionary dict;
  dict.Configure(options());
  if (!dict.Parse(input)) {
    return ZX_ERR_INVALID_ARGS;
  }
  WriteInputToFile(input, kDictionaryPath);
  has_dictionary_ = true;
  return ZX_OK;
}

Input LibFuzzerRunner::GetDictionaryAsInput() const {
  return has_dictionary_ ? ReadInputFromFile(kDictionaryPath) : Input();
}

///////////////////////////////////////////////////////////////
// Fuzzing workflows.

ZxPromise<Artifact> LibFuzzerRunner::Fuzz() {
  return fpromise::make_promise([this]() -> ZxResult<> {
           if (auto status = AddArgs(); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg(kLiveCorpusPath); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg(kSeedCorpusPath); status != ZX_OK) {
             return fpromise::error(status);
           }
           return fpromise::ok();
         })
      .and_then(RunAsync())
      .and_then([this](Artifact& artifact) {
        ReloadLiveCorpus();
        return fpromise::ok(std::move(artifact));
      })
      .wrap_with(workflow_);
}

ZxPromise<Artifact> LibFuzzerRunner::TryEach(std::vector<Input> inputs) {
  // Write the inputs out to storage for libFuzzer.
  std::filesystem::remove_all(kTempCorpusPath);
  CreateDirectory(kTempCorpusPath);
  std::vector<std::string> input_files;
  input_files.reserve(inputs.size());
  for (auto& input : inputs) {
    auto input_file = files::JoinPath(kTempCorpusPath, MakeFilename(input));
    WriteInputToFile(input, input_file);
    input_files.emplace_back(std::move(input_file));
  }
  inputs.clear();
  return TryFiles(std::move(input_files)).wrap_with(workflow_);
}

ZxPromise<Artifact> LibFuzzerRunner::TryFiles(std::vector<std::string> input_files) {
  print_all_ = true;
  return fpromise::make_promise(
      [this, fut = ZxFuture<Artifact>(),
       input_files = std::move(input_files)](Context& context) mutable -> ZxResult<Artifact> {
        // Large corpora can result in a command line that is too big for `fdio_spawn`. Try
        // inputs in batches.
        while (true) {
          if (!fut) {
            if (input_files.empty()) {
              return fpromise::ok(Artifact(FuzzResult::NO_ERRORS));
            }
            if (auto status = AddArgs(); status != ZX_OK) {
              FX_LOGS(WARNING) << "Failed to add libFuzzer command line flags.";
              return fpromise::error(status);
            }
            std::vector<std::string> deferred;
            zx_status_t status = ZX_OK;
            for (auto& input_file : input_files) {
              if (status == ZX_OK) {
                status = process_.AddArg(input_file);
              }
              if (status != ZX_OK) {
                deferred.emplace_back(std::move(input_file));
              }
            }
            if (!deferred.empty()) {
              FX_LOGS(INFO) << "Will try " << deferred.size()
                            << " remaining input(s) in a subsequent attempt.";
            }
            input_files = std::move(deferred);
            fut = RunAsync();
          }
          if (!fut(context)) {
            return fpromise::pending();
          }
          if (fut.is_error()) {
            return fpromise::error(fut.take_error());
          }
          auto artifact = fut.take_value();
          if (artifact.fuzz_result() != FuzzResult::NO_ERRORS) {
            return fpromise::ok(std::move(artifact));
          }
        }
      });
}

ZxPromise<Artifact> LibFuzzerRunner::ValidateMinimize(Input input) {
  WriteInputToFile(input, kTestInputPath);
  return TryFiles(std::vector<std::string>({kTestInputPath}))
      .and_then([](Artifact& artifact) -> ZxResult<Artifact> {
        if (artifact.fuzz_result() == FuzzResult::NO_ERRORS) {
          FX_LOGS(WARNING) << "Test input did not trigger an error.";
          return fpromise::error(ZX_ERR_INVALID_ARGS);
        }
        return fpromise::ok(std::move(artifact));
      })
      .wrap_with(workflow_, Workflow::Mode::kStart);
}

ZxPromise<Artifact> LibFuzzerRunner::Minimize(Artifact artifact) {
  FX_DCHECK(artifact.has_input());
  WriteInputToFile(artifact.input(), kTestInputPath);
  return fpromise::make_promise([this]() -> ZxResult<> {
           if (auto status = AddArgs(); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg("-minimize_crash=1"); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg(kTestInputPath); status != ZX_OK) {
             return fpromise::error(status);
           }
           return fpromise::ok();
         })
      .and_then(RunAsync())
      .and_then([](Artifact& artifact) -> ZxResult<Artifact> {
        return fpromise::ok(Artifact(FuzzResult::MINIMIZED, artifact.take_input()));
      })
      .wrap_with(workflow_, Workflow::Mode::kFinish);
}

ZxPromise<Artifact> LibFuzzerRunner::Cleanse(Input input) {
  WriteInputToFile(input, kTestInputPath);
  return fpromise::make_promise([this]() -> ZxResult<> {
           if (auto status = AddArgs(); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg("-cleanse_crash=1"); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg(kTestInputPath); status != ZX_OK) {
             return fpromise::error(status);
           }
           return fpromise::ok();
         })
      .and_then(RunAsync())
      .and_then([input = std::move(input)](Artifact& artifact) mutable {
        // A quirk of libFuzzer's cleanse workflow is that it returns no error and an empty input if
        // the input doesn't crash or is already "clean", and doesn't distinguish between the two.
        if (artifact.has_input()) {
          input = artifact.take_input();
        }
        return fpromise::ok(Artifact(FuzzResult::CLEANSED, std::move(input)));
      })
      .wrap_with(workflow_);
}

ZxPromise<> LibFuzzerRunner::ValidateMerge() {
  std::vector<std::string> input_files;
  input_files.reserve(seed_corpus_.size());
  for (const auto& filename : seed_corpus_) {
    input_files.emplace_back(files::JoinPath(kSeedCorpusPath, filename));
  }
  return TryFiles(std::move(input_files))
      .and_then([](Artifact& artifact) -> ZxResult<> {
        if (artifact.fuzz_result() != FuzzResult::NO_ERRORS) {
          FX_LOGS(WARNING) << "Seed corpus contains an input that triggers an error: '"
                           << artifact.input().ToHex() << "'";
          return fpromise::error(ZX_ERR_INVALID_ARGS);
        }
        return fpromise::ok();
      })
      .wrap_with(workflow_, Workflow::Mode::kStart);
}

ZxPromise<Artifact> LibFuzzerRunner::Merge() {
  std::filesystem::remove_all(kTempCorpusPath);
  CreateDirectory(kTempCorpusPath);
  return fpromise::make_promise([this]() -> ZxResult<> {
           if (auto status = AddArgs(); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg("-merge=1"); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg(kTempCorpusPath); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg(kSeedCorpusPath); status != ZX_OK) {
             return fpromise::error(status);
           }
           if (auto status = process_.AddArg(kLiveCorpusPath); status != ZX_OK) {
             return fpromise::error(status);
           }
           return fpromise::ok();
         })
      .and_then(RunAsync())
      .and_then([this](const Artifact& artifact) -> ZxResult<Artifact> {
        std::filesystem::remove_all(kLiveCorpusPath);
        std::filesystem::rename(kTempCorpusPath, kLiveCorpusPath);
        ReloadLiveCorpus();
        return fpromise::ok(Artifact(FuzzResult::MERGED));
      })
      .or_else([](const zx_status_t& status) -> ZxResult<Artifact> {
        std::filesystem::remove_all(kTempCorpusPath);
        return fpromise::error(status);
      })
      .wrap_with(workflow_, Workflow::Mode::kFinish);
}

Status LibFuzzerRunner::CollectStatus() {
  // For libFuzzer, we return the most recently parsed status rather than point-in-time status.
  auto status = CopyStatus(status_);

  // Add other stats.
  auto elapsed = zx::clock::get_monotonic() - start_;
  status.set_elapsed(elapsed.to_nsecs());

  if (process_.IsAlive()) {
    auto stats = process_.GetStats();
    if (stats.is_ok()) {
      std::vector<ProcessStats> process_stats({stats.take_value()});
      status.set_process_stats(std::move(process_stats));
    }
  }

  return status;
}

ZxPromise<> LibFuzzerRunner::Stop() {
  // TODO(https://fxbug.dev/42168245): If libFuzzer-for-Fuchsia watches for something sent to stdin
  // in order to call its |Fuzzer::StaticInterruptCallback|, we could ask libFuzzer to shut itself
  // down. This would guarantee we get all of its output.
  return process_.Kill().and_then(workflow_.Stop());
}

///////////////////////////////////////////////////////////////
// Process-related methods.

zx_status_t LibFuzzerRunner::AddArgs() {
  auto cmdline_iter = cmdline_.begin();
  while (cmdline_iter != cmdline_.end() && *cmdline_iter != "--") {
    if (auto status = process_.AddArg(*cmdline_iter++); status != ZX_OK) {
      return status;
    }
  }
  if (auto runs = options_->runs(); runs != kDefaultRuns) {
    if (auto status = process_.AddArg(MakeArg("runs", options_->runs())); status != ZX_OK) {
      return status;
    }
  }
  if (auto max_total_time = options_->max_total_time(); max_total_time != kDefaultMaxTotalTime) {
    max_total_time = Clamp(max_total_time, kOneSecond, "duration", "second", "max_total_time");
    options_->set_max_total_time(max_total_time);
    if (auto status = process_.AddArg(MakeArg("max_total_time", max_total_time / kOneSecond));
        status != ZX_OK) {
      return status;
    }
  }
  if (auto seed = options_->seed(); seed != kDefaultSeed) {
    if (auto status = process_.AddArg(MakeArg("seed", seed)); status != ZX_OK) {
      return status;
    }
  }
  if (auto max_input_size = options_->max_input_size(); max_input_size != kDefaultMaxInputSize) {
    if (auto status = process_.AddArg(MakeArg("max_len", max_input_size)); status != ZX_OK) {
      return status;
    }
  }
  if (auto mutation_depth = options_->mutation_depth(); mutation_depth != kDefaultMutationDepth) {
    if (auto status = process_.AddArg(MakeArg("mutate_depth", mutation_depth)); status != ZX_OK) {
      return status;
    }
  }
  if (options_->dictionary_level() != kDefaultDictionaryLevel) {
    FX_LOGS(WARNING) << "libFuzzer does not support setting the dictionary level.";
  }
  if (!options_->detect_exits()) {
    FX_LOGS(WARNING) << "libFuzzer does not support ignoring process exits.";
  }
  if (options_->detect_leaks()) {
    if (auto status = process_.AddArg(MakeArg("detect_leaks", "1")); status != ZX_OK) {
      return status;
    }
  }
  if (auto run_limit = options_->run_limit(); run_limit != kDefaultRunLimit) {
    run_limit = Clamp(run_limit, kOneSecond, "duration", "second", "run_limit");
    options_->set_run_limit(run_limit);
    if (auto status = process_.AddArg(MakeArg("timeout", run_limit / kOneSecond));
        status != ZX_OK) {
      return status;
    }
  }
  if (auto malloc_limit = options_->malloc_limit(); malloc_limit != kDefaultMallocLimit) {
    malloc_limit = Clamp(malloc_limit, kOneMb, "memory amount", "MB", "malloc_limit");
    options_->set_malloc_limit(malloc_limit);
    if (auto status = process_.AddArg(MakeArg("malloc_limit_mb", malloc_limit / kOneMb));
        status != ZX_OK) {
      return status;
    }
  }
  if (auto oom_limit = options_->oom_limit(); oom_limit != kDefaultOomLimit) {
    oom_limit = Clamp(oom_limit, kOneMb, "memory amount", "MB", "oom_limit");
    options_->set_oom_limit(oom_limit);
    if (auto status = process_.AddArg(MakeArg("rss_limit_mb", oom_limit / kOneMb));
        status != ZX_OK) {
      return status;
    }
  }
  if (auto purge_interval = options_->purge_interval(); purge_interval != kDefaultPurgeInterval) {
    purge_interval = Clamp(purge_interval, kOneSecond, "duration", "second", "purge_interval");
    options_->set_purge_interval(purge_interval);
    if (auto status =
            process_.AddArg(MakeArg("purge_allocator_interval", purge_interval / kOneSecond));
        status != ZX_OK) {
      return status;
    }
  }
  if (options_->malloc_exitcode() != kDefaultMallocExitcode) {
    FX_LOGS(WARNING) << "libFuzzer does not support setting the 'malloc_exitcode'.";
  }
  if (options_->death_exitcode() != kDefaultDeathExitcode) {
    FX_LOGS(WARNING) << "libFuzzer does not support setting the 'death_exitcode'.";
  }
  if (options_->leak_exitcode() != kDefaultLeakExitcode) {
    FX_LOGS(WARNING) << "libFuzzer does not support setting the 'leak_exitcode'.";
  }
  if (options_->oom_exitcode() != kDefaultOomExitcode) {
    FX_LOGS(WARNING) << "libFuzzer does not support setting the 'oom_exitcode'.";
  }
  if (options_->pulse_interval() != kDefaultPulseInterval) {
    FX_LOGS(WARNING) << "libFuzzer does not support setting the 'pulse_interval'.";
  }
  if (options_->debug()) {
    if (auto status = process_.AddArg(MakeArg("handle_segv", 0)); status != ZX_OK) {
      return status;
    }
    if (auto status = process_.AddArg(MakeArg("handle_bus", 0)); status != ZX_OK) {
      return status;
    }
    if (auto status = process_.AddArg(MakeArg("handle_ill", 0)); status != ZX_OK) {
      return status;
    }
    if (auto status = process_.AddArg(MakeArg("handle_fpe", 0)); status != ZX_OK) {
      return status;
    }
    if (auto status = process_.AddArg(MakeArg("handle_abrt", 0)); status != ZX_OK) {
      return status;
    }
  }
  if (options_->print_final_stats()) {
    if (auto status = process_.AddArg(MakeArg("print_final_stats", 1)); status != ZX_OK) {
      return status;
    }
  }
  if (options_->use_value_profile()) {
    if (auto status = process_.AddArg(MakeArg("use_value_profile", 1)); status != ZX_OK) {
      return status;
    }
  }
  auto sanitizer_options = options_->sanitizer_options();
  const auto& name = sanitizer_options.name;
  const auto& value = sanitizer_options.value;
  if (!name.empty() && !value.empty()) {
    if (auto status = process_.SetEnvVar(name, value); status != ZX_OK) {
      return status;
    }
  }
  auto output_flags = options_->output_flags();
  size_t close_fd_mask = 0;
  if (bool(output_flags & OutputFlags::CLOSE_STDOUT)) {
    close_fd_mask |= 1;
  }
  if (bool(output_flags & OutputFlags::CLOSE_STDERR)) {
    close_fd_mask |= 2;
  }
  if (close_fd_mask) {
    if (auto status = process_.AddArg(MakeArg("close_fd_mask", close_fd_mask)); status != ZX_OK) {
      return status;
    }
  }
  if (bool(output_flags & OutputFlags::VERBOSE)) {
    if (auto status = process_.AddArg(MakeArg("verbosity", 2)); status != ZX_OK) {
      return status;
    }
  }
  if (has_dictionary_) {
    if (auto status = process_.AddArg(MakeArg("dict", kDictionaryPath)); status != ZX_OK) {
      return status;
    }
  }
  std::filesystem::remove(kResultInputPath);
  result_input_pathname_.clear();
  if (auto status = process_.AddArg(MakeArg("exact_artifact_path", kResultInputPath));
      status != ZX_OK) {
    return status;
  }
  while (cmdline_iter != cmdline_.end()) {
    if (auto status = process_.AddArg(*cmdline_iter++); status != ZX_OK) {
      return status;
    }
  }
  return ZX_OK;
}

ZxPromise<Artifact> LibFuzzerRunner::RunAsync() {
  return fpromise::make_promise([this](Context& context) -> ZxResult<> {
           // Nothing is currently written to libFuzzer's stdin or read from its stdout, but they
           // must be available. For certain workflows, libFuzzer re-executes itself using
           // `fdio_spawn` with `FDIO_SPAWN_CLONE_ALL`, which will fail if it cannot clone an fd.
           if (auto status = process_.AddStdinPipe(); status != ZX_OK) {
             FX_LOGS(WARNING) << "Failed to pipe stdin to libFuzzer";
             return fpromise::error(status);
           }
           if (auto status = process_.AddStdoutPipe(); status != ZX_OK) {
             FX_LOGS(WARNING) << "Failed to pipe stdout from libFuzzer";
             return fpromise::error(status);
           }
           if (auto status = process_.AddStderrPipe(); status != ZX_OK) {
             FX_LOGS(WARNING) << "Failed to pipe stderr from libFuzzer";
             return fpromise::error(status);
           }
           if (auto status = process_.AddVmoVdso(); status != ZX_OK) {
             FX_LOGS(WARNING) << "Failed to pipe vDSO from libFuzzer";
             return fpromise::error(status);
           }
           if (auto status = process_.Spawn(); status != ZX_OK) {
             FX_LOGS(WARNING) << "Failed to spawn libFuzzer";
             return fpromise::error(status);
           }
           status_.set_running(true);
           start_ = zx::clock::get_monotonic();
           return fpromise::ok();
         })
      .and_then(ParseOutput())
      .and_then(
          [this, wait = ZxFuture<int64_t>()](Context& context) mutable -> ZxResult<FuzzResult> {
            if (!wait) {
              wait = process_.Wait();
            }
            if (!wait(context)) {
              return fpromise::pending();
            }
            if (wait.is_error()) {
              return fpromise::error(wait.take_error());
            }
            if (fuzz_result_ != FuzzResult::NO_ERRORS) {
              return fpromise::ok(fuzz_result_);
            }
            auto exitcode = wait.take_value();
            switch (exitcode) {
              case kLibFuzzerNoErrorExitCode:
              case ZX_TASK_RETCODE_SYSCALL_KILL:
                return fpromise::ok(FuzzResult::NO_ERRORS);
              case kLibFuzzerOOMExitCode:
              case ZX_TASK_RETCODE_OOM_KILL:
                return fpromise::ok(FuzzResult::OOM);
              case kLibFuzzerTimeoutExitCode:
                return fpromise::ok(FuzzResult::TIMEOUT);
              default:
                return fpromise::ok(FuzzResult::CRASH);
            }
          })
      .or_else([this, kill = ZxFuture<>()](
                   Context& context, const zx_status_t& status) mutable -> ZxResult<FuzzResult> {
        if (!kill) {
          kill = process_.Kill();
        }
        if (!kill(context)) {
          return fpromise::pending();
        }
        return fpromise::error(status);
      })
      .then([this](ZxResult<FuzzResult>& result) -> ZxResult<FuzzResult> {
        status_.set_running(false);
        process_.Reset();
        print_all_ = false;
        pid_ = int64_t(-1);
        fuzz_result_ = FuzzResult::NO_ERRORS;
        return std::move(result);
      })
      .and_then([this](const FuzzResult& fuzz_result) -> ZxResult<Artifact> {
        if (files::IsFile(kResultInputPath)) {
          return fpromise::ok(Artifact(fuzz_result, ReadInputFromFile(kResultInputPath)));
        }
        if (!result_input_pathname_.empty()) {
          return fpromise::ok(Artifact(fuzz_result, ReadInputFromFile(result_input_pathname_)));
        }
        return fpromise::ok(Artifact(fuzz_result));
      });
}

///////////////////////////////////////////////////////////////
// Output parsing methods.

ZxPromise<> LibFuzzerRunner::ParseOutput() {
  std::vector<ZxPromise<>> outputs;
  outputs.push_back(ParseStdout());
  outputs.push_back(ParseStderr());
  return fpromise::join_promise_vector(std::move(outputs))
      .or_else([]() -> ZxResult<std::vector<ZxResult<>>> {
        // Not possible, but needed to transform the joined promise's error type.
        return fpromise::error(ZX_ERR_INTERNAL);
      })
      .and_then([](std::vector<ZxResult<>>& results) -> ZxResult<> {
        for (const auto& result : results) {
          if (result.is_error()) {
            return fpromise::error(result.error());
          }
        }
        return fpromise::ok();
      });
}

ZxPromise<> LibFuzzerRunner::ParseStdout() {
  return fpromise::make_promise(
      [this, read_line = ZxFuture<std::string>()](Context& context) mutable -> ZxResult<> {
        while (true) {
          if (!read_line) {
            read_line = process_.ReadFromStdout();
          }
          if (!read_line(context)) {
            return fpromise::pending();
          }
          if (read_line.is_error()) {
            if (auto status = read_line.error(); status != ZX_ERR_PEER_CLOSED) {
              FX_LOGS(ERROR) << "failed to read libFuzzer stdout: " << zx_status_get_string(status);
              return fpromise::error(status);
            }
            return fpromise::ok();
          }
          auto line = read_line.take_value();
          if (verbose_ && print_all_) {
            std::cout << line << std::endl;
          }
        }
      });
}

ZxPromise<> LibFuzzerRunner::ParseStderr() {
  return fpromise::make_promise(
      [this, read_line = ZxFuture<std::string>()](Context& context) mutable -> ZxResult<> {
        while (true) {
          if (!read_line) {
            read_line = process_.ReadFromStderr();
          }
          if (!read_line(context)) {
            return fpromise::pending();
          }
          if (read_line.is_error()) {
            if (auto status = read_line.error(); status != ZX_ERR_PEER_CLOSED) {
              FX_LOGS(ERROR) << "failed to read libFuzzer stderr: " << zx_status_get_string(status);
              return fpromise::error(status);
            }
            // TODO(https://fxbug.dev/42060461): Rarely, the process output will be truncated. This
            // causes problems for tooling like undercoat. This is the only location in the
            // LibFuzzerRunner that returns `ZX_ERR_IO_INVALID`.
            if (pid_ < 0 && !process_.is_killed()) {
              FX_LOGS(ERROR) << "libFuzzer output terminated prematurely.";
              return fpromise::error(ZX_ERR_IO_INVALID);
            }
            return fpromise::ok();
          }

          auto line = read_line.take_value();
          if (verbose_) {
            std::cerr << line << std::endl;
          }

          // See libFuzzer's |Fuzzer::TryDetectingAMemoryLeak|.
          // This match is ugly, but it's the only message in current libFuzzer that this code can
          // rely on to detect a leak.
          if (line == "INFO: to ignore leaks on libFuzzer side use -detect_leaks=0.") {
            fuzz_result_ = FuzzResult::LEAK;
            continue;
          }

          // When running explicit test inputs, libFuzzer doesn't save artifacts. Record the input
          // name to determine which caused the error.
          absl::string_view input(line);
          if (re2::RE2::Consume(&input, "Running: (\\S+)", &result_input_pathname_)) {
            continue;
          }

          if (re2::RE2::Consume(&input, "==(\\d+)==", &pid_)) {
            continue;
          }

          UpdateReason reason;
          if (ParseLibFuzzerStats(line, &reason, &status_)) {
            UpdateMonitors(reason);
          }
        }
      });
}

}  // namespace fuzzing
