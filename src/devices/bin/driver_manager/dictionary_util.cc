// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/dictionary_util.h"

#include <fidl/fuchsia.component.sandbox/cpp/common_types_format.h>
#include <fidl/fuchsia.component.sandbox/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/directory_watcher.h>
#include <lib/fdio/directory.h>

#include "src/devices/bin/driver_manager/async_sharder.h"
#include "src/devices/lib/log/log.h"

namespace driver_manager {

void DictionaryUtil::ImportDictionary(
    fuchsia_component_sandbox::DictionaryRef dictionary,
    fit::callback<void(zx::result<fuchsia_component_sandbox::NewCapabilityId>)> callback) {
  ImportDictionaryWire(
      fuchsia_component_sandbox::wire::DictionaryRef{.token = std::move(dictionary.token())},
      std::move(callback));
}

void DictionaryUtil::ImportDictionaryWire(
    fuchsia_component_sandbox::wire::DictionaryRef dictionary,
    fit::callback<void(zx::result<fuchsia_component_sandbox::wire::NewCapabilityId>)> callback) {
  fuchsia_component_sandbox::NewCapabilityId imported = cap_id_++;
  store_
      ->Import(imported,
               fuchsia_component_sandbox::wire::Capability::WithDictionary(std::move(dictionary)))
      .Then([imported, callback = std::move(callback)](
                fidl::WireUnownedResult<fuchsia_component_sandbox::CapabilityStore::Import>&
                    result) mutable {
        if (!result.ok()) {
          fdf_log::warn("failed to call import dictionary ref {}", result.FormatDescription());
          callback(zx::error(result.status()));
          return;
        }
        if (result->is_error()) {
          fdf_log::warn("failed to import dictionary ref {}", result->error_value());
          callback(zx::error(ZX_ERR_INTERNAL));
          return;
        }

        callback(zx::ok(imported));
      });
}

void DictionaryUtil::CopyExportDictionary(
    fuchsia_component_sandbox::CapabilityId dictionary,
    fit::callback<void(zx::result<fuchsia_component_sandbox::DictionaryRef>)> callback) {
  uint64_t dest = cap_id_++;
  store_->DictionaryCopy(dictionary, dest)
      .Then([this, callback = std::move(callback), dest](
                fidl::WireUnownedResult<fuchsia_component_sandbox::CapabilityStore::DictionaryCopy>&
                    result) mutable {
        if (!result.ok() || result->is_error()) {
          fdf_log::error("Failed to copy dictionary. {}", result.FormatDescription());
          callback(zx::error(ZX_ERR_INTERNAL));
          return;
        }

        store_->Export(dest).Then(
            [callback = std::move(callback)](
                fidl::WireUnownedResult<fuchsia_component_sandbox::CapabilityStore::Export>&
                    result) mutable {
              if (!result.ok() || result->is_error()) {
                fdf_log::error("Failed to export dictionary. {}", result.FormatDescription());
                callback(zx::error(ZX_ERR_INTERNAL));
                return;
              }

              callback(zx::ok(fuchsia_component_sandbox::DictionaryRef{
                  std::move(result->value()->capability.dictionary().token)}));
            });
      });
}

void DictionaryUtil::DictionaryDirConnectorOpen(
    fuchsia_component_sandbox::CapabilityId dictionary, std::string_view key,
    fit::callback<void(zx::result<fidl::ClientEnd<fuchsia_io::Directory>>)> callback) {
  fuchsia_component_sandbox::NewCapabilityId dest = cap_id_++;
  store_->DictionaryGet(dictionary, fidl::StringView::FromExternal(key), dest)
      .Then([this, callback = std::move(callback), dest](
                fidl::WireUnownedResult<fuchsia_component_sandbox::CapabilityStore::DictionaryGet>&
                    result) mutable {
        if (!result.ok()) {
          fdf_log::error("Failed to get dictionary. {}", result.FormatDescription());
          callback(zx::error(ZX_ERR_INTERNAL));
          return;
        }

        if (result->is_error()) {
          fdf_log::error("Failed to get dictionary. {}", result.value().error_value());
          callback(zx::error(ZX_ERR_INTERNAL));
          return;
        }

        store_->Export(dest).Then(
            [this, callback = std::move(callback)](
                fidl::WireUnownedResult<fuchsia_component_sandbox::CapabilityStore::Export>&
                    result) mutable {
              if (!result.ok() || result->is_error()) {
                fdf_log::error("Failed to export dictionary. {}", result.FormatDescription());
                callback(zx::error(ZX_ERR_INTERNAL));
                return;
              }

              auto cap = result->value();
              if (cap->capability.is_dir_connector_router()) {
                fidl::WireClient<fuchsia_component_sandbox::DirConnectorRouter>
                    dir_connector_router_client(std::move(cap->capability.dir_connector_router()),
                                                dispatcher_);

                dir_connector_router_client->Route(fuchsia_component_sandbox::wire::RouteRequest{})
                    .Then([this, callback = std::move(callback),
                           dir_connector_router_client = std::move(dir_connector_router_client)](
                              fidl::WireUnownedResult<
                                  fuchsia_component_sandbox::DirConnectorRouter::Route>&
                                  result) mutable {
                      if (!result.ok() || result->is_error()) {
                        fdf_log::error("Failed to route dir connector. {}",
                                       result.FormatDescription());
                        callback(zx::error(ZX_ERR_INTERNAL));
                        return;
                      }

                      OpenDirConnector(std::move(result->value()->dir_connector()),
                                       std::move(callback));
                    });
              } else if (cap->capability.is_dir_connector()) {
                OpenDirConnector(std::move(cap->capability.dir_connector()), std::move(callback));
              } else {
                callback(zx::error(ZX_ERR_INTERNAL));
              }
            });
      });
}

void DictionaryUtil::CreateDictionaryWith(
    std::unordered_map<std::string, fidl::ClientEnd<fuchsia_component_sandbox::DirReceiver>>
        receivers,
    fit::callback<void(zx::result<fuchsia_component_sandbox::CapabilityId>)> callback) {
  fuchsia_component_sandbox::NewCapabilityId dest = cap_id_++;
  store_->DictionaryCreate(dest).Then(
      [this, callback = std::move(callback), dest, receivers = std::move(receivers)](
          fidl::WireUnownedResult<fuchsia_component_sandbox::CapabilityStore::DictionaryCreate>&
              result) mutable {
        if (!result.ok() || result->is_error()) {
          fdf_log::error("Failed to create dictionary. {}", result.FormatDescription());
          callback(zx::error(ZX_ERR_INTERNAL));
          return;
        }

        std::shared_ptr<AsyncSharder> sharder = std::make_shared<AsyncSharder>(
            receivers.size(), [callback = std::move(callback), dest](zx::result<> result) mutable {
              if (result.is_error()) {
                callback(result.take_error());
                return;
              }
              callback(zx::ok(dest));
            });

        for (auto& [key, receiver] : receivers) {
          fuchsia_component_sandbox::NewCapabilityId connector_dest = cap_id_++;
          store_->DirConnectorCreate(connector_dest, std::move(receiver))
              .Then(
                  [this, dest, key, connector_dest, sharder](
                      fidl::WireUnownedResult<
                          fuchsia_component_sandbox::CapabilityStore::DirConnectorCreate>& result) {
                    if (!result.ok() || result->is_error()) {
                      fdf_log::error("Failed to create dir connector. {}",
                                     result.FormatDescription());
                      sharder->CompleteShardError(ZX_ERR_INTERNAL);
                      return;
                    }

                    store_
                        ->DictionaryInsert(dest,
                                           fuchsia_component_sandbox::wire::DictionaryItem{
                                               .key = fidl::StringView::FromExternal(key),
                                               .value = connector_dest})
                        .Then(
                            [sharder](fidl::WireUnownedResult<
                                      fuchsia_component_sandbox::CapabilityStore::DictionaryInsert>&
                                          result) {
                              if (!result.ok() || result->is_error()) {
                                fdf_log::error("Failed to insert dictionary item. {}",
                                               result.FormatDescription());
                                sharder->CompleteShardError(ZX_ERR_INTERNAL);
                                return;
                              }

                              sharder->CompleteShard();
                            });
                  });
        }
      });
}

void DictionaryUtil::OpenDirConnector(
    fuchsia_component_sandbox::wire::DirConnector connector,
    fit::callback<void(zx::result<fidl::ClientEnd<fuchsia_io::Directory>>)> callback) {
  fuchsia_component_sandbox::NewCapabilityId imported = cap_id_++;
  store_
      ->Import(imported,
               fuchsia_component_sandbox::wire::Capability::WithDirConnector(std::move(connector)))
      .Then([this, callback = std::move(callback),
             imported](fidl::WireUnownedResult<fuchsia_component_sandbox::CapabilityStore::Import>&
                           result) mutable {
        if (!result.ok() || result->is_error()) {
          fdf_log::error("Failed to import dir connector. {}", result.FormatDescription());
          callback(zx::error(ZX_ERR_INTERNAL));
          return;
        }

        auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
        fidl::Arena arena;
        store_
            ->DirConnectorOpen(
                fuchsia_component_sandbox::wire::CapabilityStoreDirConnectorOpenRequest::Builder(
                    arena)
                    .id(imported)
                    .server_end(std::move(server))
                    .flags(fuchsia_io::Flags::kProtocolDirectory | fuchsia_io::kPermReadable)
                    .Build())
            .Then([callback = std::move(callback), client = std::move(client)](
                      fidl::WireUnownedResult<
                          fuchsia_component_sandbox::CapabilityStore::DirConnectorOpen>&
                          result) mutable {
              if (!result.ok() || result->is_error()) {
                fdf_log::error("Failed to open dir connector. {}", result.FormatDescription());
                callback(zx::error(ZX_ERR_INTERNAL));
                return;
              }

              callback(zx::ok(std::move(client)));
            });
      });
}

void DirReceiverImpl::Receive(ReceiveRequestView request, ReceiveCompleter::Sync& completer) {
  if (!request->has_subdir()) {
    fdf_log::error("no subdir");
    return;
  }

  if (request->subdir().get() == ".") {
    if (dir_infos_.size() != 1 || !dir_infos_[0].is_primary) {
      fdf_log::error("invalid subdir");
      return;
    }

    zx_handle_t directory = dir_infos_[0].dir.handle()->get();
    zx_status_t status = fdio_open3_at(
        directory, "/",
        request->has_flags()
            ? uint64_t{request->flags()}
            : uint64_t{fuchsia_io::Flags::kProtocolDirectory | fuchsia_io::kPermReadable},
        request->channel().release());
    if (status != ZX_OK) {
      fdf_log::error("Failed to open directory: {}", status);
    }

    return;
  }

  auto subdir = std::string(request->subdir().get());
  auto slash = subdir.find('/');
  if (slash == std::string::npos) {
    fdf_log::error("invalid subdir");
    return;
  }

  std::string instance = subdir.substr(0, slash);
  std::string proto = subdir.substr(slash + 1);
  zx_handle_t directory = ZX_HANDLE_INVALID;
  std::string source_instance;

  for (auto& dir_info : dir_infos_) {
    if (instance == "default") {
      if (dir_info.is_primary) {
        source_instance = dir_info.target_to_source_instance_mapping["default"];
        directory = dir_info.dir.handle()->get();
        break;
      }
    } else if (dir_info.parent_name == instance) {
      source_instance = dir_info.target_to_source_instance_mapping["default"];
      directory = dir_info.dir.handle()->get();
      break;
    } else {
      if (dir_info.target_to_source_instance_mapping.contains(instance)) {
        source_instance = dir_info.target_to_source_instance_mapping[instance];
        directory = dir_info.dir.handle()->get();
        break;
      }
    }
  }

  if (directory == ZX_HANDLE_INVALID) {
    fdf_log::error("unknown instance");
    return;
  }

  std::string new_subdir = std::format("{}/{}", source_instance, proto);

  // This seems to be an inconsistency in the CF APIs. The requested type is for a
  // fidl::ClientEnd<fuchsia_io::Directory>,
  // but it seems to expect to connect directly to the protocol at the subdir.
  // (aka fidl::ClientEnd<user::Protocol>).
  zx_status_t status =
      fdio_open3_at(directory, new_subdir.c_str(),
                    request->has_flags()
                        ? uint64_t{request->flags()}
                        : uint64_t{fuchsia_io::Flags::kProtocolService | fuchsia_io::kPermReadable},
                    request->channel().release());
  if (status != ZX_OK) {
    fdf_log::error("Failed to open directory: {}", status);
    return;
  }
}

void DirReceiverImpl::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_component_sandbox::DirReceiver> metadata,
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

  fdf_log::warn("fuchsia_component_sandbox::DirReceiver received unknown {} method. Ordinal: {}",
                method_type, metadata.method_ordinal);
}

}  // namespace driver_manager
