// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "l2cap.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include <cstdlib>
#include <future>

using grpc::Status;
using grpc::StatusCode;

L2capService::L2capService(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
  // Connect to fuchsia.bluetooth.bredr.Profile
  zx::result profile_client_end = component::Connect<fuchsia_bluetooth_bredr::Profile>();
  if (profile_client_end.is_ok()) {
    profile_client_.Bind(std::move(*profile_client_end));
  } else {
    FX_LOGS(ERROR) << "Error connection to Profile service: " << profile_client_end.status_string();
  }
}

grpc::Status L2capService::Connect(::grpc::ServerContext* context,
                                   const ::pandora::l2cap::ConnectRequest* request,
                                   ::pandora::l2cap::ConnectResponse* response) {
  fuchsia_bluetooth::PeerId peer_id = fuchsia_bluetooth::PeerId{
      std::strtoul(request->connection().cookie().value().c_str(), nullptr, /*base=*/10)};
  uint64_t psm = static_cast<uint16_t>(request->basic().psm());
  fuchsia_bluetooth_bredr::L2capParameters l2cap_params;
  l2cap_params.psm(psm);
  fuchsia_bluetooth_bredr::ConnectParameters connect_params =
      fuchsia_bluetooth_bredr::ConnectParameters::WithL2cap(std::move(l2cap_params));

  auto result = profile_client_->Connect({peer_id, std::move(connect_params)});
  if (result.is_error()) {
    return Status(StatusCode::INTERNAL, "fuchsia.bluetooth.bredr.Profile/Connect error: " +
                                            result.error_value().FormatDescription());
  }

  auto& channel = result->channel();
  if (!channel.socket().has_value() && !channel.connection().has_value()) {
    return Status(StatusCode::INTERNAL, "Connected channel has no socket or connection");
  }

  {
    std::scoped_lock lock(m_l2cap_channel_);
    l2cap_socket_.reset();
    l2cap_connection_.reset();

    if (channel.connection().has_value()) {
      l2cap_connection_ = std::move(channel.connection().value());
    } else {
      l2cap_socket_ = std::move(channel.socket().value());
    }
  }

  return Status::OK;
}

namespace {

constexpr uint16_t kTspxPsm = 29;

// Non-reserved UUID (0x1401 in little-endian)
const fuchsia_bluetooth::Uuid kNonReservedUuid{
    std::array<uint8_t, 16>{0xfb, 0x34, 0x9b, 0x5f, 0x80, 0x00, 0x00, 0x80, 0x00, 0x10, 0x00, 0x00,
                            0x01, 0x14, 0x00, 0x00}};

using ConnectionResult = std::pair<fuchsia_bluetooth::PeerId, fuchsia_bluetooth_bredr::Channel>;

class ConnectionReceiverImpl : public fidl::Server<fuchsia_bluetooth_bredr::ConnectionReceiver> {
 public:
  explicit ConnectionReceiverImpl(std::promise<ConnectionResult> promise)
      : promise_(std::move(promise)) {}

  void Connected(ConnectedRequest& request, ConnectedCompleter::Sync& completer) override {
    if (!called_) {
      called_ = true;
      promise_.set_value(std::make_pair(request.peer_id(), std::move(request.channel())));
    }
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_bluetooth_bredr::ConnectionReceiver> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGS(WARNING) << "Unknown method received: " << metadata.method_ordinal;
  }

 private:
  std::promise<ConnectionResult> promise_;
  bool called_ = false;
};

}  // namespace

::grpc::Status L2capService::WaitConnection(::grpc::ServerContext* context,
                                            const ::pandora::l2cap::WaitConnectionRequest* request,
                                            ::pandora::l2cap::WaitConnectionResponse* response) {
  auto endpoints = fidl::CreateEndpoints<fuchsia_bluetooth_bredr::ConnectionReceiver>();
  if (endpoints.is_error()) {
    return Status(StatusCode::INTERNAL, "Failed to create ConnectionReceiver endpoints");
  }
  auto [client_end, server_end] = std::move(*endpoints);

  std::promise<ConnectionResult> promise;
  std::future<ConnectionResult> future = promise.get_future();

  auto receiver_impl = std::make_unique<ConnectionReceiverImpl>(std::move(promise));
  auto binding = fidl::BindServer(dispatcher_, std::move(server_end), std::move(receiver_impl));

  fuchsia_bluetooth_bredr::ProtocolDescriptor protocol_desc;
  protocol_desc.protocol() = fuchsia_bluetooth_bredr::ProtocolIdentifier::kL2Cap;
  std::vector<fuchsia_bluetooth_bredr::DataElement> params;
  params.push_back(fuchsia_bluetooth_bredr::DataElement::WithUint16(kTspxPsm));
  protocol_desc.params() = std::move(params);
  std::vector<fuchsia_bluetooth_bredr::ProtocolDescriptor> protocol_desc_list;
  protocol_desc_list.push_back(std::move(protocol_desc));

  fuchsia_bluetooth_bredr::ServiceDefinition service_def;
  service_def.service_class_uuids() = {{kNonReservedUuid}};
  service_def.protocol_descriptor_list() = std::move(protocol_desc_list);
  std::vector<fuchsia_bluetooth_bredr::ServiceDefinition> services;
  services.push_back(std::move(service_def));

  fuchsia_bluetooth::ChannelParameters channel_params;
  channel_params.channel_mode() = fuchsia_bluetooth::ChannelMode::kEnhancedRetransmission;

  fuchsia_bluetooth_bredr::ProfileAdvertiseRequest advertise_request;
  advertise_request.services() = std::move(services);
  advertise_request.receiver() = std::move(client_end);
  advertise_request.parameters() = std::move(channel_params);
  auto result = profile_client_->Advertise(std::move(advertise_request));
  if (result.is_error()) {
    binding.Unbind();
    return Status(StatusCode::INTERNAL, "fuchsia.bluetooth.bredr.Profile/Advertise error: " +
                                            result.error_value().FormatDescription());
  }

  if (future.wait_for(std::chrono::seconds(5)) != std::future_status::ready) {
    binding.Unbind();
    return Status(StatusCode::DEADLINE_EXCEEDED, "Advertisement timed out without connection");
  }

  auto [peer_id, channel] = future.get();
  if (!channel.socket().has_value() && !channel.connection().has_value()) {
    return Status(StatusCode::INTERNAL, "Connected channel has no socket or connection");
  }

  {
    std::scoped_lock lock(m_l2cap_channel_);
    l2cap_socket_.reset();
    l2cap_connection_.reset();

    if (channel.connection().has_value()) {
      l2cap_connection_ = std::move(channel.connection().value());
    } else {
      l2cap_socket_ = std::move(channel.socket().value());
    }
  }
  response->mutable_channel()->mutable_cookie()->set_value(std::to_string(peer_id.value()));
  binding.Unbind();
  return Status::OK;
}

::grpc::Status L2capService::Disconnect(::grpc::ServerContext* context,
                                        const ::pandora::l2cap::DisconnectRequest* request,
                                        ::pandora::l2cap::DisconnectResponse* response) {
  std::scoped_lock lock(m_l2cap_channel_);
  if (!l2cap_socket_.is_valid() && !l2cap_connection_.is_valid()) {
    return Status(StatusCode::FAILED_PRECONDITION, "L2CAP channel not connected");
  }
  l2cap_socket_.reset();
  l2cap_connection_.reset();
  response->mutable_success();
  return Status::OK;
}

::grpc::Status L2capService::WaitDisconnection(
    ::grpc::ServerContext* context, const ::pandora::l2cap::WaitDisconnectionRequest* request,
    ::pandora::l2cap::WaitDisconnectionResponse* response) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

::grpc::Status L2capService::Receive(
    ::grpc::ServerContext* context, const ::pandora::l2cap::ReceiveRequest* request,
    ::grpc::ServerWriter<::pandora::l2cap::ReceiveResponse>* writer) {
  return Status(StatusCode::UNIMPLEMENTED, "");
}

::grpc::Status L2capService::Send(::grpc::ServerContext* context,
                                  const ::pandora::l2cap::SendRequest* request,
                                  ::pandora::l2cap::SendResponse* response) {
  std::scoped_lock lock(m_l2cap_channel_);
  if (l2cap_connection_.is_valid()) {
    std::vector<fuchsia_bluetooth::Packet> packets;
    fuchsia_bluetooth::Packet packet;
    packet.packet() = std::vector<uint8_t>(
        reinterpret_cast<const uint8_t*>(request->data().data()),
        reinterpret_cast<const uint8_t*>(request->data().data()) + request->data().size());
    packets.push_back(std::move(packet));

    auto result = fidl::Call(l2cap_connection_)->Send({{.packets = std::move(packets)}});
    if (result.is_error()) {
      return Status(StatusCode::INTERNAL, "fuchsia.bluetooth.Channel/Send error: " +
                                              result.error_value().FormatDescription());
    }
  } else if (l2cap_socket_.is_valid()) {
    size_t actual;
    zx_status_t status =
        l2cap_socket_.write(/*options=*/0, request->data().data(), request->data().size(), &actual);
    if (status != ZX_OK) {
      return Status(StatusCode::INTERNAL, std::format("Failed to write to L2CAP socket: {}",
                                                      zx_status_get_string(status)));
    }
    if (actual != request->data().size()) {
      return Status(StatusCode::INTERNAL, std::format("Short write to L2CAP socket: {} vs {}",
                                                      actual, request->data().size()));
    }
  } else {
    return Status(StatusCode::FAILED_PRECONDITION, "L2CAP channel not connected");
  }

  response->mutable_success();
  return Status::OK;
}
