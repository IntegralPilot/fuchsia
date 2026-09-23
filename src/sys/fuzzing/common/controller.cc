// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/controller.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/sanitizer.h>
#include <zircon/status.h>

#include <iostream>

#include "src/sys/fuzzing/common/async-eventpair.h"
#include "src/sys/fuzzing/common/async-socket.h"
#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/corpus-reader-client.h"
#include "src/sys/fuzzing/common/options.h"

namespace fuzzing {

using ::fuchsia::fuzzer::DONE_MARKER;

ControllerImpl::ControllerImpl(RunnerPtr runner)
    : binding_(this),
      executor_(runner->executor()),
      runner_(runner),
      artifact_(fpromise::ok(Artifact())) {
  if (auto status = zx::event::create(0, &changed_); status != ZX_OK) {
    FX_LOGS(FATAL) << "Failed to create artifact event: " << zx_status_get_string(status);
  }
  if (auto status = changed_.signal(0, Signal::kSync); status != ZX_OK) {
    FX_LOGS(FATAL) << "Failed to signal artifact event: " << zx_status_get_string(status);
  }
}

void ControllerImpl::Bind(fidl::InterfaceRequest<Controller> request) {
  FX_DCHECK(runner_);
  binding_.Bind(std::move(request));
}

ZxPromise<> ControllerImpl::ResetArtifact() {
  return fpromise::make_promise([this]() -> ZxResult<> {
           if (artifact_.is_ok() && artifact_.value().is_empty()) {
             return fpromise::ok();
           }
           artifact_ = fpromise::ok(Artifact());
           if (auto status = changed_.signal(0, Signal::kSync); status != ZX_OK) {
             FX_LOGS(ERROR) << "Failed to signal artifact event: " << zx_status_get_string(status);
             return fpromise::error(status);
           }
           return fpromise::ok();
         })
      .wrap_with(scope_);
}

ZxPromise<> ControllerImpl::Finish(ZxResult<Artifact> result) {
  return fpromise::make_promise([this, result = std::move(result)]() mutable -> ZxResult<> {
           FX_CHECK(artifact_.is_ok() && artifact_.value().is_empty());
           if (result.is_ok()) {
             artifact_ = fpromise::ok(result.take_value());
           } else {
             artifact_ = fpromise::error(result.error());
           }
           if (auto status = changed_.signal(0, Signal::kSync); status != ZX_OK) {
             FX_LOGS(ERROR) << "Failed to signal artifact event: " << zx_status_get_string(status);
             return fpromise::error(status);
           }
           std::cout << std::endl << DONE_MARKER << std::endl;
           std::cerr << std::endl << DONE_MARKER << std::endl;
           FX_LOGS(INFO) << DONE_MARKER;
           return fpromise::ok();
         })
      .wrap_with(scope_);
}

///////////////////////////////////////////////////////////////
// FIDL methods.

void ControllerImpl::Configure(Options options, ConfigureCallback callback) {
  auto task =
      fpromise::make_promise([this, overrides = std::move(options)]() mutable -> ZxResult<> {
        auto options = runner_->options();
        SetOptions(options.get(), overrides);
        return fpromise::ok();
      })
          .and_then(runner_->Configure())
          .then(
              [callback = std::move(callback)](ZxResult<>& result) { callback(std::move(result)); })
          .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::GetOptions(GetOptionsCallback callback) {
  const auto& options = runner_->options();
  callback(CopyOptions(*options));
}

void ControllerImpl::AddToCorpus(CorpusType corpus_type, FidlInput fidl_input,
                                 AddToCorpusCallback callback) {
  auto task = AsyncSocketRead(executor_, std::move(fidl_input))
                  .and_then([this, corpus_type](Input& received) -> ZxResult<> {
                    return AsZxResult(runner_->AddToCorpus(corpus_type, std::move(received)));
                  })
                  .then([callback = std::move(callback)](ZxResult<>& result) {
                    callback(std::move(result));
                  })
                  .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::ReadCorpus(CorpusType corpus_type, fidl::InterfaceHandle<CorpusReader> reader,
                                ReadCorpusCallback callback) {
  auto client = std::make_unique<CorpusReaderClient>(executor_);
  client->Bind(std::move(reader));

  // The client must be moved into the promise to prevent it going out of scope.
  auto task = fpromise::make_promise([runner = runner_, corpus_type, client = std::move(client),
                                      send = ZxFuture<>()](Context& context) mutable -> ZxResult<> {
                if (!send) {
                  send = client->Send(runner->GetCorpus(corpus_type));
                }
                if (!send(context)) {
                  return fpromise::pending();
                }
                return send.take_result();
              }).and_then([callback = std::move(callback)]() { callback(); });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::WriteDictionary(FidlInput dictionary, WriteDictionaryCallback callback) {
  auto task = AsyncSocketRead(executor_, std::move(dictionary))
                  .and_then([this](Input& received) -> ZxResult<> {
                    return AsZxResult(runner_->ParseDictionary(std::move(received)));
                  })
                  .then([callback = std::move(callback)](ZxResult<>& result) {
                    callback(std::move(result));
                  })
                  .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::ReadDictionary(ReadDictionaryCallback callback) {
  callback(AsyncSocketWrite(executor_, runner_->GetDictionaryAsInput()));
}

void ControllerImpl::AddMonitor(fidl::InterfaceHandle<Monitor> monitor,
                                AddMonitorCallback callback) {
  FX_DCHECK(runner_);
  runner_->AddMonitor(std::move(monitor));
  callback();
}

// Invokes the callback provided for long-running workflows.
template <typename ResultType, typename WorkflowCallback>
static ResultType Acknowledge(ResultType&& result, WorkflowCallback callback) {
  if (result.is_ok()) {
    callback(fpromise::ok());
  } else {
    callback(fpromise::error(result.error()));
  }
  return std::move(result);
}

void ControllerImpl::Fuzz(FuzzCallback callback) {
  auto task = ResetArtifact()
                  .then([callback = std::move(callback)](ZxResult<>& result) mutable {
                    return Acknowledge(std::move(result), std::move(callback));
                  })
                  .and_then(runner_->Fuzz())
                  .then([this](ZxResult<Artifact>& result) { return Finish(std::move(result)); })
                  .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::TryOne(FidlInput fidl_input, TryOneCallback callback) {
  auto task = ResetArtifact()
                  .and_then(AsyncSocketRead(executor_, std::move(fidl_input)))
                  .then([callback = std::move(callback)](ZxResult<Input>& result) mutable {
                    return Acknowledge(std::move(result), std::move(callback));
                  })
                  .and_then([this](Input& input) { return runner_->TryOne(std::move(input)); })
                  .then([this](ZxResult<Artifact>& result) { return Finish(std::move(result)); })
                  .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::Minimize(FidlInput fidl_input, MinimizeCallback callback) {
  auto task =
      ResetArtifact()
          .and_then(AsyncSocketRead(executor_, std::move(fidl_input)))
          .and_then([this](Input& input) { return runner_->ValidateMinimize(std::move(input)); })
          .then([callback = std::move(callback)](ZxResult<Artifact>& result) mutable {
            return Acknowledge(std::move(result), std::move(callback));
          })
          .and_then([this](Artifact& artifact) { return runner_->Minimize(std::move(artifact)); })
          .then([this](ZxResult<Artifact>& result) { return Finish(std::move(result)); })
          .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::Cleanse(FidlInput fidl_input, CleanseCallback callback) {
  auto task = ResetArtifact()
                  .and_then(AsyncSocketRead(executor_, std::move(fidl_input)))
                  .then([callback = std::move(callback)](ZxResult<Input>& result) mutable {
                    return Acknowledge(std::move(result), std::move(callback));
                  })
                  .and_then([this](Input& input) { return runner_->Cleanse(std::move(input)); })
                  .then([this](ZxResult<Artifact>& result) { return Finish(std::move(result)); })
                  .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::Merge(MergeCallback callback) {
  auto task = ResetArtifact()
                  .and_then([this]() { return runner_->ValidateMerge(); })
                  .then([callback = std::move(callback)](ZxResult<>& result) mutable {
                    return Acknowledge(std::move(result), std::move(callback));
                  })
                  .and_then([this]() { return runner_->Merge(); })
                  .then([this](ZxResult<Artifact>& result) { return Finish(std::move(result)); })
                  .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::GetStatus(GetStatusCallback callback) { callback(runner_->CollectStatus()); }

void ControllerImpl::WatchArtifact(WatchArtifactCallback callback) {
  auto task =
      executor_->MakePromiseWaitHandle(zx::unowned_handle(changed_.get()), kSync)
          .then([this](const ZxResult<zx_packet_signal_t>& result) -> ZxResult<FidlArtifact> {
            if (result.is_error()) {
              auto status = result.error();
              FX_LOGS(ERROR) << "Failed to watch for artifact: " << zx_status_get_string(status);
              return fpromise::error(status);
            }
            if (auto status = changed_.signal(Signal::kSync, 0); status != ZX_OK) {
              FX_LOGS(ERROR) << "Failed to clear artifact event: " << zx_status_get_string(status);
              return fpromise::error(status);
            }
            if (artifact_.is_error()) {
              return fpromise::error(artifact_.error());
            }
            return fpromise::ok(AsyncSocketWrite(executor_, artifact_.value()));
          })
          .then([callback = std::move(callback)](ZxResult<FidlArtifact>& result) {
            if (result.is_error()) {
              FidlArtifact fidl_artifact;
              fidl_artifact.set_error(result.error());
              callback(std::move(fidl_artifact));
            } else {
              callback(result.take_value());
            }
          })
          .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::Stop() {
  if (runner_) {
    executor_->schedule_task(runner_->Stop());
  }
}

}  // namespace fuzzing
