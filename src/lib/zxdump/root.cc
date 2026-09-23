// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/zxdump/task.h>

#include <array>
#include <string_view>

namespace zxdump {
namespace {

template <class Service, auto Member>
fit::result<Error, LiveHandle> GetFromService() {
  constexpr const char* kSvcName = fidl::DiscoverableProtocolDefaultPath<Service>;

  static constexpr auto kGetName = []() {
    constexpr std::string_view kProtocolName = fidl::DiscoverableProtocolName<Service>;
    constexpr std::string_view kSuffix{".Get"};
    std::array<char, kProtocolName.size() + kSuffix.size()> name{};
    std::span<char> out{name};
    for (std::string_view str : {kProtocolName, kSuffix}) {
      out = out.subspan(str.copy(out.data(), out.size()));
    }
    return name;
  }();

  constexpr std::string_view kCallName{kGetName.data(), kGetName.size()};

  fidl::ClientEnd<Service> client;
  if (auto result = component::Connect<Service>(); result.is_error()) {
    return fit::error{Error{kSvcName, result.status_value()}};
  } else {
    client = std::move(result).value();
  }

  auto result = fidl::WireSyncClient(std::move(client))->Get();
  if (result.ok()) {
    return fit::ok(std::move(result.value().*Member));
  }
  return fit::error{Error{kCallName, result.status()}};
}

}  // namespace

fit::result<Error, LiveHandle> GetRootJob() {
  return GetFromService<fuchsia_kernel::RootJob,
                        &fidl::WireResponse<fuchsia_kernel::RootJob::Get>::job>();
}

fit::result<Error, LiveHandle> GetInfoResource() {
  return GetFromService<fuchsia_kernel::InfoResource,
                        &fidl::WireResponse<fuchsia_kernel::InfoResource::Get>::resource>();
}

}  // namespace zxdump
