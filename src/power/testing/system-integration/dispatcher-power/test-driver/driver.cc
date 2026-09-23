// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/test.power.dispatcher/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fdf/dispatcher.h>

namespace {

class Driver final : public fdf::DriverBase2 {
 public:
  Driver() : fdf::DriverBase2("dispatcher_power_test_driver") {}

  zx::result<> Start(fdf::DriverContext context) override {
    fdf::info("Dispatcher power test driver started!");
    dispatcher_ = dispatcher();
    ScheduleNextTask();

    auto test_ctrl = context.incoming().Connect<test_power_dispatcher::TestController>();
    if (test_ctrl.is_ok()) {
      test_ctrl_.Bind(std::move(test_ctrl.value()), dispatcher_);
    } else {
      fdf::error("Failed to connect to TestController: {}", test_ctrl.status_string());
    }

    auto token_opt = context.power_element_token();
    if (token_opt.has_value()) {
      token_ = std::move(token_opt.value());
      zx_status_t status = fdf_dispatcher_register_wake_vector(
          fdf_dispatcher_get_current_dispatcher(), token_.get(), ZX_USER_SIGNAL_0);
      if (status == ZX_OK) {
        fdf::info("Registered wake vector successfully!");
        ArmWait();
        ArmNonWakeWait();
      } else {
        fdf::error("Failed to register wake vector: {}", status);
      }
    }

    return zx::ok();
  }

  void ArmNonWakeWait() {
    auto wait = std::make_unique<async::WaitOnce>(token_.get(), ZX_USER_SIGNAL_1);
    auto* wait_ptr = wait.get();
    zx_status_t status = wait_ptr->Begin(
        dispatcher_,
        [this, wait = std::move(wait)](async_dispatcher_t* d, async::WaitOnce* w,
                                       zx_status_t status, const zx_packet_signal_t* signal) {
          if (status != ZX_OK) {
            fdf::info("ArmNonWakeWait callback: wait cancelled or failed: {}", status);
            return;
          }
          fdf::info("Dispatcher power test non-wake wait triggered!");
          if (test_ctrl_.is_valid()) {
            auto result = test_ctrl_->ReportNonWakeWaitTriggered();
            if (result.is_error()) {
              fdf::error("ReportNonWakeWaitTriggered failed: {}",
                         result.error_value().FormatDescription());
            }
          }
          token_.signal(ZX_USER_SIGNAL_1, 0);
          ArmNonWakeWait();
        });
    ZX_ASSERT(status == ZX_OK);
  }

  void ArmWait() {
    auto wait = std::make_unique<async::WaitOnce>(token_.get(), ZX_USER_SIGNAL_0);
    auto* wait_ptr = wait.get();
    zx_status_t status = wait_ptr->Begin(
        dispatcher_,
        [this, wait = std::move(wait)](async_dispatcher_t* d, async::WaitOnce* w,
                                       zx_status_t status, const zx_packet_signal_t* signal) {
          if (status != ZX_OK) {
            fdf::info("ArmWait callback: wait cancelled or failed: {}", status);
            return;
          }
          fdf::info("Dispatcher power test wake vector triggered!");
          if (test_ctrl_.is_valid()) {
            auto result = test_ctrl_->ReportWakeVectorTriggered();
            if (result.is_error()) {
              fdf::error("ReportWakeVectorTriggered failed: {}",
                         result.error_value().FormatDescription());
            }
          }
          token_.signal(ZX_USER_SIGNAL_0, 0);
          ArmWait();
        });
    if (status != ZX_OK) {
      fdf::error("ArmWait: Begin failed with status: {}", status);
    }
    ZX_ASSERT(status == ZX_OK);
  }

  void ScheduleNextTask() {
    fdf::info("ScheduleNextTask called!");
    async::PostDelayedTask(
        dispatcher_,
        [this] {
          fdf::info("Dispatcher power test recurring task running.");
          if (test_ctrl_.is_valid()) {
            auto result = test_ctrl_->ReportRecurringTaskRun();
            if (result.is_error()) {
              fdf::error("ReportRecurringTaskRun failed: {}",
                         result.error_value().FormatDescription());
            }
          }
          ScheduleNextTask();
        },
        zx::msec(100));
  }

  void SystemSuspend(fdf::SuspendCompleter completer) override {
    fdf::info("Dispatcher power test driver SystemSuspend called!");
    completer(zx::ok());
  }

  void SystemResume(std::optional<fuchsia_power_broker::LeaseToken> pe_lease,
                    fdf::ResumeCompleter completer) override {
    fdf::info("Dispatcher power test driver SystemResume called! lease has value: {}",
              pe_lease.has_value());
    if (test_ctrl_.is_valid()) {
      auto result = test_ctrl_->ReportSystemResume({{.has_lease = pe_lease.has_value()}});
      if (result.is_error()) {
        fdf::error("ReportSystemResume failed: {}", result.error_value().FormatDescription());
      }
    }
    pe_lease_ = std::move(pe_lease);
    completer(zx::ok());
  }

  void Stop(fdf::StopCompleter completer) override {
    fdf::info("Dispatcher power test driver stopped!");
    completer(zx::ok());
  }

 private:
  async_dispatcher_t* dispatcher_;
  fidl::Client<test_power_dispatcher::TestController> test_ctrl_;
  std::optional<fuchsia_power_broker::LeaseToken> pe_lease_;
  zx::event token_;
};
}  // namespace

FUCHSIA_DRIVER_EXPORT2(Driver);
