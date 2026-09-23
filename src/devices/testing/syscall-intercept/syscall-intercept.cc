// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "syscall-intercept.h"

#include <fidl/fuchsia.test.syscalls/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <zircon/syscalls-next.h>

#include <memory>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>
#include <sdk/lib/driver/logging/cpp/logger.h>

#include "fidl/fuchsia.test.syscalls/cpp/wire_types.h"

namespace syscall_intercept {
namespace {

struct State {
  // Number of calls counted so far.
  uint32_t calls_count = 0;

  // The status to return on syscall.
  zx_status_t status = ZX_ERR_INTERNAL;
};

fbl::Mutex state_instance_lock;
std::unique_ptr<State> state_instance TA_GUARDED(state_instance_lock);

class Handler : public fidl::WireServer<fuchsia_test_syscalls::Control> {
 public:
  explicit Handler(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  ~Handler() override {
    if (outgoing_ != nullptr) {
      const zx::result<> result = outgoing_->RemoveService<fuchsia_test_syscalls::ControlService>();
      if (result.is_error()) {
        fdf::error("Failed to remove ControlService.");
      }
      outgoing_.reset();
    }
  }

  void AddBindings(const std::shared_ptr<fdf::OutgoingDirectory>& outgoing) {
    outgoing_ = outgoing;
    fuchsia_test_syscalls::ControlService::InstanceHandler h({
        .control = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
    });
    const zx::result<> result =
        outgoing->AddService<fuchsia_test_syscalls::ControlService>(std::move(h));
    if (result.is_error()) {
      fdf::error("Failed to add ControlService, this is probably not what you want.");
    }
  }

  /// fuchsia.test.syscalls/Control.SetSuspendEnterResult
  void SetSuspendEnterResult(
      ::fuchsia_test_syscalls::wire::ControlSetSuspendEnterResultRequest* request,
      SetSuspendEnterResultCompleter::Sync& completer) override {
    {
      fbl::AutoLock lock{&state_instance_lock};
      state_instance->status = request->status;
    }
    completer.Reply();
  }

  /// fuchsia.test.syscalls/Control.GetState
  void GetState(GetStateCompleter::Sync& completer) override {
    uint32_t calls_count = 0;
    {
      fbl::AutoLock lock{&state_instance_lock};
      calls_count = state_instance->calls_count;
    }
    completer.Reply(calls_count);
  }

 private:
  void Serve(fidl::ServerEnd<fuchsia_test_syscalls::Control> request) {
    bindings_.AddBinding(dispatcher_, std::move(request), this, fidl::kIgnoreBindingClosure);
  }

  std::shared_ptr<fdf::OutgoingDirectory> outgoing_;
  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_test_syscalls::Control> bindings_;
};

// Only accessed by FIDL.
std::unique_ptr<Handler> handler;

}  // namespace

SuspendObserver::SuspendObserver(const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                                 async_dispatcher_t* dispatcher) {
  {
    fbl::AutoLock lock{&state_instance_lock};
    state_instance = std::make_unique<State>();
  }
  handler = std::make_unique<Handler>(dispatcher);
  handler->AddBindings(outgoing);
}

SuspendObserver::~SuspendObserver() {
  handler.reset();
  {
    fbl::AutoLock lock{&state_instance_lock};
    state_instance.reset();
  }
}

}  // namespace syscall_intercept

__EXPORT zx_status_t zx_system_suspend_enter(zx_handle_t resource,
                                             zx_instant_boot_t resume_deadline, uint64_t options,
                                             zx_wake_source_report_header_t* out_header,
                                             zx_wake_source_report_entry_t* out_entries,
                                             uint32_t num_entries, uint32_t* actual_entries) {
  {
    fbl::AutoLock lock(&syscall_intercept::state_instance_lock);
    if (syscall_intercept::state_instance != nullptr) {
      auto& instance = syscall_intercept::state_instance;
      instance->calls_count++;

      // Possibly also make a real syscall here at some point in the future.

      return instance->status;
    }
  }

  fdf::error("syscall_intercept::state_instance is not initialized.");
  // Returning an error here if the instrumentation is in a bad state.
  // Between the unexpected error and the log message this should
  // hopefully be enough of a suggestion as to where the problem
  // may be.
  return ZX_ERR_INTERNAL;
}
