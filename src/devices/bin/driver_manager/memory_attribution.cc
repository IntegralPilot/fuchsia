// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/memory_attribution.h"

#include <zircon/errors.h>

#include "src/devices/lib/log/log.h"

namespace driver_manager {

namespace fma = fuchsia_memory_attribution;

zx::result<> MemoryAttributor::Publish(component::OutgoingDirectory& outgoing) {
  return outgoing.AddUnmanagedProtocol<fma::Provider>(
      [this](fidl::ServerEnd<fma::Provider> request) {
        if (binding_.has_value()) {
          // Already bound.
          return;
        }
        binding_.emplace(dispatcher_, std::move(request), this,
                         [&](auto unbind_info) { binding_.reset(); });
      });
}

void MemoryAttributor::AddDriver(zx::event component_token, zx_koid_t id, zx_koid_t process_koid) {
  pending_updates_.emplace_back(fma::AttributionUpdate::WithAdd({{
      .identifier = id,
      .description = fma::Description::WithComponent(std::move(component_token)),
      .principal_type = fma::PrincipalType::kRunnable,
  }}));
  pending_updates_.emplace_back(fma::AttributionUpdate::WithUpdate({{
      .identifier = id,
      .resources = fma::Resources::WithData(fma::Data{{
          .resources = std::vector{fma::Resource::WithKernelObject(process_koid)},
      }}),
  }}));

  if (auto completer = std::exchange(pending_completer_, std::nullopt); completer.has_value()) {
    completer->Reply(zx::ok(fma::ProviderGetResponse{{
        .attributions = std::move(pending_updates_),
    }}));
  }
}

void MemoryAttributor::RemoveDriver(zx_koid_t id) {
  pending_updates_.emplace_back(fma::AttributionUpdate::WithRemove(id));

  if (auto completer = std::exchange(pending_completer_, std::nullopt); completer.has_value()) {
    completer->Reply(zx::ok(fma::ProviderGetResponse{{
        .attributions = std::move(pending_updates_),
    }}));
  }
}

void MemoryAttributor::Get(GetCompleter::Sync& completer) {
  if (pending_completer_) {
    completer.Reply(zx::error(ZX_ERR_ALREADY_BOUND));
    return;
  }
  if (auto updates = std::move(pending_updates_); !updates.empty()) {
    completer.Reply(zx::ok(fma::ProviderGetResponse{{
        .attributions = std::move(updates),
    }}));
    return;
  }
  pending_completer_ = completer.ToAsync();
}

void MemoryAttributor::handle_unknown_method(fidl::UnknownMethodMetadata<fma::Provider> metadata,
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

  fdf_log::warn("fuchsia.memory.attribution/Provider received unknown {} method. Ordinal: {}",
                method_type, metadata.method_ordinal);
}

}  // namespace driver_manager
