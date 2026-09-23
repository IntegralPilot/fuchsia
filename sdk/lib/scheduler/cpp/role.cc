// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/scheduler/role.h>
#include <lib/zx/result.h>
#include <zircon/availability.h>

#include <shared_mutex>
#include <variant>

#include <sdk/lib/component/incoming/cpp/protocol.h>

namespace {

using fuchsia_scheduler::wire::Parameter;
using fuchsia_scheduler::wire::ParameterValue;
using fuchsia_scheduler::wire::RoleName;
using fuchsia_scheduler::wire::RoleTarget;

class RoleClient {
 public:
  RoleClient() = default;
  zx::result<std::shared_ptr<fidl::WireSyncClient<fuchsia_scheduler::RoleManager>>> Connect();
  void Disconnect();

 private:
  // Keeps track of a synchronous connection to the RoleManager service.
  // This shared pointer is set to nullptr if the connection has been terminated.
  std::shared_ptr<fidl::WireSyncClient<fuchsia_scheduler::RoleManager>> role_manager_;
  std::shared_mutex mutex_;
};

zx::result<std::shared_ptr<fidl::WireSyncClient<fuchsia_scheduler::RoleManager>>>
RoleClient::Connect() {
  // Check if we already have a connection to the role manager, and return it if we do.
  // This requires acquiring the read lock.
  {
    std::shared_lock lock(mutex_);
    if (role_manager_ != nullptr) {
      return zx::ok(role_manager_);
    }
  }

  // At this point, we don't have an existing connection to the role manager, so we need to
  // create one. This requires the following set of operations to happen in order.
  // 1. Acquire the write lock. This will prevent concurrent connection attempts.
  // 2. Make sure that another thread didn't already connect the client by the time we got the write
  //    lock.
  // 3. Establish the connection.
  std::unique_lock lock(mutex_);
  if (role_manager_ != nullptr) {
    return zx::ok(role_manager_);
  }
  auto client_end_result = component::Connect<fuchsia_scheduler::RoleManager>();
  if (!client_end_result.is_ok()) {
    return client_end_result.take_error();
  }
  role_manager_ = std::make_shared<fidl::WireSyncClient<fuchsia_scheduler::RoleManager>>(
      fidl::WireSyncClient(std::move(*client_end_result)));
  return zx::ok(role_manager_);
}

void RoleClient::Disconnect() {
  std::unique_lock lock(mutex_);
  role_manager_ = nullptr;
}

// Stores a persistent connection to the RoleManager that reconnects on disconnect.
static RoleClient role_client{};

zx::result<std::vector<fuchsia_scheduler::RoleParameter>> SetRoleCommon(
    RoleTarget target, std::string_view role, const std::vector<Parameter>& input_parameters) {
// TODO(https://fxbug.dev/323262398): Remove this check once the necessary API is in the SDK.
#if FUCHSIA_API_LEVEL_LESS_THAN(HEAD)
  return zx::error(ZX_ERR_NOT_SUPPORTED);
#endif  // #if FUCHSIA_API_LEVEL_LESS_THAN(HEAD)
  zx::result client = role_client.Connect();
  if (!client.is_ok()) {
    return client.take_error();
  }

  fidl::Arena arena;
  auto builder = fuchsia_scheduler::wire::RoleManagerSetRoleRequest::Builder(arena);
  builder.target(std::move(target))
      .role(RoleName{fidl::StringView::FromExternal(role)})
      .input_parameters(input_parameters);

  fidl::WireResult result = (*(*client))->SetRole(builder.Build());
  if (!result.ok()) {
    // If the service closed the connection, disconnect the client. This will ensure that future
    // callers of SetRole reconnect to the RoleManager service.
    if (result.status() == ZX_ERR_PEER_CLOSED) {
      role_client.Disconnect();
    }
    return zx::error(result.status());
  }
  if (!result.value().is_ok()) {
    return result.value().take_error();
  }

  std::vector<fuchsia_scheduler::RoleParameter> out_params;
  if (!result->value()->has_output_parameters()) {
    return zx::ok(out_params);
  }
  out_params.reserve(result->value()->output_parameters().size());

  for (Parameter& param : result->value()->output_parameters()) {
    std::variant<double, int64_t, std::string> value;
    switch (param.value.Which()) {
      case ParameterValue::Tag::kIntValue:
        value = param.value.int_value();
        break;
      case ParameterValue::Tag::kFloatValue:
        value = param.value.float_value();
        break;
      case ParameterValue::Tag::kStringValue:
        value = std::string{param.value.string_value().get()};
        break;
      default:
        continue;
    }
    out_params.emplace_back(std::string{param.key.get()}, std::move(value));
  }

  return zx::ok(out_params);
}

}  // anonymous namespace

namespace fuchsia_scheduler {

zx::result<std::vector<RoleParameter>> SetRoleForVmarWithParams(
    zx::unowned_vmar borrowed_vmar, std::string_view role,
    const std::vector<wire::Parameter>& input_parameters) {
  zx::vmar vmar;
  zx_status_t status = borrowed_vmar->duplicate(ZX_RIGHT_SAME_RIGHTS, &vmar);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return SetRoleCommon(wire::RoleTarget::WithVmar(std::move(vmar)), role, input_parameters);
}

zx_status_t SetRoleForVmar(zx::unowned_vmar vmar, std::string_view role) {
  return SetRoleForVmarWithParams(vmar->borrow(), role, std::vector<wire::Parameter>())
      .status_value();
}

zx_status_t SetRoleForRootVmar(std::string_view role) {
  return SetRoleForVmar(zx::vmar::root_self(), role);
}

zx::result<std::vector<RoleParameter>> SetRoleForThreadWithParams(
    zx::unowned_thread borrowed_thread, std::string_view role,
    const std::vector<wire::Parameter>& input_parameters) {
  zx::thread thread;
  zx_status_t status = borrowed_thread->duplicate(ZX_RIGHT_SAME_RIGHTS, &thread);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return SetRoleCommon(wire::RoleTarget::WithThread(std::move(thread)), role, input_parameters);
}

zx_status_t SetRoleForThread(zx::unowned_thread thread, std::string_view role) {
  return SetRoleForThreadWithParams(thread->borrow(), role, std::vector<wire::Parameter>())
      .status_value();
}

zx::result<std::vector<RoleParameter>> SetRoleForThread(
    zx::unowned_thread borrowed_thread, std::string_view role,
    std::vector<RoleParameter>& input_parameters) {
  zx::thread thread;
  zx_status_t status = borrowed_thread->duplicate(ZX_RIGHT_SAME_RIGHTS, &thread);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  // FIDL ObjectViews hold a pointer to the data they view. This is fine for ints and floats, which
  // it can directly reference the basic types. These live as long as "input_parameters" do.
  // However, strings have nested views (fidl::ObjectView<fidl::StringView>). Since the object keeps
  // a pointer the the StringView, which in turn keeps a pointer to the string data, we need a way
  // to ensure that the string data lives as long as the wire::Parameter does.
  std::vector<fidl::StringView> string_views;
  std::vector<wire::Parameter> fidl_params;
  fidl_params.reserve(input_parameters.size());

  auto to_fidl_param = [&string_views](auto&& arg) mutable -> wire::ParameterValue {
    using T = std::decay_t<decltype(arg)>;
    if constexpr (std::is_same_v<T, int64_t>) {
      return wire::ParameterValue::WithIntValue(fidl::ObjectView<int64_t>::FromExternal(&arg));
    } else if constexpr (std::is_same_v<T, double>) {
      return wire::ParameterValue::WithFloatValue(fidl::ObjectView<double>::FromExternal(&arg));
    } else if constexpr (std::is_same_v<T, std::string>) {
      fidl::StringView& view = string_views.emplace_back(fidl::StringView::FromExternal(arg));
      return wire::ParameterValue::WithStringValue(
          fidl::ObjectView<fidl::StringView>::FromExternal(&view));
    }
  };
  for (RoleParameter& param : input_parameters) {
    wire::ParameterValue val = std::visit(to_fidl_param, param.value);
    fidl_params.emplace_back(fidl::StringView::FromExternal(param.name), val);
  }
  return SetRoleCommon(wire::RoleTarget::WithThread(std::move(thread)), role, fidl_params);
}

zx_status_t SetRoleForThisThread(std::string_view role) {
  return SetRoleForThread(zx::thread::self()->borrow(), role);
}

zx::result<std::vector<RoleParameter>> SetRoleForThisThread(
    std::string_view role, std::vector<RoleParameter>& input_parameters) {
  return SetRoleForThread(zx::thread::self()->borrow(), role, input_parameters);
}

}  // namespace fuchsia_scheduler
