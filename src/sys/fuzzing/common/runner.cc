// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/runner.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/sys/fuzzing/common/status.h"

namespace fuzzing {

Runner::Runner(ExecutorPtr executor) : executor_(executor), monitors_(executor) {
  options_ = MakeOptions();
}

ZxPromise<> Runner::Initialize(std::string pkg_dir, std::vector<std::string> args) {
  return Configure()
      .and_then([this, pkg_dir = std::move(pkg_dir), args = std::move(args)]() -> ZxResult<> {
        bool has_dictionary = false;
        for (const auto& arg : args) {
          // All other flags must be first processed by the derived class.
          if (arg[0] == '-') {
            FX_LOGS(WARNING) << "Unknown flag in component manifest: " << arg;
            return fpromise::error(ZX_ERR_INVALID_ARGS);
          }

          // Remaining arguments must be data files that need to be imported.
          if (arg.rfind("data", 0) != 0) {
            FX_LOGS(WARNING) << "Unknown argument in component manifest: " << arg;
            return fpromise::error(ZX_ERR_INVALID_ARGS);
          }

          // A file argument is a dictionary.
          auto pathname = files::JoinPath(pkg_dir, arg);
          if (files::IsFile(pathname)) {
            if (has_dictionary) {
              FX_LOGS(WARNING) << "Multiple dictionaries found: " << arg;
              return fpromise::error(ZX_ERR_INVALID_ARGS);
            }
            std::vector<uint8_t> data;
            if (!files::ReadFileToVector(pathname, &data)) {
              FX_LOGS(WARNING) << "Failed to read dictionary '" << pathname
                               << "': " << strerror(errno);
              return fpromise::error(ZX_ERR_IO);
            }
            if (auto status = ParseDictionary(Input(data)); status != ZX_OK) {
              return fpromise::error(status);
            }
            has_dictionary = true;
            continue;
          }

          // Directory arguments are seed corpora.
          if (files::IsDirectory(pathname)) {
            std::vector<std::string> filenames;
            if (!files::ReadDirContents(pathname, &filenames)) {
              FX_LOGS(WARNING) << "Failed to read seed corpus '" << pathname
                               << "': " << strerror(errno);
              return fpromise::error(ZX_ERR_IO);
            }
            for (const auto& filename : filenames) {
              auto input_file = files::JoinPath(pathname, filename);
              if (!files::IsFile(input_file)) {
                continue;
              }
              std::vector<uint8_t> data;
              if (!files::ReadFileToVector(input_file, &data)) {
                FX_LOGS(WARNING) << "Failed to read input '" << input_file
                                 << "': " << strerror(errno);
                return fpromise::error(ZX_ERR_IO);
              }
              if (auto status = AddToCorpus(CorpusType::SEED, Input(data)); status != ZX_OK) {
                return fpromise::error(status);
              }
            }
            continue;
          }

          // No other positional arguments are recognized here.
          FX_LOGS(WARNING) << "No such path in package: " << arg;
          return fpromise::error(ZX_ERR_NOT_FOUND);
        }
        return fpromise::ok();
      })
      .wrap_with(scope_);
}

ZxPromise<> Runner::Configure() {
  return fpromise::make_promise([this]() -> ZxResult<> {
    if (options_->seed() == kDefaultSeed) {
      options_->set_seed(static_cast<uint32_t>(zx::ticks::now().get()));
    }
    return fpromise::ok();
  });
}

///////////////////////////////////////////////////////////////
// Workflow methods

ZxPromise<> Runner::Workflow::Start(Mode mode) {
  return fpromise::make_promise([this, mode]() -> ZxResult<> {
    if (completer_) {
      FX_LOGS(WARNING) << "Another fuzzing workflow is already in progress.";
      return fpromise::error(ZX_ERR_BAD_STATE);
    }
    ZxBridge<> bridge;
    completer_ = std::move(bridge.completer);
    consumer_ = std::move(bridge.consumer);
    if (mode == Mode::kStart || mode == Mode::kBoth) {
      runner_->StartWorkflow(scope_);
    }
    return fpromise::ok();
  });
}

ZxPromise<Artifact> Runner::TryOne(Input input) {
  std::vector<Input> inputs;
  inputs.emplace_back(std::move(input));
  return TryEach(std::move(inputs));
}

ZxPromise<> Runner::Workflow::Stop() {
  return consumer_ ? AwaitConsumer(std::move(consumer_)).box()
                   : fpromise::make_promise([]() -> ZxResult<> { return fpromise::ok(); });
}

void Runner::Workflow::Finish(Mode mode) {
  if (completer_) {
    if (mode == Mode::kFinish || mode == Mode::kBoth) {
      runner_->FinishWorkflow();
    }
    completer_.complete_ok();
  }
}

///////////////////////////////////////////////////////////////
// Status-related methods.

void Runner::AddMonitor(fidl::InterfaceHandle<Monitor> monitor) {
  monitors_.Add(std::move(monitor));
}

void Runner::UpdateMonitorsWithStatus(UpdateReason reason, Status status) {
  if (monitors_.active()) {
    monitors_.set_status(std::move(status));
    monitors_.Update(reason);
  }
}

void Runner::UpdateMonitors(UpdateReason reason) {
  UpdateMonitorsWithStatus(reason, CollectStatus());
}

}  // namespace fuzzing
