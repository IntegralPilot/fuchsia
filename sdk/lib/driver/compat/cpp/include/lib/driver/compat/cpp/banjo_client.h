// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef LIB_DRIVER_COMPAT_CPP_BANJO_CLIENT_H_
#define LIB_DRIVER_COMPAT_CPP_BANJO_CLIENT_H_

#include <fidl/fuchsia.driver.compat/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/driver/async-helpers/cpp/async_task.h>
#include <lib/driver/incoming/cpp/namespace.h>

namespace compat {

namespace internal {

uint64_t GetKoid();

template <typename Client>
zx::result<Client> OnResult(
    fidl::BaseWireResult<fuchsia_driver_compat::Device::GetBanjoProtocol>& result) {
  if (!result.ok()) {
    return zx::error(result.status());
  }

  if (result.value().is_error()) {
    return zx::error(result.value().error_value());
  }

  typename Client::Proto proto = {};
  proto.ctx = reinterpret_cast<void*>(result.value().value()->context);
  using OpsType = decltype(Client::Proto::ops);
  proto.ops = reinterpret_cast<OpsType>(result.value().value()->ops);
  Client client(&proto);
  if (!client.is_valid()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(client));
}

}  // namespace internal

// Connect Asynchronously to a banjo protocol. Returns a cancellation token that can be added
// to a |fdf::async_helpers::WorkTracker| with its |TrackWorkCompletion|.
template <typename Client>
fdf::async_helpers::AsyncTask ConnectBanjo(const std::shared_ptr<fdf::Namespace>& incoming,
                                           fit::callback<void(zx::result<Client>)> callback,
                                           std::string_view parent_name = "default") {
  zx::result compat_parent = incoming->Connect<fuchsia_driver_compat::Service::Device>(parent_name);
  if (compat_parent.is_error()) {
    callback(compat_parent.take_error());
    return fdf::async_helpers::AsyncTask(true);
  }

  static uint64_t process_koid = internal::GetKoid();

  fidl::WireClient<fuchsia_driver_compat::Device> client(
      std::move(compat_parent.value()), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  fdf::async_helpers::AsyncTask task;
  client->GetBanjoProtocol(Client::kProtocolId, process_koid)
      .Then([cb = std::move(callback), completer = task.CreateCompleter()](
                fidl::WireUnownedResult<fuchsia_driver_compat::Device::GetBanjoProtocol>&
                    result) mutable { cb(internal::OnResult<Client>(result)); });

  task.SetItem(std::move(client));
  return task;
}

// Connect Synchronously to the banjo protocol.
template <typename Client>
zx::result<Client> ConnectBanjo(const std::shared_ptr<fdf::Namespace>& incoming,
                                std::string_view parent_name = "default") {
  zx::result compat_parent = incoming->Connect<fuchsia_driver_compat::Service::Device>(parent_name);
  if (compat_parent.is_error()) {
    return compat_parent.take_error();
  }

  static uint64_t process_koid = internal::GetKoid();

  fidl::WireResult<fuchsia_driver_compat::Device::GetBanjoProtocol> result =
      fidl::WireCall(compat_parent.value())->GetBanjoProtocol(Client::kProtocolId, process_koid);

  return internal::OnResult<Client>(result);
}

}  // namespace compat

#endif  // LIB_DRIVER_COMPAT_CPP_BANJO_CLIENT_H_
