// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/profile_provider.h"

#include <lib/syslog/cpp/macros.h>

#include "src/media/audio/audio_core/reporter.h"

namespace media::audio {

fidl::InterfaceRequestHandler<fuchsia::media::ProfileProvider>
ProfileProvider::GetFidlRequestHandler() {
  return bindings_.GetHandler(this);
}

void ProfileProvider::RegisterHandlerWithCapacity(zx::thread thread_handle,
                                                  const std::string role_name, int64_t period,
                                                  float utilization,
                                                  RegisterHandlerWithCapacityCallback callback) {
  if (!role_manager_) {
    role_manager_ = context_.svc()->Connect<fuchsia::scheduler::RoleManager>();
  }

  const zx::duration interval = period ? zx::duration(period) : mix_profile_period_;
  const float scaled_interval = static_cast<float>(interval.to_nsecs()) * utilization;
  const zx::duration capacity(static_cast<zx_duration_t>(scaled_interval));

  auto request = std::move(
      fuchsia::scheduler::RoleManagerSetRoleRequest()
          .set_target(fuchsia::scheduler::RoleTarget::WithThread(std::move(thread_handle)))
          .set_role(fuchsia::scheduler::RoleName{role_name}));

  role_manager_->SetRole(
      std::move(request), [interval, capacity, callback = std::move(callback),
                           role_name](fuchsia::scheduler::RoleManager_SetRole_Result result) {
        if (result.is_response()) {
          callback(interval.get(), capacity.get());
          return;
        }
        if (result.is_err()) {
          // Failing to apply a Scheduler Profile is not fatal (e.g. it may happen in tests),
          // but we warn because performance may suffer.
          FX_PLOGS(WARNING, result.err()) << "Failed to set thread role '" << role_name << "'";
          Reporter::Singleton().FailedToApplySchedulerProfile(role_name, result.err());
        } else {
          // This should never happen (unknown method call or invalid fidl message tag).
          FX_LOGS(ERROR) << "Unknown response when setting thread role '" << role_name << "'";
        }
        callback(0, 0);
      });
}

void ProfileProvider::UnregisterHandler(zx::thread thread_handle, const std::string role_name,
                                        UnregisterHandlerCallback callback) {
  if (!role_manager_) {
    role_manager_ = context_.svc()->Connect<fuchsia::scheduler::RoleManager>();
  }

  const std::string role_name_for_unset = "fuchsia.default";
  auto request = std::move(
      fuchsia::scheduler::RoleManagerSetRoleRequest()
          .set_target(fuchsia::scheduler::RoleTarget::WithThread(std::move(thread_handle)))
          .set_role(fuchsia::scheduler::RoleName{role_name_for_unset}));

  role_manager_->SetRole(
      std::move(request), [callback = std::move(callback), role_name, role_name_for_unset](
                              fuchsia::scheduler::RoleManager_SetRole_Result result) {
        if (result.is_err()) {
          FX_PLOGS(WARNING, result.err()) << "Failed to unset thread role '" << role_name << "'";
          Reporter::Singleton().FailedToApplySchedulerProfile(role_name_for_unset, result.err());
        }
        callback();
      });
}

void ProfileProvider::RegisterMemoryRange(zx::vmar vmar_handle, std::string role_name,
                                          RegisterMemoryRangeCallback callback) {
  if (!role_manager_) {
    role_manager_ = context_.svc()->Connect<fuchsia::scheduler::RoleManager>();
  }

  auto request =
      std::move(fuchsia::scheduler::RoleManagerSetRoleRequest()
                    .set_target(fuchsia::scheduler::RoleTarget::WithVmar(std::move(vmar_handle)))
                    .set_role(fuchsia::scheduler::RoleName{role_name}));

  role_manager_->SetRole(
      std::move(request), [callback = std::move(callback), role_name = std::move(role_name)](
                              fuchsia::scheduler::RoleManager_SetRole_Result result) {
        if (result.is_err()) {
          // Failing to apply a Memory Profile is not fatal (e.g. it may happen in tests),
          // but we warn because performance may suffer.
          FX_PLOGS(WARNING, result.err()) << "Failed to set memory role '" << role_name << "'";
          Reporter::Singleton().FailedToApplyMemoryProfile(role_name, result.err());
        }
        callback();
      });
}

void ProfileProvider::UnregisterMemoryRange(zx::vmar vmar_handle,
                                            UnregisterMemoryRangeCallback callback) {
  if (!role_manager_) {
    role_manager_ = context_.svc()->Connect<fuchsia::scheduler::RoleManager>();
  }

  const std::string role_name_for_unset = "fuchsia.default";
  auto request =
      std::move(fuchsia::scheduler::RoleManagerSetRoleRequest()
                    .set_target(fuchsia::scheduler::RoleTarget::WithVmar(std::move(vmar_handle)))
                    .set_role(fuchsia::scheduler::RoleName{role_name_for_unset}));

  role_manager_->SetRole(
      std::move(request), [callback = std::move(callback), role_name_for_unset](
                              fuchsia::scheduler::RoleManager_SetRole_Result result) {
        if (result.is_err()) {
          FX_PLOGS(WARNING, result.err()) << "Failed to unset memory role";
          Reporter::Singleton().FailedToApplyMemoryProfile(role_name_for_unset, result.err());
        }
        callback();
      });
}

}  // namespace media::audio
