// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/profile_acquirer.h"

#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/channel.h>

#include <cstdlib>

#include <sdk/lib/component/incoming/cpp/protocol.h>

#include "src/media/audio/audio_core/mix_profile_config.h"
#include "src/media/audio/audio_core/reporter.h"

namespace media::audio {

namespace {

zx::result<fidl::SyncClient<fuchsia_scheduler::RoleManager>> ConnectToRoleManager() {
  auto client_end_result = component::Connect<fuchsia_scheduler::RoleManager>();
  if (!client_end_result.is_ok()) {
    return client_end_result.take_error();
  }
  return zx::ok(fidl::SyncClient(std::move(*client_end_result)));
}

zx::result<> ApplyProfile(fuchsia_scheduler::RoleTarget target, const std::string& role) {
  auto client = ConnectToRoleManager();
  if (!client.is_ok()) {
    // Failing to apply a Scheduler or Memory Profile is not fatal (e.g. it may happen in tests),
    // but we warn because performance may suffer.
    FX_PLOGS(WARNING, client.status_value())
        << "Failed to connect to fuchsia.scheduler.RoleManager";
    return zx::error(client.status_value());
  }

  auto request = std::move(fuchsia_scheduler::RoleManagerSetRoleRequest()
                               .target(std::move(target))
                               .role(fuchsia_scheduler::RoleName{role}));
  auto result = (*client)->SetRole(std::move(request));
  if (!result.is_ok()) {
    // Failing to apply a Scheduler or Memory Profile is not fatal (e.g. it may happen in tests),
    // but we warn because performance may suffer.
    FX_LOGS(WARNING) << "Failed to call SetRole, error=" << result.error_value();
    zx_status_t error_status;
    if (result.error_value().is_domain_error()) {
      error_status = result.error_value().domain_error();
    } else {
      error_status = result.error_value().framework_error().status();
    }
    return zx::error(error_status);
  }
  return zx::ok();
}

}  // namespace

zx::result<> AcquireSchedulerRole(zx::unowned_thread thread, const std::string& role) {
  TRACE_DURATION("audio", "AcquireSchedulerRole", "role", TA_STRING(role.c_str()));
  zx::thread duplicate;
  zx::result<> result;
  const zx_status_t dup_status = thread->duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate);
  if (dup_status != ZX_OK) {
    // This is unexpected, even on systems without RoleManager, so we err.
    FX_PLOGS(ERROR, dup_status) << "Failed to duplicate thread handle";
    result = zx::error(dup_status);
  } else {
    result = ApplyProfile(fuchsia_scheduler::RoleTarget::WithThread(std::move(duplicate)), role);
  }
  if (result.is_error()) {
    Reporter::Singleton().FailedToApplySchedulerProfile(role, result.error_value());
  }
  return result;
}

zx::result<> AcquireMemoryRole(zx::unowned_vmar vmar, const std::string& role) {
  TRACE_DURATION("audio", "AcquireMemoryRole", "role", TA_STRING(role.c_str()));
  zx::vmar duplicate;
  zx::result<> result;
  const zx_status_t dup_status = vmar->duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate);
  if (dup_status != ZX_OK) {
    // This is unexpected, even on systems without RoleManager, so we err.
    FX_PLOGS(ERROR, dup_status) << "Failed to duplicate vmar handle";
    result = zx::error(dup_status);
  } else {
    result = ApplyProfile(fuchsia_scheduler::RoleTarget::WithVmar(std::move(duplicate)), role);
  }
  if (result.is_error()) {
    Reporter::Singleton().FailedToApplyMemoryProfile(role, result.error_value());
  }
  return result;
}

}  // namespace media::audio
