// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "host.h"

#include <fidl/fuchsia.bluetooth.le/cpp/fidl.h>
#include <fidl/fuchsia.bluetooth.sys/cpp/markers.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <algorithm>

#include <google/protobuf/io/coded_stream.h>
#include <google/protobuf/message.h>

using grpc::Status;
using grpc::StatusCode;

using namespace std::chrono_literals;

namespace {

// `le_addr_bytes` must be in little-endian order.
zx::result<uint64_t> GetPeerId(
    fidl::SyncClient<fuchsia_bluetooth_affordances::PeerController>& client,
    std::string_view le_addr_bytes, fuchsia_bluetooth::AddressType type) {
  if (le_addr_bytes.size() != 6) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::array<uint8_t, 6> le_bytes;
  std::ranges::copy(le_addr_bytes, le_bytes.begin());
  fuchsia_bluetooth::Address addr(type, le_bytes);

  fuchsia_bluetooth_affordances::PeerControllerGetPeerIdRequest get_peer_id_request;
  get_peer_id_request.address() = addr;
  auto result = client->GetPeerId(get_peer_id_request);
  if (result.is_error()) {
    FX_LOGS(WARNING) << "fuchsia.bluetooth.affordances.PeerController/GetPeerId error: "
                     << result.error_value().FormatDescription();
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  if (!result->id().has_value()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  return zx::ok(result->id()->value());
}

}  // namespace

// TODO(https://fxbug.dev/316721276): Implement gRPCs necessary to enable GAP/A2DP testing.

HostService::HostService(async_dispatcher_t* dispatcher) {
  dispatcher_ = dispatcher;

  // Connect to fuchsia.bluetooth.affordances.PeripheralController
  zx::result peripheral_controller_client_end =
      component::Connect<fuchsia_bluetooth_affordances::PeripheralController>();
  if (peripheral_controller_client_end.is_ok()) {
    peripheral_controller_client_.Bind(std::move(*peripheral_controller_client_end));
  } else {
    FX_LOGS(ERROR) << "Error connecting to PeripheralController service: "
                   << peripheral_controller_client_end.status_string();
  }

  // Connect to fuchsia.bluetooth.affordances.HostController
  zx::result host_controller_client_end =
      component::Connect<fuchsia_bluetooth_affordances::HostController>();
  if (host_controller_client_end.is_ok()) {
    host_controller_client_.Bind(std::move(*host_controller_client_end));
  } else {
    FX_LOGS(ERROR) << "Error connecting to HostController service: "
                   << host_controller_client_end.status_string();
  }

  // Connect to fuchsia.bluetooth.affordances.PeerController
  zx::result peer_controller_client_end =
      component::Connect<fuchsia_bluetooth_affordances::PeerController>();
  if (peer_controller_client_end.is_ok()) {
    peer_controller_client_.Bind(std::move(*peer_controller_client_end));
  } else {
    FX_LOGS(ERROR) << "Error connecting to PeerController service: "
                   << peer_controller_client_end.status_string();
  }

  // Connect to fuchsia.bluetooth.affordances.CentralController
  zx::result central_controller_client_end =
      component::Connect<fuchsia_bluetooth_affordances::CentralController>();
  if (central_controller_client_end.is_ok()) {
    central_controller_client_.Bind(std::move(*central_controller_client_end));
  } else {
    FX_LOGS(ERROR) << "Error connecting to CentralController service: "
                   << central_controller_client_end.status_string();
  }

  // Connect to fuchsia.bluetooth.sys.Access
  zx::result access_sync_client_end = component::Connect<fuchsia_bluetooth_sys::Access>();
  if (access_sync_client_end.is_ok()) {
    access_sync_client_.Bind(std::move(*access_sync_client_end));
  } else {
    FX_LOGS(ERROR) << "Error connecting to Access service: "
                   << access_sync_client_end.status_string();
  }
}

Status HostService::FactoryReset(grpc::ServerContext* context,
                                 const google::protobuf::Empty* request,
                                 google::protobuf::Empty* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status HostService::Reset(grpc::ServerContext* context, const google::protobuf::Empty* request,
                          google::protobuf::Empty* response) {
  // No-op for now; return OK status.
  return {/*OK*/};
}

Status HostService::ReadLocalAddress(grpc::ServerContext* context,
                                     const google::protobuf::Empty* request,
                                     pandora::ReadLocalAddressResponse* response) {
  auto result = host_controller_client_->GetHosts();
  if (result.is_error()) {
    return Status(StatusCode::INTERNAL,
                  "fuchsia.bluetooth.affordances.HostController/GetHosts error: " +
                      result.error_value().FormatDescription());
  }

  if (!result->hosts().has_value() || result->hosts().value().empty()) {
    return Status(StatusCode::NOT_FOUND, "No hosts available");
  }

  const auto& hosts = result->hosts().value();
  for (const auto& host : hosts) {
    if (host.active().has_value() && host.active().value()) {
      if (host.addresses().has_value() && !host.addresses().value().empty()) {
        std::array<uint8_t, 6> host_addr;
        std::ranges::copy(host.addresses().value()[0].bytes(), host_addr.begin());

        // Convert local address from little-endian to big-endian, as expected by Pandora.
        std::ranges::reverse(host_addr);
        response->set_address(host_addr.data(), 6);
        return Status::OK;
      }
    }
  }

  return Status(StatusCode::NOT_FOUND, "No active host with valid address found");
}

Status HostService::Connect(grpc::ServerContext* context, const pandora::ConnectRequest* request,
                            pandora::ConnectResponse* response) {
  std::string little_endian_addr(request->address().rbegin(), request->address().rend());
  auto get_peer_id_result = GetPeerId(peer_controller_client_, little_endian_addr,
                                      fuchsia_bluetooth::AddressType::kPublic);
  if (get_peer_id_result.is_error()) {
    return Status(StatusCode::NOT_FOUND, "Peer not found");
  }
  uint64_t peer_id = *get_peer_id_result;

  auto connect_result = access_sync_client_->Connect({fuchsia_bluetooth::PeerId{peer_id}});
  if (connect_result.is_error()) {
    return Status(StatusCode::INTERNAL, "fuchsia.bluetooth.sys.Access/Connect error: " +
                                            connect_result.error_value().FormatDescription());
  }

  response->mutable_connection()->mutable_cookie()->set_value(std::to_string(peer_id));
  return {/*OK*/};
}

Status HostService::WaitConnection(grpc::ServerContext* context,
                                   const pandora::WaitConnectionRequest* request,
                                   pandora::WaitConnectionResponse* response) {
  auto peer_it = WaitForPeer(request->address(), true);
  if (peer_it->id()) {
    response->mutable_connection()->mutable_cookie()->set_value(
        std::to_string(peer_it->id()->value()));
  }
  return {/*OK*/};
}

Status HostService::ConnectLE(::grpc::ServerContext* context,
                              const ::pandora::ConnectLERequest* request,
                              ::pandora::ConnectLEResponse* response) {
  if (!request->has_public_()) {
    return Status(StatusCode::INVALID_ARGUMENT, "Expected public address field");
  }
  if (request->public_().size() != 6) {
    return Status(StatusCode::INVALID_ARGUMENT, "Expected 6 byte BD_ADDR");
  }

  std::string little_endian_addr(request->public_().rbegin(), request->public_().rend());
  auto get_peer_id_result = GetPeerId(peer_controller_client_, little_endian_addr,
                                      fuchsia_bluetooth::AddressType::kPublic);
  if (get_peer_id_result.is_error()) {
    return Status(StatusCode::NOT_FOUND, "Peer not found");
  }
  uint64_t peer_id = *get_peer_id_result;

  fuchsia_bluetooth_affordances::PeerSelector selector;
  selector.id() = fuchsia_bluetooth::PeerId{peer_id};

  auto connect_peripheral_result = central_controller_client_->ConnectPeripheral(selector);
  if (connect_peripheral_result.is_error()) {
    return Status(StatusCode::INTERNAL,
                  "fuchsia.bluetooth.affordances.CentralController/ConnectPeripheral error: " +
                      connect_peripheral_result.error_value().FormatDescription());
  }
  response->mutable_connection()->mutable_cookie()->set_value(std::to_string(peer_id));
  return {/*OK*/};
}

Status HostService::Disconnect(::grpc::ServerContext* context,
                               const ::pandora::DisconnectRequest* request,
                               ::google::protobuf::Empty* response) {
  uint64_t peer_id =
      std::strtoul(request->connection().cookie().value().c_str(), nullptr, /*base=*/10);

  auto result = access_sync_client_->Disconnect({fuchsia_bluetooth::PeerId{peer_id}});
  if (result.is_error()) {
    return Status(StatusCode::INTERNAL, "fuchsia.bluetooth.sys.Access/Disconnect error: " +
                                            result.error_value().FormatDescription());
  }
  return {/*OK*/};
}

Status HostService::WaitDisconnection(::grpc::ServerContext* context,
                                      const ::pandora::WaitDisconnectionRequest* request,
                                      ::google::protobuf::Empty* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

Status HostService::Advertise(::grpc::ServerContext* context,
                              const ::pandora::AdvertiseRequest* request,
                              ::grpc::ServerWriter<::pandora::AdvertiseResponse>* writer) {
  fuchsia_bluetooth_le::AdvertisingParameters parameters;
  fuchsia_bluetooth_le::AdvertisingData data;

  if (request->has_data()) {
    const pandora::DataTypes& req_data = request->data();

    if (req_data.le_discoverability_mode() == pandora::DiscoverabilityMode::DISCOVERABLE_GENERAL) {
      fuchsia_bluetooth_affordances::HostControllerSetDiscoverabilityRequest
          set_discoverability_request;
      set_discoverability_request.discoverable() = true;
      auto result = host_controller_client_->SetDiscoverability(set_discoverability_request);
      if (result.is_error()) {
        return Status(StatusCode::INTERNAL,
                      "fuchsia.bluetooth.affordances.HostController/SetDiscoverability error: " +
                          result.error_value().FormatDescription());
      }
    }
    if (req_data.include_complete_local_name() || req_data.include_shortened_local_name()) {
      data.name() = "sapphire";
    }
    if (!req_data.complete_service_class_uuids128().empty()) {
      std::vector<fuchsia_bluetooth::Uuid> uuids;
      for (const auto& uuid_str : req_data.complete_service_class_uuids128()) {
        UuidBytes bytes = uuid_from_string(uuid_str.c_str());
        std::array<uint8_t, 16> uuid_arr;
        std::ranges::copy(bytes.value, uuid_arr.begin());

        if (std::ranges::none_of(uuid_arr, [](uint8_t byte) { return byte != 0; })) {
          return Status(StatusCode::INVALID_ARGUMENT, "Failed to parse UUID");
        }
        uuids.emplace_back(uuid_arr);
      }

      data.service_uuids() = std::move(uuids);
    }
  }

  parameters.data() = std::move(data);
  parameters.connectable() = request->connectable();

  if (request->own_address_type() == pandora::OwnAddressType::RANDOM ||
      request->own_address_type() == pandora::OwnAddressType::RESOLVABLE_OR_RANDOM) {
    parameters.address_type() = fuchsia_bluetooth::AddressType::kRandom;
  } else {
    parameters.address_type() = fuchsia_bluetooth::AddressType::kPublic;
  }

  pandora::AdvertiseResponse response;
  auto result = peripheral_controller_client_->Advertise(
      {{.parameters = std::move(parameters), .timeout = 10 /*seconds*/}});
  if (result.is_error()) {
    auto err = result.error_value();
    if (err.is_domain_error() &&
        err.domain_error() == fuchsia_bluetooth_affordances::Error::kTimeout) {
      FX_LOGS(WARNING) << "Advertisement timed out without connection";
    } else {
      return Status(
          StatusCode::INTERNAL,
          std::format("fuchsia.bluetooth.affordances.PeripheralController/Advertise error: {}",
                      err.FormatDescription()));
    }
  } else {
    response.mutable_connection()->mutable_cookie()->set_value(
        std::to_string(result->peer_id().value().value()));
  }
  writer->Write(response);

  return Status::OK;
}

namespace {

class ScanResultListenerImpl
    : public fidl::Server<fuchsia_bluetooth_affordances::ScanResultListener> {
 public:
  explicit ScanResultListenerImpl(::grpc::ServerWriter<::pandora::ScanningResponse>* writer)
      : writer_(writer) {}

  void OnPeersDiscovered(OnPeersDiscoveredRequest& request,
                         OnPeersDiscoveredCompleter::Sync& completer) override {
    std::scoped_lock lock(mutex_);
    if (!writer_) {
      completer.Reply();
      return;
    }

    for (const fuchsia_bluetooth_affordances::ScannedPeer& peer : request.peers().value()) {
      pandora::ScanningResponse scan_rsp;

      // Address (always present)
      uint8_t addr[6];
      std::ranges::copy(peer.address().value().bytes(), addr);
      std::ranges::reverse(addr);  // Big endian -> little endian

      if (peer.address().value().type() == fuchsia_bluetooth::AddressType::kPublic) {
        scan_rsp.set_public_(addr, 6);
      } else if (peer.address().value().type() == fuchsia_bluetooth::AddressType::kRandom) {
        scan_rsp.set_random(addr, 6);
      }

      // Connectable flag (always present)
      scan_rsp.set_connectable(peer.peer().value().connectable().value());

      // Name (if present)
      if (peer.peer().value().name().has_value()) {
        scan_rsp.mutable_data()->set_complete_local_name(peer.peer().value().name().value());
      }

      if (!writer_->Write(scan_rsp)) {
        FX_LOGS(INFO) << "LE scan canceled by gRPC client.";
        writer_ = nullptr;
        break;
      }
    }
    completer.Reply();
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_bluetooth_affordances::ScanResultListener> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGS(WARNING) << "Unknown method received: " << metadata.method_ordinal;
  }

 private:
  ::grpc::ServerWriter<::pandora::ScanningResponse>* writer_;
  std::mutex mutex_;
};

}  // namespace

Status HostService::Scan(::grpc::ServerContext* context, const ::pandora::ScanRequest* request,
                         ::grpc::ServerWriter<::pandora::ScanningResponse>* writer) {
  auto endpoints = fidl::CreateEndpoints<fuchsia_bluetooth_affordances::ScanResultListener>();
  ZX_ASSERT(endpoints.is_ok());
  auto [client_end, server_end] = *std::move(endpoints);

  ScanResultListenerImpl listener_impl(writer);
  auto binding = fidl::BindServer(dispatcher_, std::move(server_end), &listener_impl);

  fuchsia_bluetooth_affordances::CentralControllerStartScanRequest start_request;
  start_request.listener() = std::move(client_end);

  auto start_result = central_controller_client_->StartScan(std::move(start_request));
  if (start_result.is_error()) {
    FX_LOGS(ERROR) << "StartScan encountered error: "
                   << start_result.error_value().FormatDescription();
    return Status(StatusCode::INTERNAL, "Failed to start scan");
  }

  // Scan for an arbitrary period of 5 seconds, which passes PTS tests.
  std::this_thread::sleep_for(std::chrono::seconds(5));

  binding.Unbind();  // Stop listening

  return Status::OK;
}

Status HostService::Inquiry(::grpc::ServerContext* context,
                            const ::google::protobuf::Empty* request,
                            ::grpc::ServerWriter<::pandora::InquiryResponse>* writer) {
  fuchsia_bluetooth_affordances::PeerControllerSetDiscoveryRequest set_discovery_request;
  set_discovery_request.discovery() = true;
  auto set_discovery_result = peer_controller_client_->SetDiscovery(set_discovery_request);
  if (set_discovery_result.is_error()) {
    return Status(StatusCode::INTERNAL,
                  "fuchsia.bluetooth.affordances.PeerController/SetDiscovery error: " +
                      set_discovery_result.error_value().FormatDescription());
  }

  // Discover for an arbitrary period of 5 seconds, which passes PTS tests.
  //
  // TODO(https://fxbug.dev/396500079): Adopt a streaming API instead of a scan period with timeout.
  std::this_thread::sleep_for(std::chrono::seconds(5));

  auto get_known_peers_result = peer_controller_client_->GetKnownPeers();
  if (get_known_peers_result.is_error()) {
    return Status(StatusCode::INTERNAL,
                  "fuchsia.bluetooth.affordances.PeerController/GetKnownPeers error: " +
                      get_known_peers_result.error_value().FormatDescription());
  }

  if (get_known_peers_result->peers().has_value()) {
    for (const fuchsia_bluetooth_sys::Peer& peer : get_known_peers_result->peers().value()) {
      pandora::InquiryResponse inquiry_rsp;
      std::array<uint8_t, 6> peer_addr;
      std::ranges::copy(peer.address().value().bytes(), peer_addr.begin());
      // Convert peer address from little-endian to big-endian, as expected by Pandora.
      std::ranges::reverse(peer_addr);
      inquiry_rsp.set_address(peer_addr.data(), 6);

      if (!writer->Write(inquiry_rsp)) {
        FX_LOGS(INFO) << "Inquiry canceled by gRPC client.";
        break;
      }
    }
  }

  set_discovery_request.discovery() = false;
  set_discovery_result = peer_controller_client_->SetDiscovery(set_discovery_request);
  if (set_discovery_result.is_error()) {
    return Status(StatusCode::INTERNAL,
                  "fuchsia.bluetooth.affordances.PeerController/SetDiscovery error: " +
                      set_discovery_result.error_value().FormatDescription());
  }

  return Status::OK;
}

Status HostService::SetDiscoverabilityMode(::grpc::ServerContext* context,
                                           const ::pandora::SetDiscoverabilityModeRequest* request,
                                           ::google::protobuf::Empty* response) {
  fuchsia_bluetooth_affordances::HostControllerSetDiscoverabilityRequest
      set_discoverability_request;
  set_discoverability_request.discoverable() =
      request->mode() != ::pandora::DiscoverabilityMode::NOT_DISCOVERABLE;
  auto result = host_controller_client_->SetDiscoverability(set_discoverability_request);
  if (result.is_error()) {
    return Status(StatusCode::INTERNAL,
                  "fuchsia.bluetooth.affordances.HostController/SetDiscoverability error: " +
                      result.error_value().FormatDescription());
  }

  return Status::OK;
}

Status HostService::SetConnectabilityMode(::grpc::ServerContext* context,
                                          const ::pandora::SetConnectabilityModeRequest* request,
                                          ::google::protobuf::Empty* response) {
  bool connectable = request->mode() == ::pandora::ConnectabilityMode::CONNECTABLE;
  if (connectable) {
    if (suppress_connections_token_.is_valid()) {
      suppress_connections_token_ = {};
      return Status::OK;
    }
  } else {
    if (!suppress_connections_token_.is_valid()) {
      auto endpoints = fidl::CreateEndpoints<fuchsia_bluetooth_sys::ProcedureToken>();
      if (endpoints.is_error()) {
        FX_LOGS(ERROR) << "Failed to create ProcedureToken endpoints: "
                       << endpoints.status_string();
        return Status(StatusCode::INTERNAL, "Failed to create ProcedureToken endpoints");
      }

      fuchsia_bluetooth_sys::AccessSetConnectionPolicyRequest set_connection_policy_request;
      set_connection_policy_request.suppress_bredr_connections() = std::move(endpoints->server);

      auto result =
          access_sync_client_->SetConnectionPolicy(std::move(set_connection_policy_request));
      if (result.is_error()) {
        return Status(StatusCode::INTERNAL,
                      "fuchsia.bluetooth.sys.Access/SetConnectionPolicy error: " +
                          result.error_value().FormatDescription());
      }

      suppress_connections_token_ = std::move(endpoints->client);
    }
  }

  return Status::OK;
}

std::vector<fuchsia_bluetooth_sys::Peer>::const_iterator HostService::WaitForPeer(
    const std::string& addr, bool enforce_connected) {
  std::vector<fuchsia_bluetooth_sys::Peer>::const_iterator peer_it;
  std::unique_lock<std::mutex> lock(m_access_);

  do {
    if (!peer_watching_) {
      peer_watching_ = true;
      access_shared_client_->WatchPeers().Then(
          [this](fidl::Result<fuchsia_bluetooth_sys::Access::WatchPeers>& watch_peers) {
            if (watch_peers.is_error()) {
              fidl::Error err = watch_peers.error_value();
              FX_LOGS(ERROR) << "Host watcher error: " << err.error() << "\n";
              return;
            }

            std::unique_lock<std::mutex> lock(this->m_access_);
            peers_ = watch_peers->updated();
            peer_watching_ = false;
            cv_access_.notify_one();
          });
    }

    cv_access_.wait_for(lock, 1000ms, [this] { return !peer_watching_; });
  } while ((peer_it = std::find_if(
                peers_.begin(), peers_.end(),
                [&addr, enforce_connected](const fuchsia_bluetooth_sys::Peer& candidate) {
                  for (size_t i = 0; i < 6; ++i) {
                    if (candidate.address()->bytes()[5 - i] !=
                        static_cast<unsigned char>(addr[i])) {
                      return false;
                    }
                  }
                  return !enforce_connected || (candidate.connected() && *candidate.connected());
                })) == peers_.end());

  return peer_it;
}
