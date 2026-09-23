// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include "src/zircon/bin/role_manager/src/config.h"
#include "zircon/syscalls/profile.h"

namespace {

constexpr char kConfigPath[] = "/pkg/profiles";

using zircon_profile::ConfiguredProfiles;
using zircon_profile::Role;

// Provides a fake implementation of the RoleManager protocol.
// This implementation parses profiles out of /pkg/profiles, but does not actually create zircon
// profile objects for each one. Therefore, SetRole calls do not set any actual profiles, but do
// check if the profile exists and return the corresponding output parameters.
class FakeRoleManager : public fidl::WireServer<fuchsia_scheduler::RoleManager> {
 public:
  static zx::result<std::unique_ptr<FakeRoleManager>> Create();
  void SetRole(SetRoleRequestView request, SetRoleCompleter::Sync& completer) override;
  void GetProfileForRole(GetProfileForRoleRequestView request,
                         GetProfileForRoleCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_scheduler::RoleManager> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  FakeRoleManager(ConfiguredProfiles profiles) : profiles_(std::move(profiles)) {}
  ConfiguredProfiles profiles_;
};

zx::result<std::unique_ptr<FakeRoleManager>> FakeRoleManager::Create() {
  auto config_result = zircon_profile::LoadConfigs(kConfigPath);
  if (config_result.is_error()) {
    FX_LOG_KV(ERROR, "Failed to load configs", FX_KV("error", config_result.error_value()),
              FX_KV("tag", "FakeRoleManager"));
    return zx::error(ZX_ERR_INTERNAL);
  }
  return zx::ok(
      std::unique_ptr<FakeRoleManager>(new FakeRoleManager{std::move(config_result.value())}));
}

void FakeRoleManager::SetRole(SetRoleRequestView request, SetRoleCompleter::Sync& completer) {
  const std::string_view role_name{request->role().role.get()};
  if (!request->target().is_thread() && !request->target().is_vmar()) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  std::vector<fuchsia_scheduler::Parameter> input_params = {};
  if (request->has_input_parameters()) {
    std::optional<std::vector<fuchsia_scheduler::Parameter>> maybe_input_params =
        fidl::ToNatural(request->input_parameters());
    if (!maybe_input_params.has_value()) {
      FX_LOG_KV(WARNING, "Unable to take ownership of input parameters.", FX_KV("role", role_name),
                FX_KV("tag", "FakeRoleManager"));
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
    input_params = maybe_input_params.value();
  }

  const fit::result role = Role::Create(role_name, input_params);
  if (role.is_error()) {
    completer.ReplyError(role.error_value());
    return;
  }

  const auto& profile_map = request->target().is_thread() ? profiles_.thread : profiles_.memory;

  // Look for the requested role in the profile map and set the output parameters if found.
  fidl::Arena arena;
  auto builder = fuchsia_scheduler::wire::RoleManagerSetRoleResponse::Builder(arena);
  if (auto search = profile_map.find(*role); search != profile_map.cend()) {
    builder.output_parameters(fidl::ToWire(arena, search->second.output_parameters));
    completer.ReplySuccess(builder.Build());
    return;
  }

  FX_LOG_KV(DEBUG, "Requested role not found", FX_KV("role", role->name()),
            FX_KV("tag", "FakeRoleManager"));
  completer.ReplyError(ZX_ERR_NOT_FOUND);
}

void FakeRoleManager::GetProfileForRole(GetProfileForRoleRequestView request,
                                        GetProfileForRoleCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_FOUND);
}

void FakeRoleManager::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_scheduler::RoleManager> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOG_KV(ERROR, "Got request to handle unknown method", FX_KV("tag", "FakeRoleManager"));
}

}  // namespace

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  zx::result create_result = FakeRoleManager::Create();
  if (create_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to create fake role manager service: "
                   << create_result.status_string();
    return -1;
  }
  std::unique_ptr<FakeRoleManager> fake_role_manager_service = std::move(create_result.value());

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);
  zx::result result =
      outgoing.AddProtocol<fuchsia_scheduler::RoleManager>(std::move(fake_role_manager_service));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add RoleManager protocol: " << result.status_string();
    return -1;
  }

  result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }
  FX_LOGS(INFO) << "Starting fake role manager\n";
  zx_status_t status = loop.Run();
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to run async loop: " << zx_status_get_string(status);
  }
  return 0;
}
