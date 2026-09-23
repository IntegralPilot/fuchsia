// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/testing/runner.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <iostream>

#include <gtest/gtest.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/sys/fuzzing/common/status.h"

namespace fuzzing {
namespace {

const char* kCrash = "CRASH";
const size_t kCrashLen = 5;

std::string as_string(const Input& input) {
  return input.size() != 0 ? std::string(reinterpret_cast<const char*>(input.data()), input.size())
                           : std::string();
}

size_t get_prefix_len(const std::string& input) {
  auto max = std::min(input.size(), kCrashLen);
  for (size_t i = 0; i < max; ++i) {
    if (input[i] != kCrash[i]) {
      return i;
    }
  }
  return max;
}

}  // namespace

const char* kFakeRunnerFlag = "--fake";

RunnerPtr FakeRunner::MakePtr(ExecutorPtr executor) {
  return RunnerPtr(new FakeRunner(std::move(executor)));
}

FakeRunner::FakeRunner(ExecutorPtr executor) : Runner(executor), workflow_(this) {
  seed_corpus_.emplace_back(Input());
  live_corpus_.emplace_back(Input());
}

ZxPromise<> FakeRunner::Initialize(std::string pkg_dir, std::vector<std::string> args) {
  auto it = std::find(args.begin(), args.end(), kFakeRunnerFlag);
  if (it != args.end()) {
    flag_ = true;
    args.erase(it, args.end());
  }
  return Runner::Initialize(std::move(pkg_dir), std::move(args));
}

zx_status_t FakeRunner::AddToCorpus(CorpusType corpus_type, Input input) {
  auto* corpus = corpus_type == CorpusType::SEED ? &seed_corpus_ : &live_corpus_;
  corpus->emplace_back(std::move(input));
  return ZX_OK;
}

std::vector<Input> FakeRunner::GetCorpus(CorpusType corpus_type) {
  const auto* corpus = corpus_type == CorpusType::SEED ? &seed_corpus_ : &live_corpus_;
  std::vector<Input> inputs;
  inputs.reserve(corpus->size());
  for (const auto& input : *corpus) {
    if (input.size() != 0) {
      inputs.emplace_back(input.Duplicate());
    }
  }
  std::sort(inputs.begin(), inputs.end());
  return inputs;
}

zx_status_t FakeRunner::ParseDictionary(const Input& input) {
  if (input == FakeRunner::invalid_dictionary()) {
    return ZX_ERR_INVALID_ARGS;
  }
  dictionary_ = input.Duplicate();
  return ZX_OK;
}

Input FakeRunner::GetDictionaryAsInput() const { return dictionary_.Duplicate(); }

ZxPromise<Artifact> FakeRunner::Fuzz() {
  return Run()
      .and_then([this](Artifact& artifact) {
        if (artifact.fuzz_result() != FuzzResult::NO_ERRORS) {
          return fpromise::ok(std::move(artifact));
        }
        // If no result was set up, sequentially increment each byte until it matches |kCrash|.
        char input[kCrashLen + 1] = {0};
        auto max_runs = options()->runs();
        uint32_t runs = 1;
        zx::duration elapsed(0);
        status_.set_running(true);
        status_.set_elapsed(elapsed.to_nsecs());
        status_.set_runs(runs);
        UpdateMonitors(UpdateReason::INIT);
        for (; runs < max_runs || max_runs == 0; ++runs) {
          inputs_.emplace_back(input);
          auto prefix_len = get_prefix_len(input);
          if (prefix_len == kCrashLen) {
            return fpromise::ok(Artifact(FuzzResult::CRASH, Input(kCrash)));
          }
          input[prefix_len]++;
          elapsed += zx::usec(10);
          status_.set_elapsed(elapsed.to_nsecs());
          status_.set_runs(runs);
          if (runs % 10 == 0) {
            UpdateMonitors(UpdateReason::PULSE);
          }
        }
        status_.set_running(false);
        UpdateMonitors(UpdateReason::DONE);
        return fpromise::ok(Artifact(FuzzResult::NO_ERRORS, Input()));
      })
      .wrap_with(workflow_);
}

ZxPromise<Artifact> FakeRunner::TryEach(std::vector<Input> inputs) {
  return Run()
      .and_then(
          [this, inputs = std::move(inputs)](const Artifact& artifact) -> ZxResult<FuzzResult> {
            for (const auto& input : inputs) {
              inputs_.emplace_back(input.Duplicate());
            }
            if (artifact.fuzz_result() != FuzzResult::NO_ERRORS) {
              return fpromise::ok(artifact.fuzz_result());
            }
            // If no result was set up, crash if the input contains |kCrash|.
            for (auto& input : inputs) {
              auto pos = as_string(input).find(kCrash);
              if (pos != std::string::npos) {
                return fpromise::ok(FuzzResult::CRASH);
              }
            }
            return fpromise::ok(FuzzResult::NO_ERRORS);
          })
      .and_then([](FuzzResult& fuzz_result) { return fpromise::ok(Artifact(fuzz_result)); })
      .wrap_with(workflow_);
}

ZxPromise<Artifact> FakeRunner::ValidateMinimize(Input input) {
  return fpromise::make_promise([this, input = std::move(input)]() mutable -> ZxResult<Artifact> {
           if (validation_error_ != ZX_OK) {
             return fpromise::error(validation_error_);
           }
           inputs_.emplace_back(input.Duplicate());
           return fpromise::ok(Artifact(FuzzResult::NO_ERRORS, std::move(input)));
         })
      .wrap_with(workflow_);
}

ZxPromise<Artifact> FakeRunner::Minimize(Artifact validated) {
  return Run()
      .and_then(
          [validated = std::move(validated)](Artifact& artifact) mutable -> ZxResult<Artifact> {
            auto minimized = artifact.take_input();
            if (minimized.size() == 0) {
              minimized = validated.take_input();
            }
            // If "CRASH" appears in the input, remove all other bytes.
            if (as_string(minimized).find(kCrash) != std::string::npos) {
              minimized = Input(kCrash);
            }
            return fpromise::ok(Artifact(FuzzResult::MINIMIZED, std::move(minimized)));
          })
      .wrap_with(workflow_);
}

ZxPromise<Artifact> FakeRunner::Cleanse(Input input) {
  return Run()
      .and_then(
          [this, cleansed = std::move(input)](Artifact& artifact) mutable -> ZxResult<Artifact> {
            inputs_.emplace_back(cleansed.Duplicate());
            if (artifact.input().size() != 0) {
              cleansed = artifact.take_input();
            }
            // If "CRASH" appears in the input, cleanse all other bytes.
            if (auto pos = as_string(cleansed).find(kCrash); pos != std::string::npos) {
              std::string data(cleansed.size(), ' ');
              data.replace(pos, kCrashLen, kCrash, 0, kCrashLen);
              cleansed = Input(data);
            }
            return fpromise::ok(Artifact(FuzzResult::CLEANSED, std::move(cleansed)));
          })
      .wrap_with(workflow_);
}

ZxPromise<> FakeRunner::ValidateMerge() {
  return fpromise::make_promise([this]() -> ZxResult<> {
           if (validation_error_ != ZX_OK) {
             return fpromise::error(validation_error_);
           }
           return fpromise::ok();
         })
      .wrap_with(workflow_);
}

ZxPromise<Artifact> FakeRunner::Merge() {
  return Run()
      .and_then([this](Artifact& artifact) {
        // The fake runner interprets the length of the input prefix that matches |kCrash| as that
        // input's "number of features". This makes merging straightforward, as the input to keep is
        // just the first input of a given prefix length when sorted lexicographically.
        size_t max_prefix_len = 0;
        for (const auto& input : seed_corpus_) {
          inputs_.emplace_back(input.Duplicate());
          auto prefix_len = get_prefix_len(as_string(input));
          if (prefix_len > max_prefix_len) {
            max_prefix_len = prefix_len;
          }
        }
        std::vector<Input> unmerged = std::move(live_corpus_);
        live_corpus_.clear();
        live_corpus_.emplace_back(Input());
        std::sort(unmerged.begin(), unmerged.end());
        for (const auto& input : unmerged) {
          inputs_.emplace_back(input.Duplicate());
          auto prefix_len = get_prefix_len(as_string(input));
          if (prefix_len > max_prefix_len) {
            live_corpus_.emplace_back(input.Duplicate());
            max_prefix_len = prefix_len;
          }
        }
        return fpromise::ok(Artifact(FuzzResult::MERGED));
      })
      .wrap_with(workflow_);
}

ZxPromise<Artifact> FakeRunner::Run() {
  return fpromise::make_promise([this]() -> ZxResult<Artifact> {
    if (error_ != ZX_OK) {
      return fpromise::error(error_);
    }
    return fpromise::ok(Artifact(result_, result_input_.Duplicate()));
  });
}

Status FakeRunner::CollectStatus() { return CopyStatus(status_); }

ZxPromise<> FakeRunner::Stop() {
  if (!completer_) {
    Bridge<> bridge;
    completer_ = std::move(bridge.completer);
    consumer_ = std::move(bridge.consumer);
  }
  return workflow_.Stop().inspect(
      [completer = std::move(completer_)](const ZxResult<>& result) mutable {
        completer.complete_ok();
      });
}

Promise<> FakeRunner::AwaitStop() {
  if (!consumer_) {
    Bridge<> bridge;
    completer_ = std::move(bridge.completer);
    consumer_ = std::move(bridge.consumer);
  }
  return consumer_.promise_or(fpromise::error());
}

void MakeCorpus(const std::string& pkg_path, std::initializer_list<const char*> inputs,
                std::vector<Input>* out) {
  ASSERT_TRUE(files::CreateDirectory(pkg_path)) << pkg_path;
  out->reserve(out->size() + inputs.size());
  for (const auto* input : inputs) {
    auto pathname = files::JoinPath(pkg_path, input);
    ASSERT_TRUE(files::WriteFile(pathname, input)) << pathname;
    out->emplace_back(input);
  }
  std::sort(out->begin(), out->end());
  out->erase(std::unique(out->begin(), out->end()), out->end());
}

void WriteInput(const std::string& pkg_path, Input contents) {
  auto dirname = files::GetDirectoryName(pkg_path);
  ASSERT_TRUE(files::CreateDirectory(dirname)) << dirname;
  const auto* data = reinterpret_cast<const char*>(contents.data());
  ASSERT_TRUE(files::WriteFile(pkg_path, data, contents.size())) << pkg_path;
}

}  // namespace fuzzing
