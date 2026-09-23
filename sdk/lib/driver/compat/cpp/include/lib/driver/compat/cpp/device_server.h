// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPAT_CPP_DEVICE_SERVER_H_
#define LIB_DRIVER_COMPAT_CPP_DEVICE_SERVER_H_

#include <fidl/fuchsia.driver.compat/cpp/wire_messaging.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/async-helpers/cpp/task_group.h>
#include <lib/driver/compat/cpp/connect.h>
#include <lib/driver/compat/cpp/service_offers.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/outgoing/cpp/handlers.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>

#include <unordered_set>
#include <utility>

namespace compat {

using Metadata = std::vector<uint8_t>;
using MetadataKey = uint32_t;
using MetadataMap = std::unordered_map<MetadataKey, const Metadata>;

using BanjoProtoId = uint32_t;

class ForwardMetadata final {
 public:
  // Creates a ForwardMetadata object in which all of the available metadata from the parent(s)
  // are forwarded.
  static ForwardMetadata All();

  // Creates a ForwardMetadata object in which none of the available metadata from the parent(s)
  // are forwarded.
  static ForwardMetadata None();

  // Creates a ForwardMetadata object in which some of the available metadata from the parent(s)
  // are forwarded. The given filter set must not be empty.
  //
  // The given set is used as a filter when looking through all of the available metadata to the
  // driver. This means if a given metadata key is not found through the parent(s), it will be
  // ignored.
  static ForwardMetadata Some(std::unordered_set<MetadataKey> filter);

  // Deprecated constructor. Use All(), Some(), None() instead.
  // TODO(https://fxbug.dev/42086090): Remove once all usages are migrated
  explicit ForwardMetadata(std::unordered_set<MetadataKey> filter) : filter_(filter) {}

  // Returns true when there's nothing to forward.
  bool empty() const;

  // Returns true if the given key meets the requirements for forwarding.
  bool should_forward(MetadataKey key) const;

 private:
  explicit ForwardMetadata(std::optional<std::unordered_set<MetadataKey>> filter)
      : filter_(std::move(filter)) {}

  std::optional<std::unordered_set<MetadataKey>> filter_;
};

// Forward declare for friend statement.
class AsyncInitializedDeviceServer;
class SyncInitializedDeviceServer;

// The DeviceServer class vends the fuchsia_driver_compat::Device interface.
// It represents a single device.
class DeviceServer : public fidl::WireServer<fuchsia_driver_compat::Device> {
  friend AsyncInitializedDeviceServer;
  friend SyncInitializedDeviceServer;

 public:
  struct GenericProtocol {
    const void* ops;
    void* ctx;
  };

  using SpecificGetBanjoProtoCb = fit::function<GenericProtocol()>;
  using GenericGetBanjoProtoCb = fit::function<zx::result<GenericProtocol>(BanjoProtoId)>;

  struct BanjoConfig {
    BanjoProtoId default_proto_id = 0;
    GenericGetBanjoProtoCb generic_callback = nullptr;
    std::unordered_map<BanjoProtoId, SpecificGetBanjoProtoCb> callbacks;
  };

  DeviceServer() = default;

  void Initialize(std::string name, std::optional<ServiceOffersV1> service_offers = std::nullopt,
                  std::optional<BanjoConfig> banjo_config = std::nullopt);

  // Functions to implement the DFv1 device API.
  zx_status_t AddMetadata(MetadataKey type, const void* data, size_t size);
  zx_status_t GetMetadata(MetadataKey type, void* buf, size_t buflen, size_t* actual);
  zx_status_t GetMetadataSize(MetadataKey type, size_t* out_size);
  zx_status_t GetProtocol(BanjoProtoId proto_id, GenericProtocol* out) const;

  // Serve this interface in an outgoing directory.
  zx_status_t Serve(async_dispatcher_t* dispatcher, fdf::OutgoingDirectory* outgoing);

  // Create offers to offer this interface to another component.
  std::vector<fuchsia_driver_framework::wire::Offer> CreateOffers2(fidl::ArenaBase& arena);
  std::vector<fuchsia_driver_framework::Offer> CreateOffers2();

  std::string_view name() const { return name_; }
  BanjoProtoId proto_id() const {
    return banjo_config_.has_value() ? banjo_config_->default_proto_id : 0;
  }
  bool has_banjo_config() const { return banjo_config_.has_value(); }

 private:
  // fuchsia.driver.compat.Compat
  void GetMetadata(GetMetadataCompleter::Sync& completer) override;
  void GetBanjoProtocol(GetBanjoProtocolRequestView request,
                        GetBanjoProtocolCompleter::Sync& completer) override;

  std::string name_;
  MetadataMap metadata_;
  std::optional<ServiceOffersV1> service_offers_;
  std::optional<BanjoConfig> banjo_config_;

  fidl::ServerBindingGroup<fuchsia_driver_compat::Device> bindings_;

  // This callback is called when the class is destructed and it will stop serving the protocol.
  fit::deferred_callback stop_serving_;
};

// Synchronously initialized Device Server. Prefer to use this as long as making sync/blocking
// calls on the current dispatcher is ok to do.
//
// See Initialize() for further details.
//
// # Thread safety
//
// This class is thread-unsafe.
class SyncInitializedDeviceServer {
 public:
  // Synchronously initialize the DeviceServer. Will immediately query the parent(s)
  // for forwarded metadata and serve on the outgoing directory synchronously before returning.
  //
  // |incoming|, |outgoing|, |node_name| can be accessed through the
  // DriverBase methods of the same name.
  //
  // |child_node_name| is the name given to the |fdf::NodeAddArgs|'s name field for the target node
  // of this server.
  //
  // |child_additional_path| is used in the case that there are intermediary nodes that are
  // owned by this driver before the target child node. Each intermediate node should be separated
  // with a '/' and it should end with a trailing '/'. Eg: "node-a/node-b/"
  //
  // |forward_metadata| contains information about the metadata to forward from the parent(s).
  //
  // |banjo_config| contains the banjo protocol information that this should serve.
  zx::result<> Initialize(const std::shared_ptr<fdf::Namespace>& incoming,
                          const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                          const std::optional<std::string>& node_name,
                          std::string_view child_node_name,
                          const ForwardMetadata& forward_metadata = ForwardMetadata::None(),
                          std::optional<DeviceServer::BanjoConfig> banjo_config = std::nullopt,
                          const std::optional<std::string>& child_additional_path = std::nullopt);

  // Provide CreateOffers as passthrough, and allow access to the inner device server.

  // Create offers to offer this interface to another component.
  std::vector<fuchsia_driver_framework::wire::Offer> CreateOffers2(fidl::ArenaBase& arena) {
    ZX_ASSERT(device_server_);
    return device_server_->CreateOffers2(arena);
  }
  std::vector<fuchsia_driver_framework::Offer> CreateOffers2() {
    ZX_ASSERT(device_server_);
    return device_server_->CreateOffers2();
  }

  const compat::DeviceServer& inner() const {
    ZX_ASSERT(device_server_);
    return device_server_.value();
  }

  compat::DeviceServer& inner() {
    ZX_ASSERT(device_server_);
    return device_server_.value();
  }

  void reset() { device_server_.reset(); }

 private:
  zx::result<> CreateAndServe(const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                              std::string child_node_name,
                              std::optional<DeviceServer::BanjoConfig> banjo_config);

  std::optional<compat::DeviceServer> device_server_;
};

// Asynchronously initialized Device Server. Use this when sync/blocking calls on the current
// dispatcher is not allowed or if your driver is already structured for async code and you want
// the performance gains of async.
//
// See Begin() for further details.
//
// # Thread safety
//
// This class is thread-unsafe.
class AsyncInitializedDeviceServer {
  struct AsyncInitStorage {
    std::shared_ptr<fdf::Namespace> incoming;
    std::shared_ptr<fdf::OutgoingDirectory> outgoing;
    std::string child_node_name;
    fit::callback<void(zx::result<>)> callback;
    ForwardMetadata forward_metadata;
    std::optional<DeviceServer::BanjoConfig> banjo_config;
    uint32_t in_flight_metadata = 0;
  };

 public:
  // Begin initialization. Will internally query the parent(s) for forwarded
  // metadata and serve on the outgoing directory when it is ready. The given callback is called
  // when the async initialization has been completed.
  //
  //
  // |incoming|, |outgoing|, |node_name| can be accessed through the
  // DriverBase methods of the same name.
  //
  // |child_node_name| is the name given to the |fdf::NodeAddArgs|'s name field for the target node
  // of this server.
  //
  // |callback| is called when the initialization is complete.
  //
  // |child_additional_path| is used in the case that there are intermediary nodes that are
  // owned by this driver before the target child node. Each intermediate node should be separated
  // with a '/' and it should end with a trailing '/'. Eg: "node-a/node-b/"
  //
  // |forward_metadata| contains information about the metadata to forward from the parent(s).
  //
  // |banjo_config| contains the banjo protocol information that this should serve.
  void Begin(const std::shared_ptr<fdf::Namespace>& incoming,
             const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
             const std::optional<std::string>& node_name, std::string_view child_node_name,
             fit::callback<void(zx::result<>)> callback,
             const ForwardMetadata& forward_metadata = ForwardMetadata::None(),
             std::optional<DeviceServer::BanjoConfig> banjo_config = std::nullopt,
             const std::optional<std::string>& child_additional_path = std::nullopt);

  // Provide CreateOffers as passthrough, and allow access to the inner device server.

  // Create offers to offer this interface to another component.
  std::vector<fuchsia_driver_framework::wire::Offer> CreateOffers2(fidl::ArenaBase& arena) {
    ZX_ASSERT(device_server_);
    return device_server_->CreateOffers2(arena);
  }
  std::vector<fuchsia_driver_framework::Offer> CreateOffers2() {
    ZX_ASSERT(device_server_);
    return device_server_->CreateOffers2();
  }

  const compat::DeviceServer& inner() const {
    ZX_ASSERT(device_server_);
    return device_server_.value();
  }

  compat::DeviceServer& inner() {
    ZX_ASSERT(device_server_);
    return device_server_.value();
  }

  void reset() {
    async_tasks_ = {};
    parent_clients_.clear();
    default_parent_client_ = {};
    device_server_.reset();
    storage_.reset();
  }

 private:
  void BeginAsyncInit();
  void OnParentDevices(zx::result<std::vector<ParentDevice>> parent_devices);
  void OnMetadataResult(
      fidl::WireUnownedResult<fuchsia_driver_compat::Device::GetMetadata>& result);
  zx::result<> CreateAndServe();
  void CompleteInitialization(zx::result<> result);

  std::optional<AsyncInitStorage> storage_;
  std::optional<compat::DeviceServer> device_server_;

  // Set in OnParentDevices().
  fidl::WireClient<fuchsia_driver_compat::Device> default_parent_client_;
  std::unordered_map<std::string, fidl::WireClient<fuchsia_driver_compat::Device>> parent_clients_;

  fdf::async_helpers::TaskGroup async_tasks_;
};

}  // namespace compat

#endif  // LIB_DRIVER_COMPAT_CPP_DEVICE_SERVER_H_
