// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/power/power_manager.h"

#include <unordered_set>

#include "src/devices/bin/driver_manager/node.h"
#include "src/devices/bin/driver_manager/power/all_drivers_element.h"
#include "src/devices/lib/log/log.h"

namespace driver_manager {

namespace {

fuchsia_power_broker::ElementSchema CreateElementSchema(
    std::string_view name, fuchsia_power_broker::PowerLevel initial_level,
    std::vector<fuchsia_power_broker::PowerLevel> valid_levels,
    fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor_channel,
    fidl::ServerEnd<fuchsia_power_broker::ElementControl> element_control,
    fidl::ClientEnd<fuchsia_power_broker::ElementRunner> element_runner,
    std::vector<fuchsia_power_broker::LevelDependency> dependencies = {},
    std::optional<zx::eventpair> initial_lease_token = std::nullopt) {
  return fuchsia_power_broker::ElementSchema{{
      .element_name = std::string(name),
      .initial_current_level = initial_level,
      .valid_levels = std::move(valid_levels),
      .dependencies = std::move(dependencies),
      .lessor_channel = std::move(lessor_channel),
      .element_control = std::move(element_control),
      .element_runner = std::move(element_runner),
      .initial_lease_token = std::move(initial_lease_token),
  }};
}

fuchsia_power_broker::LevelDependency CreateLevelDependency(
    fuchsia_power_broker::PowerLevel dependent_level,
    fuchsia_power_broker::DependencyToken requires_token,
    std::vector<fuchsia_power_broker::PowerLevel> requires_level_by_preference) {
  return fuchsia_power_broker::LevelDependency{{
      .dependent_level = dependent_level,
      .requires_token = std::move(requires_token),
      .requires_level_by_preference = std::move(requires_level_by_preference),
  }};
}

}  // namespace

PowerManager::PowerManager(
    async_dispatcher_t* dispatcher, fidl::ClientEnd<fuchsia_power_broker::Topology> power_topology,
    std::optional<fidl::ClientEnd<fuchsia_power_system::CpuElementManager>> cpu_element_mgr,
    bool wait_for_storage_token)
    : dispatcher_(dispatcher),
      power_topology_(power_topology.is_valid() ? fidl::Client<fuchsia_power_broker::Topology>(
                                                      std::move(power_topology), dispatcher)
                                                : fidl::Client<fuchsia_power_broker::Topology>()),
      cpu_element_client_(
          cpu_element_mgr.has_value()
              ? std::make_optional(fidl::Client<fuchsia_power_system::CpuElementManager>(
                    std::move(cpu_element_mgr.value()), dispatcher))
              : std::nullopt),
      wait_for_storage_token_from_driver_(wait_for_storage_token) {}

void PowerManager::FetchCpuToken() {
  if (cpu_element_client_.has_value() &&
      !std::holds_alternative<PowerDependencyToken>(cpu_callbacks_or_token_)) {
    cpu_element_client_.value()->GetCpuDependencyToken().Then(
        [this](fidl::Result<fuchsia_power_system::CpuElementManager::GetCpuDependencyToken>&
                   result) mutable {
          ZX_ASSERT_MSG(result.is_ok(), "Error getting CPU token %s",
                        result.error_value().FormatDescription().c_str());

          CallbackSet callbacks = std::move(std::get<CallbackSet>(cpu_callbacks_or_token_));
          cpu_callbacks_or_token_ = std::move(result->assertive_dependency_token().value());
          for (auto& callback : callbacks) {
            callback();
          }
        });
  }
}

void PowerManager::PublishCpuElementManager(component::OutgoingDirectory& outgoing) {
  zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_power_system::CpuElementManager>(
      cpu_element_server_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());
}

void PowerManager::CreateStoragePowerElement(fuchsia_power_broker::DependencyToken driver_token,
                                             fuchsia_power_broker::PowerLevel power_level,
                                             fit::callback<void()> post_creation) {
  std::get<CallbackSet>(storage_callbacks_or_token_).push_back(std::move(post_creation));

  // Make a storage token.
  zx::event storage_token;
  ZX_ASSERT_MSG(zx::event::create(0, &storage_token) == ZX_OK, "Failure creating storage token");

  // Create a duplicate of the token that we can send to register with power broker.
  zx::event token_copy;
  ZX_ASSERT_MSG(storage_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy) == ZX_OK,
                "Duplication of storage token failed.");

  // Create the element schema. In a future change the schema will have a dependency on a power
  // element supplied to us by the storage driver.
  auto [lessor_client, lessor_server] = fidl::Endpoints<fuchsia_power_broker::Lessor>::Create();
  auto [element_control_client, element_control_server] =
      fidl::Endpoints<fuchsia_power_broker::ElementControl>::Create();
  auto [element_runner_client, element_runner_server] =
      fidl::Endpoints<fuchsia_power_broker::ElementRunner>::Create();

  fuchsia_power_broker::ElementSchema schema = CreateElementSchema(
      "DF-Storage", /* initial_current_level */ 1, {0, 1}, std::move(lessor_server),
      std::move(element_control_server), std::move(element_runner_client));

  fidl::Client<fuchsia_power_broker::ElementControl> storage_control =
      fidl::Client<fuchsia_power_broker::ElementControl>(std::move(element_control_client),
                                                         dispatcher_);

  // We create the request even before we pass the server end of the channel to power broker.
  storage_control
      ->RegisterDependencyToken(
          {{.token = fuchsia_power_broker::DependencyToken{std::move(storage_token)}}})
      .Then([this, token = std::move(token_copy)](
                fidl::Result<fuchsia_power_broker::ElementControl::RegisterDependencyToken>
                    result) mutable {
        if (result.is_error() && result.error_value().is_framework_error()) {
          fdf_log::error(" Could not register dependency token, FIDL error: {}",
                         result.error_value().FormatDescription());
        } else if (result.is_error()) {
          fdf_log::error("Could not register dependency token, protocol error: {}",
                         static_cast<uint32_t>(result.error_value().domain_error()));
        }

        ZX_ASSERT(result.is_ok());

        // Now that we have the storage token, run any driver creation callbacks which we deferred.
        auto after_storage_callbacks =
            std::move(std::get<CallbackSet>(storage_callbacks_or_token_));
        storage_callbacks_or_token_ = std::move(token);
        for (auto& cb : after_storage_callbacks) {
          cb();
        }
      });

  if (wait_for_storage_token_from_driver_) {
    ZX_ASSERT_MSG(driver_token.is_valid(), "Storage token required, but is invalid");
    std::vector<fuchsia_power_broker::LevelDependency> dep_on_storage_driver;
    dep_on_storage_driver.push_back(CreateLevelDependency(1, std::move(driver_token), {1}));
    schema.dependencies() = std::move(dep_on_storage_driver);
  }

  power_topology_->AddElement(std::move(schema))
      .Then([this, element_control_client = std::move(storage_control),
             runner_server = std::move(element_runner_server),
             lessor_client = std::move(lessor_client)](
                fidl::Result<fuchsia_power_broker::Topology::AddElement> add_result) mutable {
        if (add_result.is_error() && add_result.error_value().is_framework_error()) {
          fdf_log::error("Could not create storage power element, FIDL error: {}",
                         add_result.error_value().FormatDescription());
        } else if (add_result.is_error()) {
          fdf_log::error("Could not create storage power element, protocol error: ",
                         static_cast<uint32_t>(add_result.error_value().domain_error()));
        } else {
          storage_control_ = std::move(element_control_client);
          storage_element_runner_.AddBinding(this->dispatcher_, std::move(runner_server), this,
                                             fidl::kIgnoreBindingClosure);
          storage_lessor_ = std::move(lessor_client);
        }

        // If we're a power-enabled platform, it is an error for creation of the storage element to
        // fail.
        ZX_ASSERT(add_result.is_ok());
      });
}

std::optional<fuchsia_power_broker::DependencyToken> PowerManager::GetCpuToken(
    fit::callback<void()> callback_if_not_available) {
  PowerDependencyToken* cpu_token = std::get_if<PowerDependencyToken>(&cpu_callbacks_or_token_);
  if (!cpu_token) {
    if (callback_if_not_available) {
      std::get<CallbackSet>(cpu_callbacks_or_token_)
          .push_back(std::move(callback_if_not_available));
    }
    return std::nullopt;
  }
  zx::event clone;
  ZX_ASSERT(cpu_token->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone) == ZX_OK);
  return fuchsia_power_broker::DependencyToken(std::move(clone));
}

std::optional<fuchsia_power_broker::DependencyToken> PowerManager::GetStorageToken(
    fit::callback<void()> callback_if_not_available) {
  PowerDependencyToken* storage_token =
      std::get_if<PowerDependencyToken>(&storage_callbacks_or_token_);
  if (!storage_token) {
    if (callback_if_not_available) {
      std::get<CallbackSet>(storage_callbacks_or_token_)
          .push_back(std::move(callback_if_not_available));
    }
    return std::nullopt;
  }
  zx::event clone;
  ZX_ASSERT(storage_token->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone) == ZX_OK);
  return fuchsia_power_broker::DependencyToken(std::move(clone));
}

std::optional<fuchsia_power_broker::DependencyToken> PowerManager::GetStorageTokenIfAvailable() {
  PowerDependencyToken* storage_token =
      std::get_if<PowerDependencyToken>(&storage_callbacks_or_token_);
  if (!storage_token) {
    return std::nullopt;
  }
  zx::event clone;
  ZX_ASSERT(storage_token->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone) == ZX_OK);
  return fuchsia_power_broker::DependencyToken(std::move(clone));
}

fuchsia_power_broker::DependencyToken PowerManager::GetCpuTokenOrAssert() {
  PowerDependencyToken* cpu_token = std::get_if<PowerDependencyToken>(&cpu_callbacks_or_token_);
  ZX_ASSERT(cpu_token);
  zx::event clone;
  ZX_ASSERT(cpu_token->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone) == ZX_OK);
  return fuchsia_power_broker::DependencyToken(std::move(clone));
}

fuchsia_power_broker::DependencyToken PowerManager::GetStorageTokenOrAssert() {
  PowerDependencyToken* storage_token =
      std::get_if<PowerDependencyToken>(&storage_callbacks_or_token_);
  ZX_ASSERT(storage_token);
  zx::event clone;
  ZX_ASSERT(storage_token->duplicate(ZX_RIGHT_SAME_RIGHTS, &clone) == ZX_OK);
  return fuchsia_power_broker::DependencyToken(std::move(clone));
}

std::optional<fuchsia_power_broker::DependencyToken> PowerManager::StorageElementToken() {
  PowerDependencyToken* token = std::get_if<PowerDependencyToken>(&storage_callbacks_or_token_);
  ZX_ASSERT_MSG(token, "Invalid state, storage token requested before being set.");

  zx::event copy;
  ZX_ASSERT(token->duplicate(ZX_RIGHT_SAME_RIGHTS, &copy) == ZX_OK);
  return fuchsia_power_broker::DependencyToken(std::move(copy));
}

void PowerManager::SetLevel(SetLevelRequestView request, SetLevelCompleter::Sync& completer) {
  completer.Reply();
}

void PowerManager::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementRunner> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  std::string method_type;
  switch (metadata.unknown_method_type) {
    case fidl::UnknownMethodType::kOneWay:
      method_type = "one-way";
      break;
    case fidl::UnknownMethodType::kTwoWay:
      method_type = "two-way";
      break;
  };

  fdf_log::warn("PowerManager ElementRunner received unknown {} method. Ordinal: {}", method_type,
                metadata.method_ordinal);
}

void PowerManager::GetCpuDependencyToken(GetCpuDependencyTokenCompleter::Sync& completer) {
  if (!std::holds_alternative<PowerDependencyToken>(cpu_callbacks_or_token_)) {
    if (!cpu_element_client_.has_value()) {
      completer.Close(ZX_ERR_BAD_STATE);
      return;
    }

    std::get<CallbackSet>(cpu_callbacks_or_token_)
        .push_back([this, completer = completer.ToAsync()]() mutable {
          zx::event cpu_copy;

          zx_status_t dupe_result = std::get<PowerDependencyToken>(cpu_callbacks_or_token_)
                                        .duplicate(ZX_RIGHT_SAME_RIGHTS, &cpu_copy);
          if (dupe_result != ZX_OK) {
            completer.Close(dupe_result);
            return;
          }

          fidl::Arena arena;
          completer.Reply(fuchsia_power_system::wire::Cpu::Builder(arena)
                              .assertive_dependency_token(std::move(cpu_copy))
                              .Build());
        });
    return;
  }

  zx::event cpu_copy;

  zx_status_t dupe_result = std::get<PowerDependencyToken>(cpu_callbacks_or_token_)
                                .duplicate(ZX_RIGHT_SAME_RIGHTS, &cpu_copy);
  if (dupe_result != ZX_OK) {
    completer.Close(dupe_result);
    return;
  }

  fidl::Arena arena;
  completer.Reply(fuchsia_power_system::wire::Cpu::Builder(arena)
                      .assertive_dependency_token(std::move(cpu_copy))
                      .Build());
}

void PowerManager::AddExecutionStateDependency(
    AddExecutionStateDependencyRequestView request,
    AddExecutionStateDependencyCompleter::Sync& completer) {
  if (!request->has_dependency_token() || !request->has_power_level()) {
    completer.ReplyError(
        ::fuchsia_power_system::wire::AddExecutionStateDependencyError::kInvalidArgs);
    return;
  }

  if (!std::holds_alternative<CallbackSet>(storage_callbacks_or_token_)) {
    completer.ReplyError(fuchsia_power_system::wire::AddExecutionStateDependencyError::kBadState);
    return;
  }

  CreateStoragePowerElement(
      std::move(request->dependency_token()), request->power_level(),
      [completer = completer.ToAsync()]() mutable { completer.ReplySuccess(); });
}

void PowerManager::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_power_system::CpuElementManager> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

void PowerManager::AddCpuExecutionStateDependency(
    fuchsia_power_broker::DependencyToken dependency_token,
    fuchsia_power_broker::PowerLevel power_level) {
  if (!cpu_element_client_.has_value()) {
    return;
  }
  cpu_element_client_.value()
      ->AddExecutionStateDependency({{
          .dependency_token = std::move(dependency_token),
          .power_level = power_level,
      }})
      .Then([](fidl::Result<fuchsia_power_system::CpuElementManager::AddExecutionStateDependency>&
                   result) {
        if (result.is_error()) {
          fdf_log::error("Failure to register execution state dependency. {}",
                         result.error_value().FormatDescription());
        }
      });
}

void PowerManager::OnBootupComplete(std::shared_ptr<Node> root_node) {
  // We only want to create the AllDrivers power element if suspend is enabled.
  if (!SuspendEnabled()) {
    return;
  }

  // If we need to wait and the storage element token isn't created yet, delay creating all drivers
  // until it's created.
  if (wait_for_storage_token_from_driver_ &&
      !std::holds_alternative<PowerDependencyToken>(storage_callbacks_or_token_)) {
    std::get<CallbackSet>(storage_callbacks_or_token_).push_back([this, root_node]() {
      CreateAllDriversPowerElement(root_node);
    });
    return;
  }
  CreateAllDriversPowerElement(root_node);
}

void PowerManager::OnNodeBound(std::shared_ptr<const Node> node) {
  if (all_drivers_) {
    all_drivers_->OnNodeBound(std::move(node));
  }
}

void PowerManager::CreateAllDriversPowerElement(std::shared_ptr<Node> root_node) {
  ZX_ASSERT_MSG(SuspendEnabled(), "Suspend must be enabled to create AllDrivers power element");
  ZX_ASSERT_MSG(!all_drivers_, "AllDrivers power element already created");
  all_drivers_ = std::make_shared<AllDriversElement>(this, root_node);

  zx::event all_drivers_token;
  if (zx::event::create(0, &all_drivers_token) != ZX_OK) {
    fdf_log::error("Failed to create all driver token");
    all_drivers_.reset();
    return;
  }

  auto [lessor_client, lessor_server] = fidl::Endpoints<fuchsia_power_broker::Lessor>::Create();
  auto [element_control_client, element_control_server] =
      fidl::Endpoints<fuchsia_power_broker::ElementControl>::Create();
  auto [element_runner_client, element_runner_server] =
      fidl::Endpoints<fuchsia_power_broker::ElementRunner>::Create();

  std::vector<fuchsia_power_broker::LevelDependency> level_deps;
  // Add storage token.
  if (auto storage_token = GetStorageTokenIfAvailable()) {
    level_deps.emplace_back(CreateLevelDependency(1, std::move(*storage_token), {1}));
  }

  // Add CPU token.
  {
    level_deps.emplace_back(CreateLevelDependency(1, GetCpuTokenOrAssert(), {1}));
  }

  fuchsia_power_broker::ElementSchema schema = CreateElementSchema(
      "AllDrivers", /* initial_current_level */ 0, {0, 1}, std::move(lessor_server),
      std::move(element_control_server), std::move(element_runner_client), std::move(level_deps));

  fidl::Client<fuchsia_power_broker::ElementControl> element_control =
      fidl::Client<fuchsia_power_broker::ElementControl>(std::move(element_control_client),
                                                         dispatcher_);

  zx::event all_drivers_token_copy;
  if (all_drivers_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &all_drivers_token_copy) != ZX_OK) {
    fdf_log::error("Failed to duplicate driver token");
    all_drivers_.reset();
    return;
  }

  // Since the server-side of this channel hasn't been given to power broker yet, this request is
  // effectively queued and will get processed after we create the element.
  element_control->RegisterDependencyToken({std::move(all_drivers_token_copy)})
      .Then([this, all_drivers_token = std::move(all_drivers_token)](
                fidl::Result<fuchsia_power_broker::ElementControl::RegisterDependencyToken>
                    result) mutable {
        if (result.is_error()) {
          fdf_log::error("Failed to register dependency token for AllDrivers: {}",
                         result.error_value());
          all_drivers_.reset();
          return;
        }

        zx::event all_drivers_driver_runner_copy;
        zx_status_t dupe_result =
            all_drivers_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &all_drivers_driver_runner_copy);
        if (dupe_result != ZX_OK) {
          fdf_log::error("Failed to duplicate all drivers token: {}", dupe_result);
          all_drivers_.reset();
          return;
        }
        all_drivers_token_ = std::move(all_drivers_driver_runner_copy);

        // Hand the AllDrivers token to SAG.
        AddCpuExecutionStateDependency(std::move(all_drivers_token), 1);
      });

  power_topology_->AddElement(std::move(schema))
      .Then([this, element_control = std::move(element_control),
             runner_server = std::move(element_runner_server),
             lessor_client = std::move(lessor_client)](
                fidl::Result<fuchsia_power_broker::Topology::AddElement> add_result) mutable {
        if (add_result.is_error() && add_result.error_value().is_framework_error()) {
          fdf_log::error("Could not create AllDrivers power element, FIDL error: {}",
                         add_result.error_value().FormatDescription());
          all_drivers_.reset();
        } else if (add_result.is_error()) {
          fdf_log::error("Could not create AllDrivers power element, protocol error: {}",
                         static_cast<uint32_t>(add_result.error_value().domain_error()));
          all_drivers_.reset();
        } else {
          PowerElementHandles pe_handles{
              .element_control = std::move(element_control),
              .element_runner = std::move(runner_server),
              .lessor =
                  fidl::Client<fuchsia_power_broker::Lessor>(std::move(lessor_client), dispatcher_),
          };
          // Give the power element's ownership to the |all_drivers_| server.
          all_drivers_->AttachElement(dispatcher_, std::move(pe_handles));
        }
      });
}

void PowerManager::CreatePowerElement(
    std::optional<fidl::ClientEnd<fuchsia_power_broker::Topology>> topology_client,
    std::string_view name, fuchsia_power_broker::DependencyToken element_token,
    std::vector<fuchsia_power_broker::DependencyToken> deps,
    fidl::ServerEnd<fuchsia_power_broker::ElementControl> control,
    fidl::ClientEnd<fuchsia_power_broker::ElementRunner> runner,
    fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor, Collection for_collection,
    std::optional<fuchsia_power_broker::DependencyToken> cpu_token_override,
    std::optional<zx::eventpair> initial_lease_token, bool is_hermetic_power_test,
    fit::callback<void(zx::result<bool>)> cb) {
  if (!SuspendEnabled() && !topology_client.has_value()) {
    cb(zx::ok(false));
    return;
  }

  std::optional<fuchsia_power_broker::DependencyToken> final_cpu_token;
  if (cpu_token_override.has_value()) {
    fuchsia_power_broker::DependencyToken clone;
    zx_status_t dupe_result =
        cpu_token_override->duplicate(ZX_RIGHT_SAME_RIGHTS, (zx::event*)&clone);
    ZX_ASSERT(dupe_result == ZX_OK);
    final_cpu_token = std::move(clone);
  } else if (SuspendEnabled() && !is_hermetic_power_test) {
    auto cpu_token = GetCpuToken();
    if (!cpu_token.has_value()) {
      GetCpuToken([this, topology_client = std::move(topology_client), name = std::string(name),
                   element_token = std::move(element_token), deps = std::move(deps),
                   control = std::move(control), runner = std::move(runner),
                   lessor = std::move(lessor), for_collection,
                   cpu_token_override = std::move(cpu_token_override),
                   initial_lease_token = std::move(initial_lease_token), is_hermetic_power_test,
                   cb = std::move(cb)]() mutable {
        CreatePowerElement(std::move(topology_client), name, std::move(element_token),
                           std::move(deps), std::move(control), std::move(runner),
                           std::move(lessor), for_collection, std::move(cpu_token_override),
                           std::move(initial_lease_token), is_hermetic_power_test, std::move(cb));
      });
      return;
    }
    final_cpu_token = std::move(*cpu_token);
  }

  std::optional<fuchsia_power_broker::DependencyToken> final_storage_token;
  if (for_collection != Collection::kBoot && SuspendEnabled() && !is_hermetic_power_test) {
    auto storage_token = GetStorageToken();
    if (!storage_token.has_value()) {
      GetStorageToken([this, topology_client = std::move(topology_client), name = std::string(name),
                       element_token = std::move(element_token), deps = std::move(deps),
                       control = std::move(control), runner = std::move(runner),
                       lessor = std::move(lessor), for_collection,
                       cpu_token_override = std::move(cpu_token_override),
                       initial_lease_token = std::move(initial_lease_token), is_hermetic_power_test,
                       cb = std::move(cb)]() mutable {
        CreatePowerElement(std::move(topology_client), name, std::move(element_token),
                           std::move(deps), std::move(control), std::move(runner),
                           std::move(lessor), for_collection, std::move(cpu_token_override),
                           std::move(initial_lease_token), is_hermetic_power_test, std::move(cb));
      });
      return;
    }
    final_storage_token = std::move(*storage_token);
  }

  std::vector<fuchsia_power_broker::LevelDependency> level_deps;
  for (fuchsia_power_broker::DependencyToken& dep : deps) {
    fuchsia_power_broker::DependencyToken clone;
    zx_status_t dupe_result = dep.duplicate(ZX_RIGHT_SAME_RIGHTS, &clone);
    if (dupe_result != ZX_OK) {
      cb(zx::error(dupe_result));
      return;
    }

    level_deps.push_back(CreateLevelDependency(1, std::move(clone), {1}));
  }

  if (final_cpu_token.has_value()) {
    level_deps.push_back(CreateLevelDependency(1, std::move(final_cpu_token.value()), {1}));
  }

  if (final_storage_token.has_value()) {
    level_deps.push_back(CreateLevelDependency(1, std::move(final_storage_token.value()), {1}));
  }

  fuchsia_power_broker::ElementSchema schema = CreateElementSchema(
      name, /* initial_current_level */ 1, {0, 1}, std::move(lessor), std::move(control),
      std::move(runner), std::move(level_deps), std::move(initial_lease_token));

  fidl::Client<fuchsia_power_broker::Topology>* topology_to_use = &power_topology_;
  std::shared_ptr<fidl::Client<fuchsia_power_broker::Topology>> driver_specific_topology;
  if (topology_client.has_value()) {
    driver_specific_topology = std::make_shared<fidl::Client<fuchsia_power_broker::Topology>>(
        std::move(topology_client.value()), dispatcher_);
    topology_to_use = driver_specific_topology.get();
  }

  (*topology_to_use)
      ->AddElement(std::move(schema))
      .Then([cb = std::move(cb), topology_client = driver_specific_topology](
                fidl::Result<fuchsia_power_broker::Topology::AddElement>& add_result) mutable {
        if (add_result.is_error() && add_result.error_value().is_framework_error()) {
          cb(zx::error(add_result.error_value().framework_error().status()));
          return;
        }
        if (add_result.is_error()) {
          switch (add_result.error_value().domain_error()) {
            case fuchsia_power_broker::AddElementError::kInvalid:
              cb(zx::error(ZX_ERR_INVALID_ARGS));
              return;
            case fuchsia_power_broker::AddElementError::kNotAuthorized:
              cb(zx::error(ZX_ERR_ACCESS_DENIED));
              return;
            default:
              cb(zx::error(ZX_ERR_INTERNAL));
              return;
          }
        }
        cb(zx::ok(true));
      });
}

void PowerManager::LeaseAllDrivers(const std::shared_ptr<Node>& root_node,
                                   fit::callback<void()> callback) {
  if (!root_node || !SuspendEnabled()) {
    callback();
    return;
  }

  std::shared_ptr<fit::deferred_callback> deferred =
      std::make_shared<fit::deferred_callback>(std::move(callback));

  std::unordered_set<std::string> visited;
  // Lambda helper for in-place recursive traversal
  auto process_node = [this, &visited, &deferred](auto& self,
                                                  const std::shared_ptr<Node>& node) -> void {
    std::string topo_path = node->MakeTopologicalPath();
    if (visited.contains(topo_path)) {
      return;
    }
    visited.insert(topo_path);

    if (node->is_bound()) {
      AcquireRebootLease(node, std::move(topo_path), deferred);
    }
    for (const auto& child : node->children()) {
      self(self, child);
    }
  };
  process_node(process_node, root_node);
}

void PowerManager::AcquireRebootLease(const std::shared_ptr<Node>& node, std::string topo_path,
                                      std::shared_ptr<fit::deferred_callback> deferred) {
  zx::eventpair lease_token, lease_token_peer;
  if (zx_status_t status = zx::eventpair::create(0, &lease_token, &lease_token_peer);
      status != ZX_OK) {
    fdf_log::error("Failed to create lease token for node '{}': {}", topo_path,
                   zx_status_get_string(status));
    return;
  }

  fuchsia_power_broker::LeaseSchema schema;
  schema.lease_token(std::move(lease_token_peer));
  schema.lease_name(node->name());

  zx::event power_token = node->DuplicatePowerToken();
  if (!power_token.is_valid()) {
    fdf_log::error("Power token is invalid for node '{}'", topo_path);
    return;
  }
  std::vector<fuchsia_power_broker::LeaseDependency> deps;
  deps.push_back(fuchsia_power_broker::LeaseDependency{{
      .requires_token = std::move(power_token),
      .requires_level = 1,
  }});
  schema.dependencies(std::move(deps));

  power_topology_->Lease(std::move(schema))
      .Then([this, topo_path = std::move(topo_path), lease_token = std::move(lease_token),
             deferred = std::move(deferred)](
                fidl::Result<fuchsia_power_broker::Topology::Lease>& result) mutable {
        if (!result.is_ok()) {
          fdf_log::error("Failed to acquire reboot lease for node '{}': {}", topo_path,
                         result.error_value());
          return;
        }
        reboot_leases_.push_back(std::move(lease_token));
      });
}

}  // namespace driver_manager
