// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_RUNNER_H_
#define SRC_SYS_FUZZING_COMMON_RUNNER_H_

#include <fuchsia/fuzzer/cpp/fidl.h>
#include <lib/fidl/cpp/interface_handle.h>
#include <lib/fit/function.h>

#include <memory>
#include <vector>

#include "src/lib/fxl/macros.h"
#include "src/sys/fuzzing/common/artifact.h"
#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/input.h"
#include "src/sys/fuzzing/common/monitor-clients.h"
#include "src/sys/fuzzing/common/options.h"
#include "src/sys/fuzzing/common/result.h"

namespace fuzzing {

using ::fuchsia::fuzzer::MonitorPtr;
using ::fuchsia::fuzzer::Status;
using ::fuchsia::fuzzer::TargetAdapter;
using ::fuchsia::fuzzer::UpdateReason;
using CorpusType = ::fuchsia::fuzzer::Corpus;

// |RunnerPtr| is the preferred way to reference a |Runner| in a future or promise without needing
// to wrap it in a scope.
class Runner;
using RunnerPtr = std::shared_ptr<Runner>;

// This base class encapsulates the logic of performing a sequence of fuzzing runs. In
// particular, it defines virtual methods for performing the fuzzing workflows asynchronously, and
// invokes those methods on a dedicated worker thread to perform them without blocking the
// controller's FIDL dispatcher thread.
class Runner {
 public:
  // Note that the destructor cannot call |Close|, |Interrupt| or |Join|, as they are virtual.
  // Instead, both this class and any derived class should have corresponding non-virtual "Impl"
  // methods and call those on destruction.
  virtual ~Runner() = default;

  // Accessors.
  const ExecutorPtr& executor() const { return executor_; }
  const OptionsPtr& options() const { return options_; }

  // Sets up the runner using arguments from the component manifest.
  virtual ZxPromise<> Initialize(std::string pkg_dir, std::vector<std::string> args);

  // Examines the options, and prepares the runner for fuzzing given their values.
  virtual ZxPromise<> Configure();

  // Add an input to the specified corpus. Returns ZX_ERR_INVALID_ARGS if |corpus_type| is
  // unrecognized.
  virtual zx_status_t AddToCorpus(CorpusType corpus_type, Input input) = 0;

  // Returns a copy of all non-empty inputs in the corpus of the given |corpus_type|.
  // The vector is sorted using Input's comparison operators.
  virtual std::vector<Input> GetCorpus(CorpusType corpus_type) = 0;

  // Parses the given |input| as an AFL-style dictionary. For format details, see
  // https://lcamtuf.coredump.cx/afl/technical_details.txt. Returns ZX_ERR_INVALID_ARGS if parsing
  // fails.
  virtual zx_status_t ParseDictionary(const Input& input) = 0;

  // Returns the current dictionary serialized into an |Input|.
  virtual Input GetDictionaryAsInput() const = 0;

  // Adds a subscriber for status updates.
  void AddMonitor(fidl::InterfaceHandle<Monitor> monitor);

  // Fuzzing workflows corresponding to methods in `fuchsia.fuzzer.Controller`.

  // Implementation of `fuchsia.fuzzer.Controller/Fuzz`.
  virtual ZxPromise<Artifact> Fuzz() = 0;

  // Implementation of `fuchsia.fuzzer.Controller/TryOne`.
  ZxPromise<Artifact> TryOne(Input input);
  virtual ZxPromise<Artifact> TryEach(std::vector<Input> inputs) = 0;

  // Implementation of `fuchsia.fuzzer.Controller/Minimize`. |ValidateMinimize| must be called
  // before |Minimize|.
  virtual ZxPromise<Artifact> ValidateMinimize(Input input) = 0;
  virtual ZxPromise<Artifact> Minimize(Artifact artifact) = 0;

  // Implementation of `fuchsia.fuzzer.Controller/Cleanse`.
  virtual ZxPromise<Artifact> Cleanse(Input input) = 0;

  // Implementation of `fuchsia.fuzzer.Controller/Merge`. |ValidateMerge| must be called
  // before |Merge|.
  virtual ZxPromise<> ValidateMerge() = 0;
  virtual ZxPromise<Artifact> Merge() = 0;

  // Creates a |Status| object representing all attached processes.
  virtual Status CollectStatus() = 0;

  // Cancels the current workflow.
  virtual ZxPromise<> Stop() = 0;

 protected:
  void set_options(Options options) { *options_ = std::move(options); }

  // Represents a single fuzzing workflow, e.g. |TryOne|, |Minimize|, etc. It holds a pointer to
  // the object that created it, but this is safe: it cannot outlive the object it is a part of.
  // It should be used in the normal way, e.g. using |wrap_with|.
  //
  // Derived runners should include a `Workflow` member, and use it to wrap any returned promises
  // that are exclusive with a fuzzing workflow, e.g. the workflows themselves and method like
  // `Configure`. Promises that are a part of a workflow can be wrapped with the workflow's
  // `scope()`.
  //
  class Workflow final {
   public:
    // In most cases, a fuzzing workflow is encapsulated by a promise returned by a single method,
    // e.g. `Fuzz`. In this case, the promise is wrapped by a `Workflow` that both starts and
    // finishes the workflow.
    //
    // If a workflow spans multiple promises from multiple methods, each method may specify a `mode`
    // when wrapping its promise. For example, if a workflow is made up of 3 promises from 3
    // different methods:
    //
    // * The first may wrap its promise with `.wrap_with(workflow_, Workflow::Mode::kStart);`.
    // * The second may wrap its promise with `.wrap_with(workflow_, Workflow::Mode::kNeither);`.
    // * The last may wrap its promise with `.wrap_with(workflow_, Workflow::Mode::kFinish);`.
    enum Mode {
      kStart,
      kFinish,
      kBoth,
      kNeither,
    };

    explicit Workflow(Runner* runner) : runner_(runner) {}
    ~Workflow() = default;

    Scope& scope() { return scope_; }

    // Use |wrap_with(workflow_)| on promises that implement a workflow's behavior to create scoped
    // actions on set up and tear down.
    template <typename Promise>
    decltype(auto) wrap(Promise promise, Mode mode = Mode::kBoth) {
      static_assert(std::is_same<typename Promise::error_type, zx_status_t>::value,
                    "Workflows must use an error type of zx_status_t.");
      return Start(mode)
          .and_then(std::move(promise))
          .inspect([this, mode](const typename Promise::result_type& result) { Finish(mode); })
          .wrap_with(scope_);
    }

    // Returns a promise to stop the current workflow. The promise completes after |Finish| is
    // called.
    ZxPromise<> Stop();

   private:
    ZxPromise<> Start(Mode mode);
    void Finish(Mode mode);

    Runner* runner_ = nullptr;
    ZxCompleter<> completer_;
    ZxConsumer<> consumer_;
    Scope scope_;
  };

  explicit Runner(ExecutorPtr executor);

  // These methods allow specific runners to implement actions that should be performed at the start
  // or end of a workflow. They are called automatically by |Workflow|. The runners may also create
  // additional tasks constrained to the workflow's |scope|.
  virtual void StartWorkflow(Scope& scope) {}
  virtual void FinishWorkflow() {}

  // Labels the current status with the given `reason`, and sends it all attached `Monitor`s.
  virtual void UpdateMonitorsWithStatus(UpdateReason reason, Status status);

  // Like `UpdateMonitorsWithStatus`, but provides the status via `CollectStatus`.
  void UpdateMonitors(UpdateReason reason);

 private:
  // Like |UpdateMonitors|, but uses UpdateReason::DONE as the reason and disconnects monitors after
  // they acknowledge receipt.
  void FinishMonitoring();

  ExecutorPtr executor_;
  OptionsPtr options_;
  MonitorClients monitors_;
  Scope scope_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(Runner);
};

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_COMMON_RUNNER_H_
