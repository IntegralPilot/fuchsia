// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "bt_transport_uart.h"

#include <assert.h>
#include <lib/async/default.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/sync/cpp/completion.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <format>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/serial/cpp/bind.h>

#include "lib/driver/component/cpp/node_add_args.h"
#include "src/connectivity/bluetooth/hci/transport/uart/cpp/bt_transport_uart_config.h"

namespace bt_transport_uart {

namespace fhbt = fuchsia_hardware_bluetooth;

ScoConnectionServer::ScoConnectionServer(SendHandler send_handler, StopHandler stop_handler,
                                         AckReceiveHandler ack_receive_handler)
    : send_handler_(std::move(send_handler)),
      stop_handler_(std::move(stop_handler)),
      ack_receive_handler_(std::move(ack_receive_handler)) {
  // Pre-allocate the vector size to avoid resizing. Reserve the space for packet indicator by +1 on
  // the size.
  write_buffer_.reserve(fhbt::kScoPacketMax + 1);
}

// fhbt::ScoConnection overrides.
void ScoConnectionServer::Send(SendRequest& request, SendCompleter::Sync& completer) {
  write_buffer_.push_back(BtHciPacketIndicator::kHciSco);
  write_buffer_.insert(write_buffer_.end(), request.packet().begin(), request.packet().end());
  send_handler_(write_buffer_, [completer = completer.ToAsync()]() mutable { completer.Reply(); });
  write_buffer_.clear();
}

void ScoConnectionServer::AckReceive(AckReceiveCompleter::Sync& completer) {
  ack_receive_handler_();
}

void ScoConnectionServer::Stop(StopCompleter::Sync& completer) { stop_handler_(); }

void ScoConnectionServer::handle_unknown_method(
    ::fidl::UnknownMethodMetadata<fhbt::ScoConnection> metadata,
    ::fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method in ScoConnection protocol, closing with ZX_ERR_NOT_SUPPORTED");
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

BtTransportUart::BtTransportUart()
    : DriverBase2("bt-transport-uart"),
      sco_connection_server_(fit::bind_member(this, &BtTransportUart::OnScoData),
                             fit::bind_member(this, &BtTransportUart::OnScoStop),
                             fit::bind_member(this, &BtTransportUart::OnAckReceive)) {
  // Pre-allocate the vector size to avoid resizing. Reserve the space for packet indicator by +1 on
  // the size.
  for (size_t i = 0; i < kSendQueueLimit; i++) {
    std::vector<uint8_t> buffer;
    buffer.reserve(fhbt::kAclPacketMax + 1);
    available_buffers_.push(std::move(buffer));
  }
}

zx::result<> BtTransportUart::Start(fdf::DriverContext context) {
  fdf::debug("Start");
  dispatcher_ = dispatcher();
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  const auto config = context.take_config<bt_transport_uart_config::Config>();

  zx::result<fdf::ClientEnd<fuchsia_hardware_serialimpl::Device>> client_end =
      incoming_->Connect<fuchsia_hardware_serialimpl::Service::Device>();
  if (client_end.is_error()) {
    fdf::error("Connect to fuchsia_hardware_serialimpl::Device protocol failed: {}", client_end);
    return zx::error(client_end.status_value());
  }

  serial_client_ = fdf::WireClient<fuchsia_hardware_serialimpl::Device>(
      std::move(client_end.value()), driver_dispatcher()->get());
  if (!serial_client_.is_valid()) {
    fdf::error("fuchsia_hardware_serialimpl::Device Client is not valid");
    return zx::error(ZX_ERR_BAD_HANDLE);
  }

  // pre-populate event packet indicators
  event_buffer_[0] = kHciEvent;
  event_buffer_offset_ = 1;
  acl_buffer_[0] = kHciAclData;
  acl_buffer_offset_ = 1;
  sco_buffer_[0] = kHciSco;
  sco_buffer_offset_ = 1;
  iso_buffer_[0] = kHciIso;
  iso_buffer_offset_ = 1;

  fdf::Arena arena('INIT');
  auto info_result = serial_client_.sync().buffer(arena)->GetInfo();
  if (!info_result.ok()) {
    fdf::error("hci_start: GetInfo failed with FIDL error {}", info_result.error());
    return zx::error(info_result.status());
  }
  if (info_result->is_error()) {
    fdf::error("hci_start: GetInfo failed with error {}",
               zx_status_get_string(info_result->error_value()));
    return zx::error(info_result->error_value());
  }

  if (info_result.value()->info.serial_class != fuchsia_hardware_serial::Class::kBluetoothHci) {
    fdf::error("hci_start: device class isn't BLUETOOTH_HCI");
    return zx::error(ZX_ERR_INTERNAL);
  }

  serial_pid_ = info_result.value()->info.serial_pid;

  auto enable_result = serial_client_.sync().buffer(arena)->Enable(true);
  if (!enable_result.ok()) {
    fdf::error("hci_start: Enable failed with FIDL error {}", enable_result.error());
    return zx::error(enable_result.status());
  }

  if (enable_result->is_error()) {
    fdf::error("hci_start: Enable failed with error {}",
               zx_status_get_string(enable_result->error_value()));
    return zx::error(enable_result->error_value());
  }

  zx_status_t status = ServeProtocols();
  if (status != ZX_OK) {
    fdf::error("Failed to serve protocols: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  if (zx::result result =
          mac_address_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), incoming_);
      result.is_error()) {
    fdf::error("Failed to forward mac address metadata: {}", result);
    return result.take_error();
  }

  // Add child node for the vendor driver to bind.
  // Build offers
  std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fhbt::HciService>(),
      fdf::MakeOffer2<fuchsia_hardware_serialimpl::Service>(),
  };
  std::optional mac_address_offer = mac_address_metadata_server_.CreateOffer();
  if (mac_address_offer.has_value()) {
    offers.push_back(std::move(mac_address_offer.value()));
  }

  if (config.enable_suspend()) {
    // Forward PowerTokenService to our parent if suspend is enabled.
    fuchsia_hardware_power::PowerTokenService::InstanceHandler handler({
        .token_provider =
            [this](fidl::ServerEnd<fuchsia_hardware_power::PowerTokenProvider> server) {
              zx::result<> result =
                  incoming_->Connect<fuchsia_hardware_power::PowerTokenService::TokenProvider>(
                      std::move(server));
              if (result.is_error()) {
                fdf::warn("Failed to connect to power token service: {}", result);
              }
            },
    });

    zx::result result = outgoing()->AddService<fuchsia_hardware_power::PowerTokenService>(
        std::move(handler), "default");
    if (result.is_error()) {
      fdf::error("Failed to add power token service: {}", result);
      return result.take_error();
    }

    offers.push_back(fdf::MakeOffer2<fuchsia_hardware_power::PowerTokenService>());
  }

  auto properties = std::to_array({
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.bluetooth.HciService"),
  });

  // Add bt-transport-uart child node.
  zx::result child = AddChild("bt-transport-uart", properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add bt-transport-uart node, error: {}", child);
    return child.take_error();
  }

  child_node_controller_.Bind(std::move(child.value()), dispatcher());

  // Add bt-transport-uart-impl child node.
  std::vector<fuchsia_driver_framework::Offer> impl_offers = {
      fdf::MakeOffer2<fuchsia_hardware_serialimpl::Service>(),
  };
  auto impl_properties = std::to_array({
      // Prevent the serial core driver from binding to our node.
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_serial::BIND_PROTOCOL_DEVICE),
  });
  zx::result impl_child = AddChild("bt-transport-uart-impl", impl_properties, impl_offers);
  if (impl_child.is_error()) {
    fdf::error("Failed to add bt-transport-uart-impl node, error: {}", impl_child);
    return impl_child.take_error();
  }
  impl_controller_ = std::move(impl_child.value());

  return zx::ok();
}

void BtTransportUart::Stop(fdf::StopCompleter completer) {
  fdf::trace("Unbind");

  // We are now shutting down.  Make sure that any pending callbacks in
  // flight from the serial_impl are nerfed and that our thread is shut down.
  std::atomic_store_explicit(&shutting_down_, true, std::memory_order_relaxed);

  // Finish by making sure that all in flight transactions transactions have
  // been canceled.
  fdf::Arena arena('CANC');
  auto result = serial_client_.sync().buffer(arena)->CancelAll();
  if (!result.ok()) {
    fdf::error("hci_prepare_stop: CancelAll failed with FIDL error {}", result.error());
    completer(zx::error(result.status()));
    return;
  }

  fdf::trace("Stop complete");

  completer(zx::ok());
}

size_t BtTransportUart::EventPacketLength() {
  // payload length is in byte 2 of the packet
  // add 3 bytes for packet indicator, event code and length byte
  return event_buffer_offset_ > 2 ? event_buffer_[2] + 3 : 0;
}

size_t BtTransportUart::AclPacketLength() {
  // length is in bytes 3 and 4 of the packet
  // add 5 bytes for packet indicator, control info and length fields
  return acl_buffer_offset_ > 4 ? (acl_buffer_[3] | (acl_buffer_[4] << 8)) + 5 : 0;
}

size_t BtTransportUart::ScoPacketLength() {
  // payload length is byte 3 of the packet
  // add 4 bytes for packet indicator, handle, and length byte
  return sco_buffer_offset_ > 3 ? (sco_buffer_[3] + 4) : 0;
}

size_t BtTransportUart::IsoPacketLength() {
  // length is in bytes 3 and 4 of the packet
  // add 5 bytes for packet indicator, handle, flags, and length fields
  return iso_buffer_offset_ > 4 ? (iso_buffer_[3] | ((iso_buffer_[4] & 0x3f) << 8)) + 5 : 0;
}

void BtTransportUart::SendSnoop(fidl::VectorView<uint8_t>& packet,
                                fuchsia_hardware_bluetooth::wire::SnoopPacket::Tag type,
                                fhbt::wire::PacketDirection direction) {
  if (!snoop_server_.has_value()) {
    return;
  }

  static bool log_emitted = false;
  if ((snoop_seq_ - acked_snoop_seq_) > kSeqNumMaxDiff) {
    if (!log_emitted) {
      fdf::warn(
          "Too many snoop packets not acked, current seq: {}, acked seq: {}, skipping snoop packets",
          snoop_seq_, acked_snoop_seq_);
      log_emitted = true;
    }
    return;
  }
  // Reset log when the acked sequence number catches up.
  log_emitted = false;

  fidl::Arena arena;
  auto builder = fhbt::wire::SnoopOnObservePacketRequest::Builder(arena);
  switch (type) {
    case fhbt::wire::SnoopPacket::Tag::kEvent:
      ZX_DEBUG_ASSERT(direction == fhbt::wire::PacketDirection::kControllerToHost);
      builder.packet(fhbt::wire::SnoopPacket::WithEvent(arena, packet));
      break;
    case fhbt::wire::SnoopPacket::Tag::kCommand:
      ZX_DEBUG_ASSERT(direction == fhbt::wire::PacketDirection::kHostToController);
      builder.packet(fhbt::wire::SnoopPacket::WithCommand(arena, packet));
      break;
    case fhbt::wire::SnoopPacket::Tag::kAcl:
      builder.packet(fhbt::wire::SnoopPacket::WithAcl(arena, packet));
      break;
    case fhbt::wire::SnoopPacket::Tag::kSco:
      builder.packet(fhbt::wire::SnoopPacket::WithSco(arena, packet));
      break;
    case fhbt::wire::SnoopPacket::Tag::kIso:
      builder.packet(fhbt::wire::SnoopPacket::WithIso(arena, packet));
      break;
    default:
      fdf::error("Unknown snoop packet type: {}", static_cast<fidl_xunion_tag_t>(type));
  }
  builder.direction(direction);
  builder.sequence(snoop_seq_++);

  fidl::OneWayStatus result = fidl::WireSendEvent(*snoop_server_)->OnObservePacket(builder.Build());
  if (!result.ok()) {
    fdf::error("Failed to send snoop for sent packet: {}, unbinding snoop server", result.error());
    snoop_server_->Close(ZX_ERR_INTERNAL);
    snoop_server_.reset();
  }
}

void BtTransportUart::HciBeginShutdown() {
  bool was_shutting_down = shutting_down_.exchange(true, std::memory_order_relaxed);
  if (!was_shutting_down) {
    (void)take_node();
  }
}

void BtTransportUart::OnScoData(std::vector<uint8_t>& packet, fit::function<void(void)> callback) {
  if (available_buffers_.empty()) {
    fdf::error("Send queue is full, closing SCO connection");
    callback();
    sco_connection_binding_.RemoveBindings(&sco_connection_server_);
    return;
  }

  auto& buffer = available_buffers_.front();
  buffer = std::move(packet);
  send_queue_.emplace(std::move(buffer), std::move(callback));
  available_buffers_.pop();
  send_queue_task_.Post(dispatcher_);
}

void BtTransportUart::OnScoStop() {
  sco_connection_binding_.RemoveBindings(&sco_connection_server_);
}

void BtTransportUart::ProcessOnePacketFromSendQueue() {
  if (!can_send_) {
    return;
  }
  if (send_queue_.empty()) {
    return;
  }

  auto& buffer_entry = send_queue_.front();
  SerialWrite(buffer_entry.data_, std::move(buffer_entry.callback_));

  auto snoop_data = fidl::VectorView<uint8_t>::FromExternal(buffer_entry.data_.data() + 1,
                                                            buffer_entry.data_.size() - 1);

  fhbt::wire::SnoopPacket::Tag snoop_type = fhbt::wire::SnoopPacket::Tag::kIso;
  switch (buffer_entry.data_[0]) {
    case BtHciPacketIndicator::kHciAclData:
      snoop_type = fhbt::wire::SnoopPacket::Tag::kAcl;
      break;
    case BtHciPacketIndicator::kHciCommand:
      snoop_type = fhbt::wire::SnoopPacket::Tag::kCommand;
      break;
    case BtHciPacketIndicator::kHciSco:
      snoop_type = fhbt::wire::SnoopPacket::Tag::kSco;
      break;
    case BtHciPacketIndicator::kHciIso:
      snoop_type = fhbt::wire::SnoopPacket::Tag::kIso;
      break;
    default:
      fdf::debug("Unsupported snoop sent packet type: {}", buffer_entry.data_[0]);
      send_queue_.pop();
      return;
  }
  SendSnoop(snoop_data, snoop_type, fhbt::wire::PacketDirection::kHostToController);
  buffer_entry.data_.clear();
  available_buffers_.push(std::move(buffer_entry.data_));
  send_queue_.pop();
}

void BtTransportUart::SerialWrite(const std::vector<uint8_t>& data,
                                  fit::function<void(void)> callback) {
  can_send_ = false;

  fdf::Arena arena('WRIT');
  auto data_view = fidl::VectorView<uint8_t>::FromExternal(const_cast<std::vector<uint8_t>&>(data));
  serial_client_.buffer(arena)->Write(data_view).ThenExactlyOnce(
      [this](fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Write>& result) mutable {
        if (!result.ok()) {
          fdf::error("hci_bind: Write failed with FIDL error {}", result.error());
          HciTransportWriteComplete(result.status());
          return;
        }

        if (result->is_error()) {
          fdf::error("hci_bind: Write failed with error {}",
                     zx_status_get_string(result->error_value()));
          HciTransportWriteComplete(result->error_value());
          return;
        }
        HciTransportWriteComplete(ZX_OK);
      });
  callback();
}

void BtTransportUart::HciHandleUartReadEvents(const uint8_t* buf, size_t length) {
  const uint8_t* const end = buf + length;
  while (buf < end) {
    if (cur_uart_packet_type_ == kHciNone) {
      // start of new packet. read packet type
      cur_uart_packet_type_ = static_cast<BtHciPacketIndicator>(*buf++);
    }
    switch (cur_uart_packet_type_) {
      case kHciEvent:
        ProcessNextUartPacketFromReadBuffer(event_buffer_, sizeof(event_buffer_),
                                            &event_buffer_offset_, &buf, end,
                                            &BtTransportUart::EventPacketLength, kHciEvent);
        break;
      case kHciAclData:
        ProcessNextUartPacketFromReadBuffer(acl_buffer_, sizeof(acl_buffer_), &acl_buffer_offset_,
                                            &buf, end, &BtTransportUart::AclPacketLength,
                                            kHciAclData);
        break;
      case kHciSco:
        ProcessNextUartPacketFromReadBuffer(sco_buffer_, sizeof(sco_buffer_), &sco_buffer_offset_,
                                            &buf, end, &BtTransportUart::ScoPacketLength, kHciSco);
        break;
      case kHciIso:
        ProcessNextUartPacketFromReadBuffer(iso_buffer_, sizeof(iso_buffer_), &iso_buffer_offset_,
                                            &buf, end, &BtTransportUart::IsoPacketLength, kHciIso);
        break;
      default:
        fdf::error("unsupported HCI packet type {} received. We may be out of sync",
                   uint8_t{cur_uart_packet_type_});
        cur_uart_packet_type_ = kHciNone;
        return;
    }
  }
}

void BtTransportUart::OnAckReceive() {
  if (unacked_receive_packet_number_ == 0) {
    fdf::error("Receiving a packet ack when no received packet is pending ack.");
    return;
  }
  unacked_receive_packet_number_--;
  if ((unacked_receive_packet_number_ < kUnackedReceivePacketRecover) && read_stopped_) {
    fdf::info("Restarting read with {} unacked packets", unacked_receive_packet_number_);
    // Resume reading data from the uart data buffer if half of the unacked packets are acked.
    queue_read_task_.Post(dispatcher_);
    read_stopped_ = false;
  }
}

void BtTransportUart::ProcessNextUartPacketFromReadBuffer(uint8_t* buffer, size_t buffer_size,
                                                          size_t* buffer_offset,
                                                          const uint8_t** uart_src,
                                                          const uint8_t* uart_end,
                                                          PacketLengthFunction get_packet_length,
                                                          BtHciPacketIndicator packet_ind) {
  size_t packet_length = (this->*get_packet_length)();

  while (!packet_length && *uart_src < uart_end) {
    // read until we have enough to compute packet length
    buffer[*buffer_offset] = **uart_src;
    (*buffer_offset)++;
    (*uart_src)++;
    packet_length = (this->*get_packet_length)();
  }

  // Out of bytes, but we still don't know the packet length.  Just wait for
  // the next packet.
  if (!packet_length) {
    return;
  }

  if (packet_length > buffer_size) {
    fdf::error(
        "packet_length is too large ({} > {}) during packet reassembly. Dropping and "
        "attempting to re-sync.",
        packet_length, buffer_size);

    // Reset the reassembly state machine.
    *buffer_offset = 1;
    cur_uart_packet_type_ = kHciNone;
    // Consume the rest of the UART buffer to indicate that it is corrupt.
    *uart_src = uart_end;
    return;
  }

  size_t remaining = uart_end - *uart_src;
  size_t copy_size = packet_length - *buffer_offset;
  copy_size = std::min(copy_size, remaining);

  ZX_ASSERT(*buffer_offset + copy_size <= buffer_size);
  memcpy(buffer + *buffer_offset, *uart_src, copy_size);
  *uart_src += copy_size;
  *buffer_offset += copy_size;

  if (*buffer_offset != packet_length) {
    // The packet is incomplete, the next chunk should continue the same packet.
    return;
  }

  fidl::Arena arena;
  auto fidl_vec = fidl::VectorView<uint8_t>::FromExternal(&buffer[1], packet_length - 1);
  if (packet_ind == kHciSco) {
    if (sco_connection_binding_.size() == 0) {
      fdf::debug("No SCO connection available for sending SCO packets up.");
    } else {
      sco_connection_binding_.ForEachBinding(
          [&](const fidl::ServerBinding<fhbt::ScoConnection>& binding) {
            fidl::OneWayStatus result = fidl::WireSendEvent(binding)->OnReceive(fidl_vec);

            if (!result.ok()) {
              fdf::error("Failed to send SCO packet to bt-host: {}", result.error());
            } else {
              unacked_receive_packet_number_++;
            }
          });
    }
  } else if (packet_ind == kHciAclData) {
    auto received_packet = fhbt::wire::ReceivedPacket::WithAcl(arena, fidl_vec);
    if (hci_transport_binding_.has_value()) {
      fidl::OneWayStatus result =
          fidl::WireSendEvent(hci_transport_binding_.value())->OnReceive(received_packet);
      if (!result.ok()) {
        fdf::error("Failed to send ACL packet to host: {}", result.error());
      } else {
        unacked_receive_packet_number_++;
      }
    } else {
      // Note that this likely happens during system shutdown, when the other end of the channel
      // has been shutdown but this driver haven't gotten into the PrepareStop() step. If it
      // doesn't happen during shutdown, this might indicate a bug in either the driver or the
      // other end of this FIDL connection.
      fdf::info("No HciTransport bindings available for sending up ACL packets");
    }
  } else if (packet_ind == kHciEvent) {
    auto received_packet = fhbt::wire::ReceivedPacket::WithEvent(arena, fidl_vec);
    if (hci_transport_binding_.has_value()) {
      fidl::OneWayStatus result =
          fidl::WireSendEvent(hci_transport_binding_.value())->OnReceive(received_packet);
      if (!result.ok()) {
        fdf::error("Failed to send event packet to host: {}", result.error());
      } else {
        unacked_receive_packet_number_++;
      }
    } else {
      // Note that this likely happens during system shutdown, when the other end of the channel
      // has been shutdown but this driver haven't gotten into the PrepareStop() step. If it
      // doesn't happen during shutdown, this might indicate a bug in either the driver or the
      // other end of this FIDL connection.
      fdf::info("No HciTransport bindings available for sending up event packets.");
    }
  } else if (packet_ind == kHciIso) {
    auto received_packet = fhbt::wire::ReceivedPacket::WithIso(arena, fidl_vec);
    if (hci_transport_binding_.has_value()) {
      fidl::OneWayStatus result =
          fidl::WireSendEvent(hci_transport_binding_.value())->OnReceive(received_packet);
      if (!result.ok()) {
        fdf::error("Failed to send ISO packet to host: {}", result.error());
      } else {
        unacked_receive_packet_number_++;
      }
    } else {
      // Note that this likely happens during system shutdown, when the other end of the channel
      // has been shutdown but this driver haven't gotten into the PrepareStop() step. If it
      // doesn't happen during shutdown, this might indicate a bug in either the driver or the
      // other end of this FIDL connection.
      fdf::info("No HciTransport bindings available for sending up ISO packets.");
    }
  } else {
    fdf::error("Unsupported packet type received");
  }

  fhbt::wire::SnoopPacket::Tag type = fhbt::wire::SnoopPacket::Tag::kIso;
  if (packet_ind == kHciAclData) {
    type = fhbt::wire::SnoopPacket::Tag::kAcl;
  } else if (packet_ind == kHciEvent) {
    type = fhbt::wire::SnoopPacket::Tag::kEvent;
  } else if (packet_ind == kHciSco) {
    type = fhbt::wire::SnoopPacket::Tag::kSco;
  } else if (packet_ind == kHciIso) {
    type = fhbt::wire::SnoopPacket::Tag::kIso;
  } else {
    fdf::error("Unsupported packet type for snoop.");
  }

  SendSnoop(fidl_vec, type, fhbt::wire::PacketDirection::kControllerToHost);

  // reset buffer
  cur_uart_packet_type_ = kHciNone;
  *buffer_offset = 1;
}

void BtTransportUart::HciReadComplete(zx_status_t status, const uint8_t* buffer, size_t length) {
  fdf::trace("Read complete with status: {}", zx_status_get_string(status));

  // If we are in the process of shutting down, we are done.
  if (atomic_load_explicit(&shutting_down_, std::memory_order_relaxed)) {
    return;
  }

  if (status == ZX_OK) {
    HciHandleUartReadEvents(buffer, length);
    if (unacked_receive_packet_number_ >= kUnackedReceivePacketLimit) {
      fdf::warn(
          "Too many unacked packets ({} > {}) sent to the host, stop fetching data from the bus temporarily",
          unacked_receive_packet_number_, kUnackedReceivePacketLimit);
      // Stop reading data from the uart buffer if there are too many unacked packets sent to the
      // host.
      read_stopped_ = true;
      return;
    }
    queue_read_task_.Post(dispatcher_);
  } else {
    // There is not much we can do in the event of a UART read error.  Do not
    // queue a read job and start the process of shutting down.
    fdf::error("Fatal UART read error ({}), shutting down", zx_status_get_string(status));
    HciBeginShutdown();
  }
}

void BtTransportUart::HciTransportWriteComplete(zx_status_t status) {
  fdf::trace("Write complete with status: {}", zx_status_get_string(status));

  // If we are in the process of shutting down, we are done as soon as we
  // have freed our operation.
  if (atomic_load_explicit(&shutting_down_, std::memory_order_relaxed)) {
    return;
  }

  if (status != ZX_OK) {
    HciBeginShutdown();
    return;
  }

  ZX_DEBUG_ASSERT(!can_send_);
  can_send_ = true;

  // Resume processing the data in queue.
  send_queue_task_.Post(dispatcher_);
}

// fhbt::HciTransport protocol overrides.
void BtTransportUart::Send(fidl::Server<fhbt::HciTransport>::SendRequest& request,
                           fidl::Server<fhbt::HciTransport>::SendCompleter::Sync& completer) {
  if (available_buffers_.empty()) {
    fdf::error("Send queue is full, closing HciTransport connection");
    completer.Reply();
    hci_transport_binding_->Close(ZX_ERR_INTERNAL);
    hci_transport_binding_.reset();
    return;
  }

  auto& buffer = available_buffers_.front();
  const char* data_type_name = nullptr;
  // Figure out which data type we are dealing with and the constants which go
  // along with it.
  switch (request.Which()) {
    case fhbt::SentPacket::Tag::kIso:
      buffer.push_back(kHciIso);
      buffer.insert(buffer.end(), request.iso().value().begin(), request.iso().value().end());
      data_type_name = "ISO";
      break;
    case fhbt::SentPacket::Tag::kAcl:
      buffer.push_back(kHciAclData);
      buffer.insert(buffer.end(), request.acl().value().begin(), request.acl().value().end());
      data_type_name = "ACL";
      break;
    case fhbt::SentPacket::Tag::kCommand:
      buffer.push_back(kHciCommand);
      buffer.insert(buffer.end(), request.command().value().begin(),
                    request.command().value().end());
      data_type_name = "command";
      break;
    default:
      fdf::error("Unknown packet type: {}", static_cast<fidl_xunion_tag_t>(request.Which()));
  }

  fdf::trace("received data type: {}", data_type_name);

  send_queue_.emplace(std::move(buffer),
                      [completer = completer.ToAsync()]() mutable { completer.Reply(); });
  available_buffers_.pop();
  send_queue_task_.Post(dispatcher_);
}

void BtTransportUart::AckReceive(
    fidl::Server<fhbt::HciTransport>::AckReceiveCompleter::Sync& completer) {
  OnAckReceive();
}

void BtTransportUart::ConfigureSco(
    fidl::Server<fhbt::HciTransport>::ConfigureScoRequest& request,
    fidl::Server<fhbt::HciTransport>::ConfigureScoCompleter::Sync& completer) {
  if (!request.connection().has_value()) {
    fdf::error("No ScoConnection server end received from the host.");
    return;
  }
  if (sco_connection_binding_.size() != 0) {
    fdf::error("ScoConnection connection exists.");
    return;
  }

  sco_connection_binding_.AddBinding(dispatcher_, std::move(request.connection().value()),
                                     &sco_connection_server_, fidl::kIgnoreBindingClosure);
  // Only useful in tests, it doesn't hurt to signal it when no one is waiting for it.
  sco_connection_setup_.Signal();
}

fit::function<void(void)> BtTransportUart::WaitForSnoopCallback() {
  return [this]() { snoop_setup_.Wait(); };
}

fit::function<void(void)> BtTransportUart::WaitForScoConnectionCallback() {
  return [this]() { sco_connection_setup_.Wait(); };
}

uint64_t BtTransportUart::GetAckedSnoopSeq() { return acked_snoop_seq_; }

bool BtTransportUart::HasScoConnection() const { return sco_connection_binding_.size() != 0; }

void BtTransportUart::handle_unknown_method(
    ::fidl::UnknownMethodMetadata<fhbt::HciTransport> metadata,
    ::fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method in HciTransport protocol, closing with ZX_ERR_NOT_SUPPORTED");
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void BtTransportUart::GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) {
  fuchsia_hardware_serial::wire::SerialPortInfo info{
      .serial_class = fuchsia_hardware_serial::wire::Class::kBluetoothHci,
      .serial_pid = serial_pid_,
  };
  completer.buffer(arena).ReplySuccess(info);
}
void BtTransportUart::Config(ConfigRequestView request, fdf::Arena& arena,
                             ConfigCompleter::Sync& completer) {
  auto result = serial_client_.sync().buffer(arena)->Config(request->baud_rate, request->flags);
  if (!result.ok()) {
    fdf::error("Config request failed with FIDL error {}", result.error());
    completer.buffer(arena).ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Config request failed with error {}", zx_status_get_string(result->error_value()));
    completer.buffer(arena).ReplyError(result->error_value());
    return;
  }
  completer.buffer(arena).ReplySuccess();
}

void BtTransportUart::Enable(EnableRequestView request, fdf::Arena& arena,
                             EnableCompleter::Sync& completer) {
  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void BtTransportUart::Read(fdf::Arena& arena, ReadCompleter::Sync& completer) {
  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void BtTransportUart::Write(WriteRequestView request, fdf::Arena& arena,
                            WriteCompleter::Sync& completer) {
  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void BtTransportUart::CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) {
  completer.buffer(arena).Reply();
}

void BtTransportUart::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error(
      "Unknown method in fuchsia_hardware_serialimpl::Device protocol, closing with ZX_ERR_NOT_SUPPORTED");
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void BtTransportUart::AcknowledgePackets(AcknowledgePacketsRequest& request,
                                         AcknowledgePacketsCompleter::Sync& completer) {
  acked_snoop_seq_ = std::max(request.sequence(), acked_snoop_seq_);
}

void BtTransportUart::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::Snoop> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method in fidl::Server<fuchsia_hardware_bluetooth::Snoop>");
}

void BtTransportUart::QueueUartRead() {
  fdf::Arena arena('READ');
  serial_client_.buffer(arena)->Read().ThenExactlyOnce([this](fdf::WireUnownedResult<
                                                              fuchsia_hardware_serialimpl::Device::
                                                                  Read>& result) {
    if (!result.ok()) {
      fdf::error("Read request failed with FIDL error {}", result.error());
      HciReadComplete(result.status(), nullptr, 0);
      return;
    }

    if (result->is_error()) {
      const zx_status_t error = result->error_value();
      if (error == ZX_ERR_CANCELED) {
        fdf::warn(
            "Read request is cancel by the bus driver, it's likely the serial driver is de-initialized.");
      } else {
        fdf::error("Read request failed with error {}",
                   zx_status_get_string(result->error_value()));
      }
      HciReadComplete(error, nullptr, 0);
      return;
    }
    HciReadComplete(ZX_OK, result->value()->data.data(), result->value()->data.size());
  });
}

zx_status_t BtTransportUart::ServeProtocols() {
  // Add HCI services to the outgoing directory.
  auto hci_transport_protocol = [this](fidl::ServerEnd<fhbt::HciTransport> server_end) mutable {
    if (hci_transport_binding_) {
      fdf::warn("HciTransport binding exists, replacing it.");
      // Don't queue another read task for the new binding.
    } else {
      fdf::info("HciTransport server binding.");
      queue_read_task_.Post(dispatcher_);
    }
    hci_transport_binding_.emplace(dispatcher_, std::move(server_end), this,
                                   [this](fidl::UnbindInfo) {
                                     hci_transport_binding_.reset();
                                     fdf::info("HciTransport server binding unbound.");
                                   });
  };
  auto snoop_protocol = [this](fidl::ServerEnd<fhbt::Snoop> server_end) mutable {
    if (snoop_server_.has_value()) {
      fdf::error("Snoop protocol connect with Snoop already active");
      return;
    }
    snoop_server_.emplace(dispatcher_, std::move(server_end), this,
                          [this](fidl::UnbindInfo) { snoop_server_.reset(); });
    // Only useful in tests, it doesn't hurt to signal it when no one is waiting for it.
    snoop_setup_.Signal();
  };

  fhbt::HciService::InstanceHandler hci_handler(
      {.hci_transport = std::move(hci_transport_protocol), .snoop = std::move(snoop_protocol)});

  auto status = outgoing()->AddService<fhbt::HciService>(std::move(hci_handler));
  if (status.is_error()) {
    fdf::error("Failed to add HCI service to outgoing directory: {}", status);
    return status.error_value();
  }

  // Add Serial service to the outgoing directory.
  auto serial_protocol =
      [this](fdf::ServerEnd<fuchsia_hardware_serialimpl::Device> server_end) mutable {
        fdf::BindServer(driver_dispatcher()->get(), std::move(server_end), this);
      };
  fuchsia_hardware_serialimpl::Service::InstanceHandler serial_handler(
      {.device = std::move(serial_protocol)});
  status = outgoing()->AddService<fuchsia_hardware_serialimpl::Service>(std::move(serial_handler));
  if (status.is_error()) {
    fdf::error("Failed to add Serial service to outgoing directory: {}", status);
    return status.error_value();
  }

  return ZX_OK;
}

}  // namespace bt_transport_uart

FUCHSIA_DRIVER_EXPORT2(bt_transport_uart::BtTransportUart);
