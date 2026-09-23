// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "role.h"

#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/profile.h>
#include <lib/zx/thread.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/profile.h>

#include "resource.h"

using zircon_profile::Role;

constexpr char kConfigPath[] = "/config/profiles";

zx::result<std::unique_ptr<RoleManager>> RoleManager::Create(inspect::Node& parent) {
  auto profile_resource_result = GetSystemProfileResource();
  if (profile_resource_result.is_error()) {
    FX_LOGS(ERROR) << "failed to get profile resource: " << profile_resource_result.status_string();
    return profile_resource_result.take_error();
  }
  zx::resource profile_resource = std::move(profile_resource_result.value());

  auto config_result = zircon_profile::LoadConfigs(kConfigPath);
  if (config_result.is_error()) {
    FX_LOG_KV(ERROR, "Failed to load configs", FX_KV("error", config_result.error_value()),
              FX_KV("tag", "RoleManager"));
    return zx::error(ZX_ERR_INTERNAL);
  }

  auto create = [&profile_resource](zircon_profile::ProfileMap& profiles) {
    // Create profiles for each configured role. If creating the profile fails, remove the role
    // entry.
    for (auto iter = profiles.begin(); iter != profiles.end();) {
      const zx_status_t status =
          zx::profile::create(profile_resource, 0, &iter->second.info, &iter->second.profile);
      if (status != ZX_OK) {
        FX_LOG_KV(ERROR, "Failed to create profile for role. Requests for this role will fail.",
                  FX_KV("role", iter->first.name()), FX_KV("status", zx_status_get_string(status)));
        iter = profiles.erase(iter);
      } else {
        ++iter;
      }
    }
  };
  create(config_result->thread);
  create(config_result->memory);

  // Apply the dispatch role if defined.
  const std::string dispatch_role_name = "fuchsia.system.profile-provider.dispatch";
  const fit::result dispatch_role = Role::Create(dispatch_role_name);
  if (dispatch_role.is_error()) {
    FX_LOG_KV(ERROR, "Failed to parse dispatch role.",
              FX_KV("error", zx_status_get_string(dispatch_role.error_value())),
              FX_KV("tag", "ProfileProvider"));
  }
  const auto search = config_result->thread.find(*dispatch_role);
  if (search != config_result->thread.end()) {
    const zx_status_t status = zx::thread::self()->set_profile(search->second.profile, 0);
    if (status != ZX_OK) {
      FX_LOG_KV(ERROR, "Failed to set role", FX_KV("error", zx_status_get_string(status)),
                FX_KV("tag", "RoleManager"));
    }
  }

  auto manager = std::unique_ptr<RoleManager>(
      new RoleManager{std::move(profile_resource), std::move(config_result.value())});
  manager->PublishInspect(parent);
  return zx::ok(std::move(manager));
}

void RoleManager::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_scheduler::RoleManager> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOG_KV(ERROR, "Got request to handle unknown method", FX_KV("tag", "RoleManager"));
}

void RoleManager::LogRequest(SetRoleRequestView request) {
  // Return early if debug logging is not enabled. This allows us to bypass an extra
  // zx_object_get_info syscall.
  if (fuchsia_logging::GetMinLogSeverity() > FUCHSIA_LOG_DEBUG) {
    return;
  }

  zx_info_handle_basic_t handle_info{};
  const std::string_view role_name{request->role().role.get()};
  if (request->target().is_thread()) {
    zx_status_t status = request->target().thread().get_info(ZX_INFO_HANDLE_BASIC, &handle_info,
                                                             sizeof(handle_info), nullptr, nullptr);
    if (status != ZX_OK) {
      FX_LOG_KV(ERROR, "Failed to get info for thread: ", FX_KV("role", role_name),
                FX_KV("status", status), FX_KV("tag", "RoleManager"));
      return;
    }
    FX_LOG_KV(DEBUG, "Role requested for thread:", FX_KV("role", role_name),
              FX_KV("pid", handle_info.related_koid), FX_KV("tid", handle_info.koid),
              FX_KV("tag", "RoleManager"));
  } else if (request->target().is_vmar()) {
    zx_status_t status = request->target().vmar().get_info(ZX_INFO_HANDLE_BASIC, &handle_info,
                                                           sizeof(handle_info), nullptr, nullptr);
    if (status != ZX_OK) {
      FX_LOG_KV(ERROR, "Failed to get info for vmar: ", FX_KV("role", role_name),
                FX_KV("status", status), FX_KV("tag", "RoleManager"));
      return;
    }
    FX_LOG_KV(DEBUG, "Role requested for vmar:", FX_KV("role", role_name),
              FX_KV("pid", handle_info.related_koid), FX_KV("koid", handle_info.koid),
              FX_KV("tag", "RoleManager"));
  }
}

void RoleManager::SetRole(SetRoleRequestView request, SetRoleCompleter::Sync& completer) {
  const std::string_view role_name{request->role().role.get()};
  zx_handle_t target_handle = ZX_HANDLE_INVALID;
  if (request->target().is_thread()) {
    target_handle = request->target().thread().get();
  } else if (request->target().is_vmar()) {
    target_handle = request->target().vmar().get();
  } else {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  LogRequest(request);

  std::vector<fuchsia_scheduler::Parameter> input_params = {};
  if (request->has_input_parameters()) {
    std::optional<std::vector<fuchsia_scheduler::Parameter>> maybe_input_params =
        fidl::ToNatural(request->input_parameters());
    if (!maybe_input_params.has_value()) {
      FX_LOG_KV(WARNING, "Unable to take ownership of input parameters.", FX_KV("role", role_name),
                FX_KV("tag", "RoleManager"));
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

  auto& profile_map = request->target().is_thread() ? profiles_.thread : profiles_.memory;

  // Look for the requested role in the profile map and set the profile if found.
  fidl::Arena arena;
  auto builder = fuchsia_scheduler::wire::RoleManagerSetRoleResponse::Builder(arena);
  if (auto search = profile_map.find(*role); search != profile_map.cend()) {
    search->second.request_count++;

    zx_status_t status = zx_object_set_profile(target_handle, search->second.profile.get(), 0);
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    builder.output_parameters(fidl::ToWire(arena, search->second.output_parameters));
    completer.ReplySuccess(builder.Build());
    return;
  }

  FX_LOG_KV(DEBUG, "Requested role not found", FX_KV("role", role->name()),
            FX_KV("tag", "RoleManager"));
  completer.ReplyError(ZX_ERR_NOT_FOUND);
}

void RoleManager::PublishInspect(inspect::Node& parent) {
  parent.RecordLazyNode("config", [this] { return Inspect(); });
}

fpromise::promise<inspect::Inspector> RoleManager::Inspect() {
  inspect::Inspector inspector;
  inspector.GetRoot().RecordChild("thread_roles", [&](inspect::Node& node) {
    for (auto& [role, profile] : profiles_.thread) {
      node.RecordChild(role.name(),
                       [&](inspect::Node& role_node) { profile.PopulateInspect(role_node); });
    }
  });

  inspector.GetRoot().RecordChild("memory_roles", [&](inspect::Node& node) {
    for (auto& [role, profile] : profiles_.memory) {
      node.RecordChild(role.name(),
                       [&](inspect::Node& role_node) { profile.PopulateInspect(role_node); });
    }
  });

  return fpromise::make_ok_promise(inspector);
}

void RoleManager::GetProfileForRole(GetProfileForRoleRequestView request,
                                    GetProfileForRoleCompleter::Sync& completer) {
  const std::string_view role_name{request->role().role.get()};

  std::vector<fuchsia_scheduler::Parameter> input_params = {};
  if (request->has_input_parameters()) {
    std::optional<std::vector<fuchsia_scheduler::Parameter>> maybe_input_params =
        fidl::ToNatural(request->input_parameters());
    if (!maybe_input_params.has_value()) {
      FX_LOG_KV(WARNING, "Unable to take ownership of input parameters.", FX_KV("role", role_name),
                FX_KV("tag", "RoleManager"));
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

  // Look for the requested role in the profile map.
  if (!request->has_target()) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto& profile_map =
      request->target() == fuchsia_scheduler::RoleType::kTask ? profiles_.thread : profiles_.memory;
  if (auto search = profile_map.find(*role); search != profile_map.cend()) {
    search->second.request_count++;

    zx::profile profile;
    zx_status_t status = search->second.profile.duplicate(ZX_RIGHT_SAME_RIGHTS, &profile);
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }

    fidl::Arena arena;
    auto builder = fuchsia_scheduler::wire::RoleManagerGetProfileForRoleResponse::Builder(arena);
    builder.profile(std::move(profile));
    builder.output_parameters(fidl::ToWire(arena, search->second.output_parameters));
    completer.ReplySuccess(builder.Build());
    return;
  }

  FX_LOG_KV(DEBUG, "Requested role not found", FX_KV("role", role->name()),
            FX_KV("tag", "RoleManager"));
  completer.ReplyError(ZX_ERR_NOT_FOUND);
}
