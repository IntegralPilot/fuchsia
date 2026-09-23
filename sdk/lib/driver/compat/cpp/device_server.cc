// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdio/directory.h>

namespace compat {

namespace {

void CollectMetadataFrom(fidl::VectorView<fuchsia_driver_compat::wire::Metadata> metadata,
                         const ForwardMetadata& forward_metadata, DeviceServer* server) {
  for (auto& metadata : metadata) {
    auto should_forward = forward_metadata.should_forward(metadata.type);
    if (should_forward) {
      size_t size;
      zx_status_t status =
          metadata.data.get_property(ZX_PROP_VMO_CONTENT_SIZE, &size, sizeof(size));
      if (status != ZX_OK) {
        fdf::error("Failed to get metadata vmo size: {}", zx_status_get_string(status));
        continue;
      }

      Metadata data(size);
      status = metadata.data.read(data.data(), 0, data.size());
      if (status != ZX_OK) {
        fdf::error("Failed to read metadata vmo: {}", zx_status_get_string(status));
        continue;
      }

      server->AddMetadata(metadata.type, data.data(), data.size());
    }
  }
}

}  // namespace

ForwardMetadata ForwardMetadata::All() { return ForwardMetadata(std::nullopt); }

ForwardMetadata ForwardMetadata::None() {
  return ForwardMetadata(std::make_optional(std::unordered_set<MetadataKey>{}));
}

ForwardMetadata ForwardMetadata::Some(std::unordered_set<MetadataKey> filter) {
  ZX_ASSERT_MSG(!filter.empty(), "ForwardMetadata::Some 'filter' cannot be empty.");
  return ForwardMetadata(std::make_optional(filter));
}

bool ForwardMetadata::empty() const { return filter_.has_value() && filter_->empty(); }

bool ForwardMetadata::should_forward(MetadataKey key) const {
  if (!filter_.has_value()) {
    return true;
  }

  return filter_->find(key) != filter_->end();
}

void DeviceServer::Initialize(std::string name, std::optional<ServiceOffersV1> service_offers,
                              std::optional<BanjoConfig> banjo_config) {
  name_ = std::move(name);
  service_offers_ = std::move(service_offers);
  banjo_config_ = std::move(banjo_config);
}

zx_status_t DeviceServer::AddMetadata(uint32_t type, const void* data, size_t size) {
  Metadata metadata(size);
  auto begin = static_cast<const uint8_t*>(data);
  std::copy(begin, begin + size, metadata.begin());
  auto [_, inserted] = metadata_.emplace(type, std::move(metadata));
  if (!inserted) {
    // TODO(https://fxbug.dev/42063857): Return ZX_ERR_ALREADY_EXISTS instead once we do so in DFv1.
    return ZX_OK;
  }
  return ZX_OK;
}

zx_status_t DeviceServer::GetMetadata(uint32_t type, void* buf, size_t buflen, size_t* actual) {
  auto it = metadata_.find(type);
  if (it == metadata_.end()) {
    return ZX_ERR_NOT_FOUND;
  }
  auto& [_, metadata] = *it;

  *actual = metadata.size();
  if (buflen < metadata.size()) {
    return ZX_ERR_BUFFER_TOO_SMALL;
  }
  auto size = std::min(buflen, metadata.size());
  auto begin = metadata.begin();
  std::copy(begin, begin + size, static_cast<uint8_t*>(buf));

  return ZX_OK;
}

zx_status_t DeviceServer::GetMetadataSize(uint32_t type, size_t* out_size) {
  auto it = metadata_.find(type);
  if (it == metadata_.end()) {
    return ZX_ERR_NOT_FOUND;
  }
  auto& [_, metadata] = *it;
  *out_size = metadata.size();
  return ZX_OK;
}

zx_status_t DeviceServer::GetProtocol(BanjoProtoId proto_id, GenericProtocol* out) const {
  if (!banjo_config_.has_value()) {
    return ZX_ERR_NOT_FOUND;
  }

  // If there is a specific entry for the proto_id, use it.
  auto specific_entry = banjo_config_->callbacks.find(proto_id);
  if (specific_entry != banjo_config_->callbacks.end()) {
    auto& get_banjo_protocol = specific_entry->second;
    if (out) {
      *out = get_banjo_protocol();
    }

    return ZX_OK;
  }

  // Otherwise use the generic one if one was provided.
  if (!banjo_config_->generic_callback) {
    return ZX_ERR_NOT_FOUND;
  }

  zx::result generic_result = banjo_config_->generic_callback(proto_id);
  if (generic_result.is_error()) {
    return ZX_ERR_NOT_FOUND;
  }

  if (out) {
    *out = generic_result.value();
  }

  return ZX_OK;
}

zx_status_t DeviceServer::Serve(async_dispatcher_t* dispatcher, fdf::OutgoingDirectory* outgoing) {
  auto device = [this, dispatcher](
                    fidl::ServerEnd<fuchsia_driver_compat::Device> server_end) mutable -> void {
    bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
  };
  fuchsia_driver_compat::Service::InstanceHandler handler({.device = device});
  zx::result<> status =
      outgoing->AddService<fuchsia_driver_compat::Service>(std::move(handler), name());
  if (status.is_error()) {
    return status.error_value();
  }
  stop_serving_ = [this, outgoing]() {
    (void)outgoing->RemoveService<fuchsia_driver_compat::Service>(name_);
  };

  if (service_offers_) {
    return service_offers_->Serve(dispatcher, outgoing);
  }
  return ZX_OK;
}

std::vector<fuchsia_driver_framework::wire::Offer> DeviceServer::CreateOffers2(
    fidl::ArenaBase& arena) {
  std::vector<fuchsia_driver_framework::wire::Offer> offers;
  // Create the main fuchsia.driver.compat.Service offer.
  offers.push_back(fdf::MakeOffer2<fuchsia_driver_compat::Service>(arena, name()));

  if (service_offers_) {
    auto service_offers = service_offers_->CreateOffers2(arena);
    offers.reserve(offers.size() + service_offers.size());
    offers.insert(offers.end(), service_offers.begin(), service_offers.end());
  }
  return offers;
}

std::vector<fuchsia_driver_framework::Offer> DeviceServer::CreateOffers2() {
  std::vector<fuchsia_driver_framework::Offer> offers;
  // Create the main fuchsia.driver.compat.Service offer.
  offers.push_back(fdf::MakeOffer2<fuchsia_driver_compat::Service>(name()));

  if (service_offers_) {
    auto service_offers = service_offers_->CreateOffers2();
    offers.reserve(offers.size() + service_offers.size());
    offers.insert(offers.end(), service_offers.begin(), service_offers.end());
  }
  return offers;
}

void DeviceServer::GetMetadata(GetMetadataCompleter::Sync& completer) {
  std::vector<fuchsia_driver_compat::wire::Metadata> metadata;
  metadata.reserve(metadata_.size());
  for (auto& [type, data] : metadata_) {
    fuchsia_driver_compat::wire::Metadata new_metadata;
    new_metadata.type = type;
    zx::vmo vmo;

    zx_status_t status = zx::vmo::create(data.size(), 0, &new_metadata.data);
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    status = new_metadata.data.write(data.data(), 0, data.size());
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    size_t size = data.size();
    status = new_metadata.data.set_property(ZX_PROP_VMO_CONTENT_SIZE, &size, sizeof(size));
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }

    metadata.push_back(std::move(new_metadata));
  }
  completer.ReplySuccess(fidl::VectorView<fuchsia_driver_compat::wire::Metadata>::FromExternal(
      metadata.data(), metadata.size()));
}

void DeviceServer::GetBanjoProtocol(GetBanjoProtocolRequestView request,
                                    GetBanjoProtocolCompleter::Sync& completer) {
  // First check that we are in the same driver host.
  static uint64_t process_koid = []() {
    zx_info_handle_basic_t basic;
    ZX_ASSERT(zx::process::self()->get_info(ZX_INFO_HANDLE_BASIC, &basic, sizeof(basic), nullptr,
                                            nullptr) == ZX_OK);
    return basic.koid;
  }();

  if (process_koid != request->process_koid) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  GenericProtocol result;
  zx_status_t status = GetProtocol(request->proto_id, &result);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }

  completer.ReplySuccess(reinterpret_cast<uint64_t>(result.ops),
                         reinterpret_cast<uint64_t>(result.ctx));
}

zx::result<> SyncInitializedDeviceServer::Initialize(
    const std::shared_ptr<fdf::Namespace>& incoming,
    const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
    const std::optional<std::string>& node_name, std::string_view child_node_name,
    const ForwardMetadata& forward_metadata, std::optional<DeviceServer::BanjoConfig> banjo_config,
    const std::optional<std::string>& child_additional_path) {
  auto child_node_name_str = std::string(child_node_name);

  // We can just serve and return if no metadata is needed.
  if (forward_metadata.empty()) {
    return CreateAndServe(outgoing, child_node_name_str, std::move(banjo_config));
  }

  // First connect to all the parents.
  auto parent_devices = ConnectToParentDevices(incoming.get());
  if (parent_devices.is_error()) {
    fdf::debug("Failed to get parent devices: {}.", parent_devices);

    // No parents if we are the root node.
    return CreateAndServe(outgoing, child_node_name_str, std::move(banjo_config));
  }

  // Now we have out parent clients, store them.
  fidl::WireSyncClient<fuchsia_driver_compat::Device> default_parent_client;
  std::unordered_map<std::string, fidl::WireSyncClient<fuchsia_driver_compat::Device>>
      parent_clients;
  for (auto& parent : parent_devices.value()) {
    if (parent.name == "default") {
      default_parent_client.Bind(std::move(parent.client));
      continue;
    }

    // TODO(https://fxbug.dev/42051759): When services stop adding extra instances
    // separated by ',' then remove this check.
    if (parent.name.find(',') != std::string::npos) {
      continue;
    }

    parent_clients[parent.name] =
        fidl::WireSyncClient<fuchsia_driver_compat::Device>(std::move(parent.client));
  }

  // No default parent found.
  if (!default_parent_client.is_valid()) {
    fdf::debug("Failed to find the default parent.");

    // No parents if we are the root node.
    return CreateAndServe(outgoing, child_node_name_str, std::move(banjo_config));
  }

  // We will create and serve the inner DeviceServer so we can add the metadata to it.
  auto result = CreateAndServe(outgoing, child_node_name_str, std::move(banjo_config));
  if (result.is_error()) {
    return result;
  }

  // Forward metadata
  if (parent_clients.empty()) {
    fidl::WireResult metadata_result = default_parent_client->GetMetadata();
    if (!metadata_result.ok()) {
      fdf::warn("Failed to get metadata from default parent. {}", metadata_result.status_string());
    } else if (metadata_result.value().is_error()) {
      fdf::warn("Failed to get metadata from default parent. {}",
                zx_status_get_string(metadata_result.value().error_value()));
      return zx::ok();
    } else {
      CollectMetadataFrom(metadata_result->value()->metadata, forward_metadata,
                          &device_server_.value());
    }
  } else {
    for (auto& [parent_name, parent_client] : parent_clients) {
      fidl::WireResult metadata_result = parent_client->GetMetadata();
      if (!metadata_result.ok()) {
        fdf::warn("Failed to get metadata from parent {}. {}", parent_name,
                  metadata_result.status_string());
      } else if (metadata_result.value().is_error()) {
        fdf::warn("Failed to get metadata from parent {}. {}", parent_name,
                  zx_status_get_string(metadata_result.value().error_value()));
      } else {
        CollectMetadataFrom(metadata_result->value()->metadata, forward_metadata,
                            &device_server_.value());
      }
    }
  }

  return zx::ok();
}

zx::result<> SyncInitializedDeviceServer::CreateAndServe(
    const std::shared_ptr<fdf::OutgoingDirectory>& outgoing, std::string child_node_name,
    std::optional<DeviceServer::BanjoConfig> banjo_config) {
  ZX_ASSERT_MSG(device_server_ == std::nullopt,
                "Cannot call Initialize on the SyncInitializedDeviceServer more than once.");
  device_server_.emplace();
  device_server_->Initialize(std::move(child_node_name), std::nullopt, std::move(banjo_config));

  zx_status_t serve_result =
      device_server_->Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), outgoing.get());
  if (serve_result != ZX_OK) {
    fdf::error("Failed to serve: {}", zx_status_get_string(serve_result));
    device_server_.reset();
    return zx::error(serve_result);
  }

  return zx::ok();
}

void AsyncInitializedDeviceServer::Begin(const std::shared_ptr<fdf::Namespace>& incoming,
                                         const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                                         const std::optional<std::string>& node_name,
                                         std::string_view child_node_name,
                                         fit::callback<void(zx::result<>)> callback,
                                         const ForwardMetadata& forward_metadata,
                                         std::optional<DeviceServer::BanjoConfig> banjo_config,
                                         const std::optional<std::string>& child_additional_path) {
  ZX_ASSERT_MSG(storage_ == std::nullopt,
                "Cannot call Begin on AsyncInitializedDeviceServer more than once.");
  storage_.emplace(AsyncInitStorage{
      .incoming = incoming,
      .outgoing = outgoing,
      .child_node_name = std::string(child_node_name),
      .callback = std::move(callback),
      .forward_metadata = forward_metadata,
      .banjo_config = std::move(banjo_config),
  });
  BeginAsyncInit();
}

void AsyncInitializedDeviceServer::BeginAsyncInit() {
  ZX_ASSERT(storage_);
  if (storage_->forward_metadata.empty()) {
    OnParentDevices(zx::ok(std::vector<ParentDevice>{}));
    return;
  }

  auto task = ConnectToParentDevices(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                     storage_->incoming.get(),
                                     [this](zx::result<std::vector<ParentDevice>> parents) {
                                       OnParentDevices(std::move(parents));
                                     });
  async_tasks_.AddTask(std::move(task));
}

void AsyncInitializedDeviceServer::OnParentDevices(
    zx::result<std::vector<ParentDevice>> parent_devices) {
  if (!storage_) {
    return;
  }

  if (parent_devices.is_error()) {
    fdf::debug("Failed to get parent devices: {}.", parent_devices);

    // No parents if we are the root node.
    zx::result result = CreateAndServe();
    if (result.is_error()) {
      CompleteInitialization(result.take_error());
      return;
    }

    CompleteInitialization(zx::ok());
    return;
  }

  for (auto& parent : parent_devices.value()) {
    if (parent.name == "default") {
      default_parent_client_.Bind(std::move(parent.client),
                                  fdf::Dispatcher::GetCurrent()->async_dispatcher());
      continue;
    }

    // TODO(https://fxbug.dev/42051759): When services stop adding extra instances
    // separated by ',' then remove this check.
    if (parent.name.find(',') != std::string::npos) {
      continue;
    }

    parent_clients_[parent.name] = fidl::WireClient<fuchsia_driver_compat::Device>(
        std::move(parent.client), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

  if (!default_parent_client_.is_valid()) {
    fdf::debug("Failed to find the default parent.");

    // No parents if we are the root node.
    zx::result result = CreateAndServe();
    if (result.is_error()) {
      CompleteInitialization(result.take_error());
      return;
    }

    CompleteInitialization(zx::ok());
    return;
  }

  // We will create and serve the inner DeviceServer so we can add the metadata to it.
  zx::result create_result = CreateAndServe();
  if (create_result.is_error()) {
    CompleteInitialization(create_result.take_error());
    return;
  }

  if (parent_clients_.empty()) {
    storage_->in_flight_metadata++;
    default_parent_client_->GetMetadata().Then(
        [this](fidl::WireUnownedResult<fuchsia_driver_compat::Device::GetMetadata>& result) {
          OnMetadataResult(result);
        });
  } else {
    for (auto& [parent_name, parent_client] : parent_clients_) {
      storage_->in_flight_metadata++;
      parent_client->GetMetadata().Then(
          [this](fidl::WireUnownedResult<fuchsia_driver_compat::Device::GetMetadata>& result) {
            OnMetadataResult(result);
          });
    }
  }
}

void AsyncInitializedDeviceServer::OnMetadataResult(
    fidl::WireUnownedResult<fuchsia_driver_compat::Device::GetMetadata>& result) {
  if (!storage_) {
    return;
  }

  if (!result.ok()) {
    fdf::warn("Failed to get metadata: {}", result.status_string());
  } else if (result.value().is_error()) {
    fdf::warn("Failed to get metadata: {}", zx_status_get_string(result.value().error_value()));

  } else {
    CollectMetadataFrom(result.value().value()->metadata, storage_->forward_metadata,
                        &device_server_.value());
  }

  storage_->in_flight_metadata--;
  if (storage_->in_flight_metadata == 0) {
    CompleteInitialization(zx::ok());
    return;
  }
}

zx::result<> AsyncInitializedDeviceServer::CreateAndServe() {
  if (!storage_) {
    return zx::error(ZX_ERR_CANCELED);
  }

  ZX_ASSERT(device_server_ == std::nullopt);
  device_server_.emplace();
  device_server_->Initialize(std::move(storage_->child_node_name), std::nullopt,
                             std::move(storage_->banjo_config));

  zx_status_t serve_result = device_server_->Serve(
      fdf::Dispatcher::GetCurrent()->async_dispatcher(), storage_->outgoing.get());
  if (serve_result != ZX_OK) {
    fdf::error("Failed to serve: {}", zx_status_get_string(serve_result));
    device_server_.reset();
    return zx::error(serve_result);
  }

  return zx::ok();
}

void AsyncInitializedDeviceServer::CompleteInitialization(zx::result<> result) {
  if (result.is_error()) {
    device_server_.reset();
  }

  if (!storage_) {
    return;
  }

  auto callback = std::move(storage_->callback);
  storage_.reset();
  callback(result);
}

}  // namespace compat
