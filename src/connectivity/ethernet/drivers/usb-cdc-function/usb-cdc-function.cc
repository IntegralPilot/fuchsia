// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/ethernet/drivers/usb-cdc-function/usb-cdc-function.h"

#include <endian.h>
#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/defer.h>
#include <lib/trace/event.h>

#include <cstdint>
#include <cstring>
#include <vector>

#include <usb-endpoint/usb-endpoint-client.h>
#include <usb/request-fidl.h>

namespace usb_cdc_function {

namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace ffunction = fuchsia_hardware_usb_function;
namespace frequest = fuchsia_hardware_usb_request;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

namespace {

// Returns true if the given FIDL error represents an expected transport teardown or
// disconnection status (such as peer closure, cancellation, or device unplug) during
// driver shutdown.
//
// Generically handles:
// 1. Methods without domain errors (where error_value() is fidl::Error with .status()).
// 2. Methods with domain errors (where error_value() is fidl::ErrorsIn<Method> with
//    .is_framework_error() and .is_domain_error()).
// 3. Raw zx_status_t integer status codes.
template <typename ErrorType>
bool IsExpectedFidlDisconnect(const ErrorType &error) {
  constexpr auto is_disconnect_status = [](zx_status_t status) {
    return status == ZX_ERR_PEER_CLOSED || status == ZX_ERR_CANCELED ||
           status == ZX_ERR_IO_NOT_PRESENT;
  };

  if constexpr (requires {
                  error.is_framework_error();
                  error.framework_error().status();
                  error.is_domain_error();
                  error.domain_error();
                }) {
    if (error.is_framework_error()) {
      return is_disconnect_status(error.framework_error().status());
    }
    if (error.is_domain_error()) {
      return is_disconnect_status(error.domain_error());
    }
    return false;
  } else if constexpr (requires {
                         error.is_framework_error();
                         error.framework_error().status();
                       }) {
    if (error.is_framework_error()) {
      return is_disconnect_status(error.framework_error().status());
    }
    return false;
  } else if constexpr (requires {
                         error.is_domain_error();
                         error.domain_error();
                       }) {
    if (error.is_domain_error()) {
      return is_disconnect_status(error.domain_error());
    }
    return false;
  } else if constexpr (requires { error.status(); }) {
    return is_disconnect_status(error.status());
  } else if constexpr (std::is_integral_v<ErrorType>) {
    return is_disconnect_status(error);
  } else {
    return false;
  }
}
}  // namespace

zx_status_t UsbCdcFunction::cdc_generate_mac_address() {
  zx::result result =
      fdf_metadata::GetMetadataIfExists<fuchsia_boot_metadata::MacAddressMetadata>(incoming());
  if (result.is_error()) {
    fdf::error("Failed to get MAC address metadata: {}", result);
    return result.status_value();
  }
  if (result.value().has_value()) {
    const auto &metadata = result.value().value();
    if (!metadata.mac_address().has_value()) {
      fdf::error("MAC address metadata missing mac_address field");
      return ZX_ERR_INTERNAL;
    }
    mac_addr_ = metadata.mac_address().value().octets();
  } else {
    fdf::info("ethernet MAC metadata not found. Generating random address");

    zx_cprng_draw(mac_addr_.data(), mac_addr_.size());
    mac_addr_[0] = 0x02;
  }

  char buffer[sizeof(mac_addr_) * 3];
  snprintf(buffer, sizeof(buffer), "%02X%02X%02X%02X%02X%02X", mac_addr_[0], mac_addr_[1],
           mac_addr_[2], mac_addr_[3], mac_addr_[4], mac_addr_[5]);
  mac_addr_string_ = buffer;
  // Make the host and device addresses different so packets are routed
  // correctly.
  mac_addr_[5] ^= 1;

  return ZX_OK;
}

void UsbCdcFunction::DiscardPendingTxBuffers(zx_status_t status) {
  if (tx_completion_queue_.empty()) {
    return;
  }
  if (!netdevice_ifc_.is_valid()) {
    while (!tx_completion_queue_.empty()) {
      tx_completion_queue_.pop();
    }
    return;
  }

  fdf::Arena arena(kArenaTag);
  const size_t count = tx_completion_queue_.size();
  fidl::VectorView<fnetdev::wire::TxResult> results(arena, count);
  for (size_t i = 0; i < count; ++i) {
    uint32_t id = tx_completion_queue_.front();
    tx_completion_queue_.pop();
    results[i] = {.id = id, .status = status};
  }

  fidl::OneWayStatus fidl_status = netdevice_ifc_.buffer(arena)->CompleteTx(results);
  if (!fidl_status.ok()) {
    fdf::error("Failed to complete tx: {}", fidl_status.FormatDescription());
  }
}

void UsbCdcFunction::ReturnPendingRxSpace() {
  if (rx_space_buffers_.empty()) {
    return;
  }
  if (!netdevice_ifc_.is_valid()) {
    while (!rx_space_buffers_.empty()) {
      rx_space_buffers_.pop();
    }
    return;
  }

  fdf::Arena arena(kArenaTag);
  const size_t count = rx_space_buffers_.size();
  fidl::VectorView<fnetdev::wire::RxBuffer> rx_buffers(arena, count);
  fidl::VectorView<fnetdev::wire::RxBufferPart> rx_buffers_parts(arena, count);

  for (size_t i = 0; i < count; ++i) {
    rx_buffers_parts[i] = {
        .id = rx_space_buffers_.front().id,
        .offset = 0,
        .length = 0,
    };
    rx_space_buffers_.pop();
    rx_buffers[i] = {
        .meta =
            {
                .port = kPortId,
                .frame_type = fuchsia_hardware_network::FrameType::kEthernet,
            },
        .data =
            fidl::VectorView<fnetdev::wire::RxBufferPart>::FromExternal(&rx_buffers_parts[i], 1),
    };
  }

  fidl::OneWayStatus fidl_status = netdevice_ifc_.buffer(arena)->CompleteRx(rx_buffers);
  if (!fidl_status.ok()) {
    fdf::error("Failed to complete rx: {}", fidl_status.FormatDescription());
  }
}

void UsbCdcFunction::CdcIntrComplete(std::vector<fendpoint::Completion> completions) {
  for (auto &completion : completions) {
    intr_ep_.PutRequest(usb::FidlRequest{std::move(completion.request().value())});
  }

  if (unbound_.load()) {
    CheckStopComplete();
  } else if (pending_notification_ && intr_ep_.GetInFlightCount() == 0) {
    pending_notification_ = false;
    CdcSendNotifications();
  }
  CheckSetConfiguredDone();
}

void UsbCdcFunction::CdcSendNotifications() {
  if (unbound_.load() || !configured_ || !intr_ep_.client().is_valid()) {
    return;
  }

  if (intr_ep_.GetInFlightCount() > 0) {
    pending_notification_ = true;
    return;
  }
  usb_cdc_notification_t network_notification = {
      .bmRequestType = USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
      .bNotification = USB_CDC_NC_NETWORK_CONNECTION,
      .wValue = online_,
      .wIndex = descriptors_.cdc_intf_0.b_interface_number,
      .wLength = 0,
  };

  usb_cdc_speed_change_notification_t speed_notification = {
      .notification =
          {
              .bmRequestType = USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
              .bNotification = USB_CDC_NC_CONNECTION_SPEED_CHANGE,
              .wValue = 0,
              .wIndex = descriptors_.cdc_intf_0.b_interface_number,
              .wLength = 2 * sizeof(uint32_t),
          },
      .downlink_br = 0,
      .uplink_br = 0,
  };

  if (online_) {
    if (speed_ == fdescriptor::UsbSpeed::kSuper) {
      // Claim to be gigabit speed.
      speed_notification.downlink_br = speed_notification.uplink_br = 1000 * 1000 * 1000;
    } else {
      // Claim to be 100 megabit speed.
      speed_notification.downlink_br = speed_notification.uplink_br = 100 * 1000 * 1000;
    }
  } else {
    speed_notification.downlink_br = speed_notification.uplink_br = 0;
  }
  std::optional<usb::FidlRequest> req = intr_ep_.GetRequest();
  if (!req.has_value()) {
    fdf::error("[bug] intr_ep_.GetRequest(): no request available");
    return;
  }

  req->clear_buffers();
  zx::result<std::vector<size_t>> actual = req->CachedCopyTo(
      0, &network_notification, sizeof(network_notification), intr_ep_.GetMapped());
  if (actual.is_error()) {
    fdf::error("CdcSendNotifications: CachedCopyTo failed: {}", actual.status_string());
    intr_ep_.PutRequest(std::move(req.value()));
    return;
  }

  size_t actual_total = 0;
  for (size_t i = 0; i < actual->size(); i++) {
    req.value()->data()->at(i).size((*actual)[i]);
    actual_total += (*actual)[i];
  }
  if (actual_total != sizeof(network_notification)) {
    fdf::error("CdcSendNotifications: incomplete copy for network_notification");
    intr_ep_.PutRequest(std::move(req.value()));
    return;
  }
  std::optional<usb::FidlRequest> req2 = intr_ep_.GetRequest();
  if (!req2.has_value()) {
    fdf::error("[bug] intr_ep_.GetRequest(): no request available");
    intr_ep_.PutRequest(std::move(req.value()));
    return;
  }

  actual =
      req2->CachedCopyTo(0, &speed_notification, sizeof(speed_notification), intr_ep_.GetMapped());
  if (actual.is_error()) {
    fdf::error("CdcSendNotifications: CachedCopyTo failed: {}", actual.status_string());
    intr_ep_.PutRequest(std::move(req.value()));
    intr_ep_.PutRequest(std::move(req2.value()));
    return;
  }

  actual_total = 0;
  for (size_t i = 0; i < actual->size(); i++) {
    req2.value()->data()->at(i).size((*actual)[i]);
    actual_total += (*actual)[i];
  }
  if (actual_total != sizeof(speed_notification)) {
    fdf::error("CdcSendNotifications: incomplete copy for speed_notification");
    intr_ep_.PutRequest(std::move(req.value()));
    intr_ep_.PutRequest(std::move(req2.value()));
    return;
  }

  fdf::Arena arena(kArenaTag);
  std::array<frequest::wire::Request, 2> reqs = {
      fidl::ToWire(arena, req->take_request()),
      fidl::ToWire(arena, req2->take_request()),
  };
  fidl::OneWayStatus queue_status = intr_ep_.client().wire()->QueueRequests(
      fidl::VectorView<frequest::wire::Request>::FromExternal(reqs.data(), reqs.size()));
  if (!queue_status.ok()) {
    fdf::error("[bug] intr_ep_->QueueRequests(): {}", queue_status.FormatDescription());
    for (auto &r : reqs) {
      intr_ep_.PutRequest(usb::FidlRequest(fidl::ToNatural(r)));
    }
  }
}

void UsbCdcFunction::CdcRxComplete(std::vector<fendpoint::Completion> completions) {
  if (unbound_.load()) {
    for (auto &completion : completions) {
      bulk_out_ep_.PutRequest(usb::FidlRequest{std::move(completion.request().value())});
    }
    CheckStopComplete();
    return;
  }
  ProcessRxCompletions(std::move(completions));
  CheckSetConfiguredDone();
  CheckSetInterfaceDone();
}

void UsbCdcFunction::ProcessRxCompletions(std::vector<fendpoint::Completion> completions) {
  if (completions.empty()) {
    return;
  }
  fdf::Arena arena(kArenaTag);
  const size_t count = completions.size();

  fidl::VectorView<frequest::wire::Request> reqs(arena, count);
  size_t reqs_count = 0;

  fidl::VectorView<fnetdev::wire::RxBuffer> rx_buffers(arena, count);
  fidl::VectorView<fnetdev::wire::RxBufferPart> rx_buffers_parts(arena, count);
  size_t rx_buffers_count = 0;

  auto reset_and_enqueue = [&](usb::FidlRequest req) {
    req.reset_buffers(bulk_out_ep_.GetMapped());
    reqs[reqs_count++] = fidl::ToWire(arena, req.take_request());
  };

  for (auto &completion : completions) {
    zx_status_t status = *completion.status();
    if (status == ZX_ERR_IO_NOT_PRESENT || status == ZX_ERR_CANCELED || !online_ || !configured_) {
      bulk_out_ep_.PutRequest(usb::FidlRequest{std::move(completion.request().value())});
      continue;
    }

    if (status != ZX_OK) {
      fdf::error("[bug] rx_completion: {}", zx_status_get_string(status));
      usb::FidlRequest req(std::move(completion.request().value()));
      bulk_out_inspect_.AddFailedRxBytes(req.length());
      reset_and_enqueue(std::move(req));
      continue;
    }

    if (rx_space_buffers_.empty()) {
      rx_completion_queue_.push_back(std::move(completion));
      continue;
    }

    if (!completion.request().has_value()) {
      fdf::error("rx completion missing request");
      continue;
    }
    usb::FidlRequest req(std::move(completion.request().value()));
    const size_t request_length = completion.transfer_size().value_or(0);
    bulk_out_inspect_.AddRxBytes(request_length);

    fnetdev::wire::RxSpaceBuffer space = rx_space_buffers_.front();

    auto *stored_vmo = vmo_store_.GetVmo(space.region.vmo);
    if (!stored_vmo) {
      fdf::error("rx space with unknown vmo {}", space.region.vmo);
      reset_and_enqueue(std::move(req));
      continue;
    }

    if (request_length > space.region.length || space.region.offset > stored_vmo->data().size() ||
        request_length > stored_vmo->data().size() - space.region.offset) {
      fdf::error("rx buffer region out of bounds: offset {} length {} vmo size {}",
                 space.region.offset, space.region.length, stored_vmo->data().size());
      reset_and_enqueue(std::move(req));
      continue;
    }

    if (zx::result<std::vector<size_t>> res = req.CachedCopyFrom(
            0, reinterpret_cast<void *>(stored_vmo->data().data() + space.region.offset),
            request_length, bulk_out_ep_.GetMapped());
        res.is_error()) {
      fdf::error("failed to copy rx data: {}", res.status_string());
      reset_and_enqueue(std::move(req));
      continue;
    }

    rx_buffers_parts[rx_buffers_count] = fnetdev::wire::RxBufferPart{
        .id = space.id,
        .offset = 0,
        .length = static_cast<uint32_t>(request_length),
    };
    rx_buffers[rx_buffers_count] = {
        .meta =
            {
                .port = kPortId,
                .frame_type = fuchsia_hardware_network::FrameType::kEthernet,
            },
        .data = fidl::VectorView<fnetdev::wire::RxBufferPart>::FromExternal(
            &rx_buffers_parts[rx_buffers_count], 1),
    };

    rx_buffers_count++;
    rx_space_buffers_.pop();

    reset_and_enqueue(std::move(req));
  }

  if (reqs_count > 0) {
    fidl::OneWayStatus queue_status = bulk_out_ep_.client().wire()->QueueRequests(
        fidl::VectorView<frequest::wire::Request>::FromExternal(reqs.data(), reqs_count));
    if (!queue_status.ok()) {
      fdf::error("failed to queue rx requests: {}", queue_status.FormatDescription());
      for (size_t i = 0; i < reqs_count; ++i) {
        bulk_out_ep_.PutRequest(usb::FidlRequest(fidl::ToNatural(reqs[i])));
      }
    }
  }

  if (rx_buffers_count > 0 && netdevice_ifc_.is_valid()) {
    fidl::OneWayStatus queue_status = netdevice_ifc_.buffer(arena)->CompleteRx(
        fidl::VectorView<fnetdev::wire::RxBuffer>::FromExternal(rx_buffers.data(),
                                                                rx_buffers_count));
    if (!queue_status.ok()) {
      fdf::error("failed to complete rx buffers: {}", queue_status.FormatDescription());
    }
  }
  bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
}

void UsbCdcFunction::CdcTxComplete(std::vector<fendpoint::Completion> completions) {
  if (unbound_.load()) {
    for (auto &completion : completions) {
      bulk_in_ep_.PutRequest(usb::FidlRequest{std::move(completion.request().value())});
    }
    CheckStopComplete();
    return;
  }
  fdf::Arena arena(kArenaTag);
  const size_t count = completions.size();
  fidl::VectorView<fnetdev::wire::TxResult> results(arena, count);
  size_t results_count = 0;
  for (auto &completion : completions) {
    zx_status_t status = *completion.status();
    usb::FidlRequest req(std::move(completion.request().value()));
    size_t size = req.length();
    bulk_in_ep_.PutRequest(std::move(req));
    if (status != ZX_OK) {
      fdf::debug("tx completion error: {}", zx_status_get_string(status));
      bulk_in_inspect_.AddFailedTxBytes(size);
    } else {
      bulk_in_inspect_.AddTxBytes(completion.transfer_size().value_or(0));
    }
    if (tx_completion_queue_.empty()) {
      fdf::error("received tx completion without pending tx");
      continue;
    }
    const uint32_t tx_id = tx_completion_queue_.front();
    results[results_count++] = {.id = tx_id, .status = status};
    tx_completion_queue_.pop();
  }
  if (results_count > 0 && netdevice_ifc_.is_valid()) {
    fidl::OneWayStatus status = netdevice_ifc_.buffer(arena)->CompleteTx(
        fidl::VectorView<fnetdev::wire::TxResult>::FromExternal(results.data(), results_count));
    if (!status.ok()) {
      fdf::error("CompleteTx() failed: {}", status.FormatDescription());
    }
  }
  bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
  CheckSetConfiguredDone();
  CheckSetInterfaceDone();
}

void UsbCdcFunction::Control(ControlRequest &request, ControlCompleter::Sync &completer) {
  auto setup = request.setup();
  uint16_t w_value = setup.w_value();
  uint16_t w_index = setup.w_index();
  uint16_t w_length = setup.w_length();

  fdf::debug(
      "bmRequestType={:02x} bRequest={:02x} wValue={:04x} ({}) "
      "wIndex={:04x} ({}) wLength={:04x} ({})",
      setup.bm_request_type(), setup.b_request(), w_value, w_value, w_index, w_index, w_length,
      w_length);
  TRACE_DURATION("cdc_eth", __func__);

  if (setup.bm_request_type() == (USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE) &&
      setup.b_request() == USB_CDC_SET_ETHERNET_PACKET_FILTER) {
    fdf::debug("setting packet filter not supported");
    completer.Reply(zx::ok(std::vector<uint8_t>{}));
    return;
  }

  if (setup.bm_request_type() == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT) &&
      setup.b_request() == USB_REQ_CLEAR_FEATURE && setup.w_value() == USB_ENDPOINT_HALT) {
    fdf::debug("clearing endpoint-halt not supported");
    completer.Reply(zx::ok(std::vector<uint8_t>{}));
    return;
  }

  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

struct UsbCdcFunction::SetConfiguredSharedState
    : public std::enable_shared_from_this<SetConfiguredSharedState> {
  bool intr_cancelled = false;
  bool bulk_in_cancelled = false;
  bool bulk_out_cancelled = false;
  bool target_configured = false;
  fdescriptor::UsbSpeed target_speed = fdescriptor::UsbSpeed::kUndefined;
  std::optional<UsbCdcFunction::SetConfiguredCompleter::Async> completer;
  UsbCdcFunction *self = nullptr;

  ~SetConfiguredSharedState() {
    if (completer.has_value()) {
      completer->Reply(zx::error(ZX_ERR_INTERNAL));
    }
  }

  void Cancel() {
    self = nullptr;
    if (completer.has_value()) {
      auto c = std::move(*completer);
      completer.reset();
      c.Reply(zx::error(ZX_ERR_CANCELED));
    }
  }

  void CheckDone() {
    // CheckDone() is serialized on the driver dispatcher loop.
    if (!self || !completer.has_value()) {
      return;
    }
    if (self->unbound_.load()) {
      Cancel();
      return;
    }

    self->DrainRxCompletionQueue();

    bool intr_full = self->intr_ep_.RequestsFull();
    bool bulk_in_full = self->bulk_in_ep_.RequestsFull();
    bool bulk_out_full = self->bulk_out_ep_.RequestsFull();

    if (!intr_cancelled || !intr_full) {
      return;
    }
    if (!bulk_in_cancelled || !bulk_in_full) {
      return;
    }
    if (!bulk_out_cancelled || !bulk_out_full) {
      return;
    }

    auto async_completer = std::move(*completer);
    completer.reset();
    auto self_shared = shared_from_this();

    if (!target_configured) {
      self->DisableAllEndpoints(
          [self_shared, completer = std::move(async_completer)](zx_status_t status) mutable {
            if (status != ZX_OK) {
              fdf::error("DisableAllEndpoints failed: {}", zx_status_get_string(status));
            }
            if (!self_shared->self || self_shared->self->unbound_.load() ||
                self_shared->self->set_configured_state_ != self_shared) {
              completer.Reply(zx::error(ZX_ERR_CANCELED));
              return;
            }
            self_shared->self->DiscardPendingTxBuffers(ZX_ERR_CANCELED);
            self_shared->self->ReturnPendingRxSpace();
            self_shared->self->DrainRxCompletionQueue();
            self_shared->self->speed_ = fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined;
            self_shared->self->configured_ = false;
            if (self_shared->self->set_configured_state_ == self_shared) {
              self_shared->self->set_configured_state_.reset();
            }
            completer.Reply(zx::ok());
          });
    } else {
      // Host-initiated soft reset (true -> true):
      // All pending requests from the previous session have returned. Discard remaining buffers.
      self->DiscardPendingTxBuffers(ZX_ERR_CANCELED);
      self->ReturnPendingRxSpace();
      self->DrainRxCompletionQueue();

      if (!self->async_function_.is_valid()) {
        fdf::error("async_function_ is not valid during soft reset ConfigureEndpoint");
        self->configured_ = false;
        self->speed_ = fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined;
        if (self->set_configured_state_ == self_shared) {
          self->set_configured_state_.reset();
        }
        async_completer.Reply(zx::error(ZX_ERR_BAD_STATE));
        return;
      }

      ffunction::EndpointConfiguration ep_config;
      ffunction::EndpointDescriptor desc;
      desc.bm_attributes(self->descriptors_.intr_ep.bm_attributes);
      desc.w_max_packet_size(le16toh(self->descriptors_.intr_ep.w_max_packet_size));
      desc.b_interval(self->descriptors_.intr_ep.b_interval);
      ep_config.descriptor(std::move(desc));

      self->async_function_
          ->ConfigureEndpoint({self->descriptors_.intr_ep.b_endpoint_address, std::move(ep_config)})
          .Then([self_shared, completer = std::move(async_completer)](
                    fidl::Result<fuchsia_hardware_usb_function::UsbFunction::ConfigureEndpoint>
                        &result) mutable {
            if (!self_shared->self || self_shared->self->unbound_.load() ||
                self_shared->self->set_configured_state_ != self_shared) {
              completer.Reply(zx::error(ZX_ERR_CANCELED));
              return;
            }
            if (result.is_error()) {
              fdf::error("[bug] ConfigureEndpoint(intr): {}",
                         result.error_value().FormatDescription());
              self_shared->self->configured_ = false;
              self_shared->self->speed_ = fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined;
              if (self_shared->self->set_configured_state_ == self_shared) {
                self_shared->self->set_configured_state_.reset();
              }
              completer.Reply(zx::error(ZX_ERR_INTERNAL));
              return;
            }
            self_shared->self->speed_ = self_shared->target_speed;
            self_shared->self->configured_ = true;
            self_shared->self->CdcSendNotifications();
            if (self_shared->self->set_configured_state_ == self_shared) {
              self_shared->self->set_configured_state_.reset();
            }
            completer.Reply(zx::ok());
          });
    }
  }
};

void UsbCdcFunction::CheckSetConfiguredDone() {
  if (set_configured_state_) {
    set_configured_state_->CheckDone();
  }
}

void UsbCdcFunction::SetConfigured(SetConfiguredRequest &request,
                                   SetConfiguredCompleter::Sync &completer) {
  bool configured = request.configured();
  fdescriptor::UsbSpeed speed = request.speed();
  TRACE_DURATION("cdc_eth", __func__, "configured", configured, "speed",
                 static_cast<uint32_t>(speed));
  // Prevent a race with teardown, don't do any work if we're going away.
  if (unbound_.load()) {
    completer.Reply(zx::error(ZX_ERR_CANCELED));
    return;
  }

  // If already unconfigured and requesting unconfigured (false -> false), reply immediately.
  if (configured_ == configured && !configured && !set_configured_state_) {
    completer.Reply(zx::ok());
    return;
  }

  // Cancel any prior in-flight SetConfigured and SetInterface transitions.
  if (set_configured_state_) {
    set_configured_state_->Cancel();
    set_configured_state_.reset();
  }
  CancelSetInterface();

  // Instantly isolate the data plane from new packets
  online_ = false;
  online_property_.Set(false);
  UpdatePortStatus();

  fdf::info("configured = {}", configured);

  // Fresh configuration (false -> true): endpoints are not active yet, configure interrupt ep.
  if (configured && !configured_) {
    if (!async_function_.is_valid()) {
      fdf::error("async_function_ is not valid in SetConfigured");
      completer.Reply(zx::error(ZX_ERR_BAD_STATE));
      return;
    }
    auto async_completer = completer.ToAsync();
    auto state = std::make_shared<SetConfiguredSharedState>();
    state->completer = std::move(async_completer);
    state->self = this;
    state->target_configured = true;
    state->target_speed = speed;
    set_configured_state_ = state;

    ffunction::EndpointConfiguration ep_config;
    ffunction::EndpointDescriptor desc;
    desc.bm_attributes(descriptors_.intr_ep.bm_attributes);
    desc.w_max_packet_size(le16toh(descriptors_.intr_ep.w_max_packet_size));
    desc.b_interval(descriptors_.intr_ep.b_interval);
    ep_config.descriptor(std::move(desc));

    async_function_
        ->ConfigureEndpoint({descriptors_.intr_ep.b_endpoint_address, std::move(ep_config)})
        .Then([state](fidl::Result<fuchsia_hardware_usb_function::UsbFunction::ConfigureEndpoint>
                          &result) mutable {
          if (!state->self || state->self->unbound_.load() ||
              state->self->set_configured_state_ != state) {
            state->Cancel();
            return;
          }
          if (result.is_error()) {
            fdf::error("[bug] ConfigureEndpoint(intr): {}",
                       result.error_value().FormatDescription());
            if (state->self->set_configured_state_ == state) {
              state->self->set_configured_state_.reset();
            }
            if (state->completer.has_value()) {
              auto c = std::move(*state->completer);
              state->completer.reset();
              c.Reply(zx::error(ZX_ERR_INTERNAL));
            }
            return;
          }
          state->self->speed_ = state->target_speed;
          state->self->configured_ = true;
          state->self->CdcSendNotifications();
          if (state->self->set_configured_state_ == state) {
            state->self->set_configured_state_.reset();
          }
          if (state->completer.has_value()) {
            auto c = std::move(*state->completer);
            state->completer.reset();
            c.Reply(zx::ok());
          }
        });
    return;
  }

  // Either deconfiguring (true -> false) or host soft-reset (true -> true):
  // Endpoints were previously configured. Asynchronously cancel and drain active requests.
  pending_notification_ = false;
  auto async_completer = completer.ToAsync();

  auto state = std::make_shared<SetConfiguredSharedState>();
  state->completer = std::move(async_completer);
  state->self = this;
  state->target_configured = configured;
  state->target_speed = speed;
  set_configured_state_ = state;

  if (!intr_ep_.client().is_valid() || intr_ep_.RequestsFull()) {
    state->intr_cancelled = true;
  } else {
    intr_ep_->CancelAll().Then(
        [state](fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::CancelAll> &result) mutable {
          if (!result.is_ok() && !IsExpectedFidlDisconnect(result.error_value())) {
            fdf::warn("intr ep CancelAll failed: {}", result.error_value().FormatDescription());
          }
          state->intr_cancelled = true;
          state->CheckDone();
        });
  }

  if (!bulk_out_ep_.client().is_valid() || bulk_out_ep_.RequestsFull()) {
    state->bulk_out_cancelled = true;
  } else {
    bulk_out_ep_->CancelAll().Then(
        [state](fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::CancelAll> &result) mutable {
          if (!result.is_ok() && !IsExpectedFidlDisconnect(result.error_value())) {
            fdf::warn("bulk out ep CancelAll failed: {}", result.error_value().FormatDescription());
          }
          state->bulk_out_cancelled = true;
          state->CheckDone();
        });
  }

  if (!bulk_in_ep_.client().is_valid() || bulk_in_ep_.RequestsFull()) {
    state->bulk_in_cancelled = true;
  } else {
    bulk_in_ep_->CancelAll().Then(
        [state](fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::CancelAll> &result) mutable {
          if (!result.is_ok() && !IsExpectedFidlDisconnect(result.error_value())) {
            fdf::warn("bulk in ep CancelAll failed: {}", result.error_value().FormatDescription());
          }
          state->bulk_in_cancelled = true;
          state->CheckDone();
        });
  }

  state->CheckDone();
}

struct UsbCdcFunction::SetInterfaceSharedState
    : public std::enable_shared_from_this<SetInterfaceSharedState> {
  uint8_t target_alt_setting = 0;
  bool bulk_in_cancelled = false;
  bool bulk_out_cancelled = false;
  bool transition_started = false;
  std::optional<UsbCdcFunction::SetInterfaceCompleter::Async> completer;
  UsbCdcFunction *self = nullptr;

  ~SetInterfaceSharedState() { Reply(zx::error(ZX_ERR_INTERNAL)); }

  void Reply(zx::result<> result) {
    if (completer.has_value()) {
      auto c = std::move(*completer);
      completer.reset();
      c.Reply(result);
    }
  }

  void Cancel() {
    self = nullptr;
    Reply(zx::error(ZX_ERR_CANCELED));
  }

  bool IsActive() const {
    return self && !self->unbound_.load() && self->configured_ && !self->set_configured_state_ &&
           self->set_interface_state_.get() == this;
  }

  void CheckDone() {
    // CheckDone() is serialized on the driver dispatcher loop.
    if (!self || !completer.has_value() || transition_started) {
      return;
    }
    if (!IsActive()) {
      Cancel();
      return;
    }
    self->DrainRxCompletionQueue();

    bool bulk_in_full = self->bulk_in_ep_.RequestsFull();
    bool bulk_out_full = self->bulk_out_ep_.RequestsFull();

    if (!bulk_in_cancelled || !bulk_in_full) {
      return;
    }
    if (!bulk_out_cancelled || !bulk_out_full) {
      return;
    }

    transition_started = true;
    auto self_shared = shared_from_this();

    if (!self->async_function_.is_valid()) {
      fdf::error("async_function_ is not valid in SetInterface");
      if (self->set_interface_state_ == self_shared) {
        self->set_interface_state_.reset();
      }
      Reply(zx::error(ZX_ERR_BAD_STATE));
      return;
    }

    if (target_alt_setting == 0) {
      struct DisableState {
        int pending_calls = 2;
        zx_status_t status = ZX_OK;
      };
      auto disable_state = std::make_shared<DisableState>();

      for (const uint8_t ep_addr : {self->BulkOutAddress(), self->BulkInAddress()}) {
        self->async_function_->DisableEndpoint({ep_addr}).Then(
            [ep_addr, disable_state,
             self_shared](fidl::Result<fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>
                              &result) mutable {
              if (result.is_error()) {
                if (!IsExpectedFidlDisconnect(result.error_value())) {
                  fdf::error("Failed to disable endpoint {}: {}", ep_addr,
                             result.error_value().FormatDescription());
                  if (disable_state->status == ZX_OK) {
                    disable_state->status = ZX_ERR_INTERNAL;
                  }
                }
              }
              if (!self_shared->IsActive()) {
                self_shared->Reply(zx::error(ZX_ERR_CANCELED));
                return;
              }
              disable_state->pending_calls--;
              if (disable_state->pending_calls == 0) {
                self_shared->self->DiscardPendingTxBuffers(ZX_ERR_CANCELED);
                self_shared->self->ReturnPendingRxSpace();
                self_shared->self->DrainRxCompletionQueue();
                zx_status_t reply_status = disable_state->status;
                if (self_shared->self->set_interface_state_ == self_shared) {
                  self_shared->self->set_interface_state_.reset();
                }
                self_shared->Reply(zx::make_result(reply_status));
              }
            });
      }
    } else {
      self->DiscardPendingTxBuffers(ZX_ERR_CANCELED);
      self->ReturnPendingRxSpace();
      self->DrainRxCompletionQueue();

      StepDisableBulkOut();
    }
  }

  void StepDisableBulkOut() {
    ffunction::EndpointConfiguration bulk_out_config;
    {
      ffunction::EndpointDescriptor desc;
      desc.bm_attributes(self->descriptors_.bulk_out_ep.bm_attributes);
      desc.w_max_packet_size(le16toh(self->descriptors_.bulk_out_ep.w_max_packet_size));
      desc.b_interval(self->descriptors_.bulk_out_ep.b_interval);
      bulk_out_config.descriptor(std::move(desc));
    }

    auto self_shared = shared_from_this();
    self->async_function_->DisableEndpoint({self->BulkOutAddress()})
        .Then([self_shared, bulk_out_config = std::move(bulk_out_config)](
                  fidl::Result<fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>
                      &result) mutable {
          if (!self_shared->IsActive()) {
            self_shared->Reply(zx::error(ZX_ERR_CANCELED));
            return;
          }
          if (result.is_error() && !IsExpectedFidlDisconnect(result.error_value())) {
            fdf::error("Failed to disable endpoint {}: {}", self_shared->self->BulkOutAddress(),
                       result.error_value().FormatDescription());
          }
          self_shared->StepConfigureBulkOut(std::move(bulk_out_config));
        });
  }

  void StepConfigureBulkOut(ffunction::EndpointConfiguration bulk_out_config) {
    auto self_shared = shared_from_this();
    self->async_function_->ConfigureEndpoint({self->BulkOutAddress(), std::move(bulk_out_config)})
        .Then([self_shared](
                  fidl::Result<fuchsia_hardware_usb_function::UsbFunction::ConfigureEndpoint>
                      &result) mutable {
          if (!self_shared->IsActive()) {
            self_shared->Reply(zx::error(ZX_ERR_CANCELED));
            return;
          }
          if (result.is_error()) {
            if (!IsExpectedFidlDisconnect(result.error_value())) {
              fdf::error("[bug] ConfigureEndpoint(bulk_out) failed: {}",
                         result.error_value().FormatDescription());
            }
            if (self_shared->self->set_interface_state_ == self_shared) {
              self_shared->self->set_interface_state_.reset();
            }
            self_shared->Reply(zx::error(ZX_ERR_INTERNAL));
            return;
          }
          self_shared->StepDisableBulkIn();
        });
  }

  void StepDisableBulkIn() {
    auto self_shared = shared_from_this();
    self->async_function_->DisableEndpoint({self->BulkInAddress()})
        .Then(
            [self_shared](fidl::Result<fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint>
                              &result) mutable {
              if (!self_shared->IsActive()) {
                self_shared->Reply(zx::error(ZX_ERR_CANCELED));
                return;
              }
              if (result.is_error() && !IsExpectedFidlDisconnect(result.error_value())) {
                fdf::error("Failed to disable endpoint {}: {}", self_shared->self->BulkInAddress(),
                           result.error_value().FormatDescription());
              }
              self_shared->StepConfigureBulkIn();
            });
  }

  void StepConfigureBulkIn() {
    ffunction::EndpointConfiguration bulk_in_config;
    {
      ffunction::EndpointDescriptor desc;
      desc.bm_attributes(self->descriptors_.bulk_in_ep.bm_attributes);
      desc.w_max_packet_size(le16toh(self->descriptors_.bulk_in_ep.w_max_packet_size));
      desc.b_interval(self->descriptors_.bulk_in_ep.b_interval);
      bulk_in_config.descriptor(std::move(desc));
    }

    auto self_shared = shared_from_this();
    self->async_function_->ConfigureEndpoint({self->BulkInAddress(), std::move(bulk_in_config)})
        .Then([self_shared](
                  fidl::Result<fuchsia_hardware_usb_function::UsbFunction::ConfigureEndpoint>
                      &result) mutable {
          if (!self_shared->IsActive()) {
            self_shared->Reply(zx::error(ZX_ERR_CANCELED));
            return;
          }
          if (result.is_error()) {
            if (!IsExpectedFidlDisconnect(result.error_value())) {
              fdf::error("[bug] ConfigureEndpoint(bulk_in) failed: {}",
                         result.error_value().FormatDescription());
            }
            if (self_shared->self->set_interface_state_ == self_shared) {
              self_shared->self->set_interface_state_.reset();
            }
            self_shared->Reply(zx::error(ZX_ERR_INTERNAL));
            return;
          }
          self_shared->StepCompleteAltSetting1();
        });
  }

  void StepCompleteAltSetting1() {
    // Set online and update port status BEFORE queueing rx requests so that any
    // completions that fire immediately are processed normally rather than dropped.
    self->online_ = true;
    self->online_property_.Set(true);
    self->UpdatePortStatus();
    self->CdcSendNotifications();

    fdf::Arena arena(kArenaTag);
    std::vector<usb::FidlRequest> popped_reqs;
    popped_reqs.reserve(kRxDepth);
    while (!self->bulk_out_ep_.RequestsEmpty()) {
      auto req = self->bulk_out_ep_.GetRequest();
      if (!req.has_value()) {
        fdf::error("Expected available bulk out request but none found");
        break;
      }
      popped_reqs.push_back(std::move(*req));
    }
    const size_t count = popped_reqs.size();
    fidl::VectorView<frequest::wire::Request> reqs(arena, count);
    for (size_t i = 0; i < count; ++i) {
      popped_reqs[i].reset_buffers(self->bulk_out_ep_.GetMapped());
      reqs[i] = fidl::ToWire(arena, popped_reqs[i].take_request());
    }
    auto self_shared = shared_from_this();
    if (count > 0) {
      if (!self->bulk_out_ep_.client().is_valid()) {
        fdf::error("bulk_out_ep_ client is invalid when queueing rx requests");
        for (size_t i = 0; i < count; ++i) {
          self->bulk_out_ep_.PutRequest(usb::FidlRequest(fidl::ToNatural(reqs[i])));
        }
        if (self->set_interface_state_ == self_shared) {
          self->set_interface_state_.reset();
        }
        Reply(zx::error(ZX_ERR_INTERNAL));
        return;
      }
      fidl::OneWayStatus queue_status = self->bulk_out_ep_.client().wire()->QueueRequests(reqs);
      if (!queue_status.ok()) {
        fdf::error("Failed to queue rx requests: {}", queue_status.FormatDescription());
        for (size_t i = 0; i < count; ++i) {
          self->bulk_out_ep_.PutRequest(usb::FidlRequest(fidl::ToNatural(reqs[i])));
        }
        if (self->set_interface_state_ == self_shared) {
          self->set_interface_state_.reset();
        }
        Reply(zx::error(ZX_ERR_INTERNAL));
        return;
      }
    }
    if (self->set_interface_state_ == self_shared) {
      self->set_interface_state_.reset();
    }
    Reply(zx::ok());
  }
};

void UsbCdcFunction::CancelSetInterface() {
  if (set_interface_state_) {
    set_interface_state_->Cancel();
    set_interface_state_.reset();
  }
}

void UsbCdcFunction::CheckSetInterfaceDone() {
  if (set_interface_state_) {
    set_interface_state_->CheckDone();
  }
}

void UsbCdcFunction::SetInterface(SetInterfaceRequest &request,
                                  SetInterfaceCompleter::Sync &completer) {
  uint8_t interface = request.interface();
  uint8_t alt_setting = request.alt_setting();

  if (unbound_.load()) {
    completer.Reply(zx::error(ZX_ERR_CANCELED));
    return;
  }

  if (!configured_ || set_configured_state_) {
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  // The communication interface only supports alternative setting 0.
  // No endpoint configuration is required for this interface.
  if (interface == descriptors_.comm_intf.b_interface_number) {
    if (alt_setting != 0) {
      completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
      return;
    }
    completer.Reply(zx::ok());
    return;
  }

  if (interface != descriptors_.cdc_intf_0.b_interface_number || alt_setting > 1) {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  // Cancel any prior in-flight SetInterface transition.
  CancelSetInterface();

  // Instantly isolate the data plane from new packets!
  online_ = false;
  online_property_.Set(false);
  UpdatePortStatus();
  if (alt_setting == 0) {
    CdcSendNotifications();
  }

  auto async_completer = completer.ToAsync();
  auto state = std::make_shared<SetInterfaceSharedState>();
  state->target_alt_setting = alt_setting;
  state->completer = std::move(async_completer);
  state->self = this;
  set_interface_state_ = state;

  if (!bulk_out_ep_.client().is_valid() || bulk_out_ep_.RequestsFull()) {
    state->bulk_out_cancelled = true;
  } else {
    bulk_out_ep_.client()->CancelAll().Then(
        [state](fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::CancelAll> &result) mutable {
          if (!result.is_ok() && !IsExpectedFidlDisconnect(result.error_value())) {
            fdf::warn("bulk out ep CancelAll failed: {}", result.error_value().FormatDescription());
          }
          state->bulk_out_cancelled = true;
          state->CheckDone();
        });
  }

  if (!bulk_in_ep_.client().is_valid() || bulk_in_ep_.RequestsFull()) {
    state->bulk_in_cancelled = true;
  } else {
    bulk_in_ep_.client()->CancelAll().Then(
        [state](fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::CancelAll> &result) mutable {
          if (!result.is_ok() && !IsExpectedFidlDisconnect(result.error_value())) {
            fdf::warn("bulk in ep CancelAll failed: {}", result.error_value().FormatDescription());
          }
          state->bulk_in_cancelled = true;
          state->CheckDone();
        });
  }

  state->CheckDone();
}

void UsbCdcFunction::handle_unknown_method(
    fidl::UnknownMethodMetadata<ffunction::UsbFunctionInterface> metadata,
    fidl::UnknownMethodCompleter::Sync &completer) {
  fdf::error("Unknown method %ld", metadata.method_ordinal);
}

// NetworkDeviceImpl protocol:
zx::result<> UsbCdcFunction::Start(fdf::DriverContext context) {
  unbound_.store(false);
  online_ = false;
  configured_ = false;
  pending_notification_ = false;
  deconfigure_called_ = false;
  deconfigure_completed_ = false;
  intr_cancelled_ = false;
  bulk_in_cancelled_ = false;
  bulk_out_cancelled_ = false;
  set_configured_state_.reset();
  set_interface_state_.reset();
  stop_completer_.reset();

  inspector_ = context.CreateInspector(this);
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result result = incoming()->Connect<ffunction::UsbFunctionService::Device>();
  if (result.is_error()) {
    fdf::error("could not connect to UsbFunctionService: {}", result.status_string());
    return result.take_error();
  }
  function_.Bind(std::move(*result));

  zx::result async_client = incoming()->Connect<ffunction::UsbFunctionService::Device>();
  if (async_client.is_error()) {
    fdf::error("could not connect async UsbFunctionService: {}", async_client.status_string());
    return async_client.take_error();
  }
  async_function_.Bind(std::move(*async_client), dispatcher());

  zx_status_t status = cdc_generate_mac_address();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  zx::result intr_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (intr_endpoints.is_error()) {
    return intr_endpoints.take_error();
  }
  std::vector<ffunction::EndpointResource> resources;
  resources.push_back(ffunction::EndpointResource(
      fdescriptor::EndpointDirection::kIn, std::move(intr_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithInterrupt({}), INTR_MAX_PACKET));

  zx::result bulk_in_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (bulk_in_endpoints.is_error()) {
    return bulk_in_endpoints.take_error();
  }
  resources.push_back(ffunction::EndpointResource(
      fdescriptor::EndpointDirection::kIn, std::move(bulk_in_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), BULK_MAX_PACKET));

  zx::result bulk_out_endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  if (bulk_out_endpoints.is_error()) {
    return bulk_out_endpoints.take_error();
  }
  resources.push_back(ffunction::EndpointResource(
      fdescriptor::EndpointDirection::kOut, std::move(bulk_out_endpoints->server),
      fuchsia_hardware_usb_endpoint::EndpointInfo::WithBulk({}), BULK_MAX_PACKET));

  fidl::Request<ffunction::UsbFunction::AllocResources> alloc_req;
  alloc_req.interface_count(2);
  alloc_req.endpoints(std::move(resources));
  alloc_req.strings({{mac_addr_string_}});

  fidl::Result alloc_result = function_->AllocResources(std::move(alloc_req));
  if (alloc_result.is_error()) {
    fdf::error("AllocResources failed: {}", alloc_result.error_value().FormatDescription());
    return zx::error(alloc_result.error_value().is_framework_error()
                         ? alloc_result.error_value().framework_error().status()
                         : alloc_result.error_value().domain_error());
  }

  auto &response = alloc_result.value();
  descriptors_.comm_intf.b_interface_number = response.interface_nums()[0];
  descriptors_.cdc_intf_0.b_interface_number = response.interface_nums()[1];
  descriptors_.cdc_intf_1.b_interface_number = descriptors_.cdc_intf_0.b_interface_number;
  descriptors_.cdc_union.bControlInterface = descriptors_.comm_intf.b_interface_number;
  descriptors_.cdc_union.bSubordinateInterface = descriptors_.cdc_intf_0.b_interface_number;

  descriptors_.intr_ep.b_endpoint_address = response.endpoint_addrs()[0];
  descriptors_.bulk_in_ep.b_endpoint_address = response.endpoint_addrs()[1];
  descriptors_.bulk_out_ep.b_endpoint_address = response.endpoint_addrs()[2];
  descriptors_.cdc_eth.iMACAddress = response.string_indices()[0];

  status = intr_ep_.Init(std::move(intr_endpoints->client), dispatcher());
  if (status != ZX_OK) {
    fdf::error("[bug] intr_ep_.Init(): {}", zx_status_get_string(status));
    return zx::error(status);
  }

  size_t actual = intr_ep_.AddRequests(INTR_COUNT, BULK_REQ_SIZE,
                                       fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  if (actual != INTR_COUNT) {
    fdf::error("[bug] intr_ep_.AddRequests(): want {}, got {}", INTR_COUNT, actual);
    return zx::error(ZX_ERR_INTERNAL);
  }

  status = bulk_in_ep_.Init(std::move(bulk_in_endpoints->client), dispatcher());
  if (status != ZX_OK) {
    fdf::error("[bug] bulk_in_ep_.Init(): {}", zx_status_get_string(status));
    return zx::error(status);
  }

  actual = bulk_in_ep_.AddRequests(kTxDepth, BULK_REQ_SIZE,
                                   fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  if (actual != kTxDepth) {
    fdf::error("[bug] bulk_in_ep_.AddRequests(): want {}, got {}", kTxDepth, actual);
    return zx::error(ZX_ERR_INTERNAL);
  }

  status = bulk_out_ep_.Init(std::move(bulk_out_endpoints->client), dispatcher());
  if (status != ZX_OK) {
    fdf::error("[bug] bulk_out_ep_.Init(): {}", zx_status_get_string(status));
    return zx::error(status);
  }

  actual = bulk_out_ep_.AddRequests(kRxDepth, BULK_REQ_SIZE,
                                    fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  if (actual != kRxDepth) {
    fdf::error("[bug] bulk_out_ep_.AddRequests(): want {}, got {}", kRxDepth, actual);
    return zx::error(ZX_ERR_INTERNAL);
  }

  zx::result iface_endpoints = fidl::CreateEndpoints<ffunction::UsbFunctionInterface>();
  if (iface_endpoints.is_error()) {
    return iface_endpoints.take_error();
  }
  fidl::BindServer(dispatcher(), std::move(iface_endpoints->server), this);

  std::vector<uint8_t> descriptors_buffer(sizeof(descriptors_));
  memcpy(descriptors_buffer.data(), &descriptors_, sizeof(descriptors_));

  fidl::Request<ffunction::UsbFunction::Configure> config_req;
  config_req.configuration(std::move(descriptors_buffer));
  config_req.iface(std::move(iface_endpoints->client));

  fidl::Result config_res = function_->Configure(std::move(config_req));
  if (config_res.is_error()) {
    fdf::error("Configure failed: {}", config_res.error_value().FormatDescription());
    return zx::error(config_res.error_value().is_framework_error()
                         ? config_res.error_value().framework_error().status()
                         : config_res.error_value().domain_error());
  }

  if (zx_status_t status = vmo_store_.Reserve(fuchsia_hardware_network::wire::kMaxDataVmos);
      status != ZX_OK) {
    fdf::error("failed to initialize vmo store: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  inspect_node_ = inspector().root().CreateChild("usb-cdc-function");
  online_property_ = inspect_node_.CreateBool("online", online_);
  bulk_in_inspect_.Init(inspect_node_, "bulk_in");
  bulk_out_inspect_.Init(inspect_node_, "bulk_out");

  throughput_tracker_.emplace(dispatcher(), [this](zx::duration delta) {
    bulk_in_inspect_.MeasureThroughput(delta);
    bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
    bulk_out_inspect_.MeasureThroughput(delta);
    bulk_out_inspect_.UpdateRxQueue(bulk_out_ep_.GetInFlightCount());
  });
  throughput_tracker_->Start();

  if (zx::result result =
          child_.Initialize(incoming(), outgoing(), context.node_name(), "usb-cdc-netdev");
      result.is_error()) {
    fdf::error("Failed to initialize compat server: {}", result);
    return result.take_error();
  }

  // NetworkDeviceImpl service handler
  auto protocol = [this](fdf::ServerEnd<fnetdev::NetworkDeviceImpl> server_end) mutable {
    fdf::BindServer(driver_dispatcher()->get(), std::move(server_end), this);
  };
  fnetdev::Service::InstanceHandler handler({.network_device_impl = std::move(protocol)});

  if (auto status = outgoing()->AddService<fnetdev::Service>(std::move(handler));
      status.is_error()) {
    fdf::error("failed to add netdev service handler: {}", status.status_string());
    return status.take_error();
  }
  std::vector offers = child_.CreateOffers2();
  offers.push_back(fdf::MakeOffer2<fnetdev::Service>());

  zx::result controller = AddChild(
      "usb-cdc-netdev", cpp20::span<const fuchsia_driver_framework::NodeProperty2>{}, offers);
  if (controller.is_error()) {
    fdf::error("Failed to add child: {}", controller);
    return controller.take_error();
  }
  child_controller_ = std::move(controller.value());

  return zx::ok();
}

void UsbCdcFunction::DrainRxCompletionQueue() {
  for (auto &completion : rx_completion_queue_) {
    if (completion.request().has_value()) {
      bulk_out_ep_.PutRequest(usb::FidlRequest{std::move(completion.request().value())});
    }
  }
  rx_completion_queue_.clear();
}

void UsbCdcFunction::Stop(fdf::StopCompleter completer) {
  if (stop_completer_.has_value()) {
    fdf::warn("Stop() called while teardown is already in progress");
    completer(zx::ok());
    return;
  }
  if (throughput_tracker_) {
    throughput_tracker_->Stop();
  }
  unbound_.store(true);
  pending_notification_ = false;
  stop_completer_.emplace(std::move(completer));

  if (set_configured_state_) {
    set_configured_state_->Cancel();
    set_configured_state_.reset();
  }

  CancelSetInterface();

  // Reset cancellation and deconfigure flags
  intr_cancelled_ = false;
  bulk_in_cancelled_ = false;
  bulk_out_cancelled_ = false;
  deconfigure_called_ = false;
  deconfigure_completed_ = false;

  DrainRxCompletionQueue();
  online_ = false;

  DiscardPendingTxBuffers(ZX_ERR_CANCELED);
  ReturnPendingRxSpace();

  if (!intr_ep_.client().is_valid()) {
    intr_cancelled_ = true;
  } else {
    intr_ep_->CancelAll().Then(
        [this](fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::CancelAll> &result) mutable {
          if (!result.is_ok() && !IsExpectedFidlDisconnect(result.error_value())) {
            fdf::warn("intr ep CancelAll failed: {}", result.error_value().FormatDescription());
          }
          intr_cancelled_ = true;
          CheckStopComplete();
        });
  }

  if (!bulk_out_ep_.client().is_valid()) {
    bulk_out_cancelled_ = true;
  } else {
    bulk_out_ep_->CancelAll().Then(
        [this](fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::CancelAll> &result) mutable {
          if (!result.is_ok() && !IsExpectedFidlDisconnect(result.error_value())) {
            fdf::warn("bulk out ep CancelAll failed: {}", result.error_value().FormatDescription());
          }
          bulk_out_cancelled_ = true;
          CheckStopComplete();
        });
  }

  if (!bulk_in_ep_.client().is_valid()) {
    bulk_in_cancelled_ = true;
  } else {
    bulk_in_ep_->CancelAll().Then(
        [this](fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::CancelAll> &result) mutable {
          if (!result.is_ok() && !IsExpectedFidlDisconnect(result.error_value())) {
            fdf::warn("bulk in ep CancelAll failed: {}", result.error_value().FormatDescription());
          }
          bulk_in_cancelled_ = true;
          CheckStopComplete();
        });
  }

  CheckStopComplete();
}

void UsbCdcFunction::CheckStopComplete() {
  // CheckStopComplete() is serialized on the driver dispatcher loop.
  if (!stop_completer_.has_value()) {
    return;
  }

  DrainRxCompletionQueue();
  ReturnPendingRxSpace();

  bool intr_full = intr_ep_.RequestsFull();
  bool bulk_in_full = bulk_in_ep_.RequestsFull();
  bool bulk_out_full = bulk_out_ep_.RequestsFull();

  fdf::info(
      "CheckStopComplete state: intr(full:{}, canc:{}), bulk_in(full:{}, canc:{}), bulk_out(full:{}, canc:{})",
      intr_full, intr_cancelled_, bulk_in_full, bulk_in_cancelled_, bulk_out_full,
      bulk_out_cancelled_);

  if (!intr_full || !intr_cancelled_) {
    return;
  }
  if (!bulk_in_full || !bulk_in_cancelled_) {
    return;
  }
  if (!bulk_out_full || !bulk_out_cancelled_) {
    return;
  }

  if (!deconfigure_called_) {
    deconfigure_called_ = true;
    fdf::info("All endpoints quiet and cancelled. Disabling and deconfiguring...");
    DisableAllEndpoints([this](zx_status_t status) {
      if (status != ZX_OK) {
        fdf::error("DisableAllEndpoints failed: {}", zx_status_get_string(status));
      }
      if (async_function_.is_valid()) {
        async_function_->Deconfigure().Then(
            [this](fidl::Result<fuchsia_hardware_usb_function::UsbFunction::Deconfigure>
                       &result) mutable {
              if (!result.is_ok()) {
                if (!IsExpectedFidlDisconnect(result.error_value())) {
                  fdf::error("Deconfigure failed: {}", result.error_value().FormatDescription());
                }
              } else {
                fdf::info("Deconfigure completed successfully.");
              }
              deconfigure_completed_ = true;
              CheckStopComplete();
            });
      } else {
        deconfigure_completed_ = true;
        CheckStopComplete();
      }
    });
    return;
  }

  if (!deconfigure_completed_) {
    fdf::info("Waiting for deconfigure to complete");
    return;
  }

  fdf::info("Deconfigure complete. Invoking stop_completer_.");
  if (bulk_out_ep_.client().is_valid()) {
    bulk_out_ep_.Close();
  }
  if (bulk_in_ep_.client().is_valid()) {
    bulk_in_ep_.Close();
  }
  if (intr_ep_.client().is_valid()) {
    intr_ep_.Close();
  }
  // Reset all teardown flags so no stale state leaks if driver is reused or stopped again.
  deconfigure_called_ = false;
  deconfigure_completed_ = false;
  intr_cancelled_ = false;
  bulk_in_cancelled_ = false;
  bulk_out_cancelled_ = false;

  auto completer = std::move(*stop_completer_);
  stop_completer_.reset();
  completer(zx::ok());
}

void UsbCdcFunction::Init(fnetdev::wire::NetworkDeviceImplInitRequest *request, fdf::Arena &arena,
                          InitCompleter::Sync &completer) {
  netdevice_ifc_.Bind(std::move(request->iface), driver_dispatcher()->get());

  auto [client, server] = fdf::Endpoints<fnetdev::NetworkPort>::Create();
  fdf::BindServer(driver_dispatcher()->get(), std::move(server), this);

  // Add port 1
  netdevice_ifc_.buffer(arena)
      ->AddPort(kPortId, std::move(client))
      // Then exactly once so we're sure to complete this transaction even if
      // the dispatcher is shut down.
      .ThenExactlyOnce(
          [completer = completer.ToAsync()](
              fdf::WireUnownedResult<fnetdev::NetworkDeviceIfc::AddPort> &result) mutable {
            fdf::Arena arena(kArenaTag);
            if (!result.ok()) {
              fdf::error("AddPort failed: {}", result.FormatDescription());
              completer.buffer(arena).Reply(result.status());
              return;
            }
            completer.buffer(arena).Reply(result->status);
          });
}

void UsbCdcFunction::Start(fdf::Arena &arena, StartCompleter::Sync &completer) {
  UpdatePortStatus();
  completer.buffer(arena).Reply(ZX_OK);
}

void UsbCdcFunction::Stop(fdf::Arena &arena, StopCompleter::Sync &completer) {
  DiscardPendingTxBuffers(ZX_ERR_CANCELED);
  ReturnPendingRxSpace();
  completer.buffer(arena).Reply();
}

void UsbCdcFunction::GetInfo(
    fdf::Arena &arena,
    fdf::WireServer<fnetdev::NetworkDeviceImpl>::GetInfoCompleter::Sync &completer) {
  fnetdev::wire::DeviceImplInfo info = fnetdev::wire::DeviceImplInfo::Builder(arena)
                                           .tx_depth(kTxDepth)
                                           .rx_depth(kRxDepth)
                                           .rx_threshold(kRxDepth / 2)
                                           .max_buffer_parts(1)
                                           .max_buffer_length(BULK_REQ_SIZE)
                                           .buffer_alignment(1)
                                           .min_rx_buffer_length(ETH_MTU)
                                           .min_tx_buffer_length(0)
                                           .Build();

  completer.buffer(arena).Reply(info);
}

void UsbCdcFunction::QueueTx(fnetdev::wire::NetworkDeviceImplQueueTxRequest *request,
                             fdf::Arena &arena, QueueTxCompleter::Sync &completer) {
  const size_t count = request->buffers.size();
  fidl::VectorView<frequest::wire::Request> reqs(arena, count);
  size_t reqs_count = 0;
  fidl::VectorView<fnetdev::wire::TxResult> results(arena, count);
  size_t results_count = 0;
  fidl::VectorView<uint32_t> queued_ids(arena, count);

  for (const auto &buffer : request->buffers) {
    if (unbound_.load() || !online_) {
      results[results_count++] = {.id = buffer.id, .status = ZX_ERR_BAD_STATE};
      continue;
    }
    if (buffer.data.size() != 1) {
      fdf::warn("Invalid buffer data size {} for id {}", buffer.data.size(), buffer.id);
      results[results_count++] = {.id = buffer.id, .status = ZX_ERR_INVALID_ARGS};
      continue;
    }
    const auto &region = buffer.data[0];

    std::optional<usb::FidlRequest> tx_req = bulk_in_ep_.GetRequest();

    if (!tx_req.has_value()) {
      // Given we're matching our request depth to the netdevice depth, this
      // shouldn't happen.
      fdf::warn("No USB request available for id {}", buffer.id);
      results[results_count++] = {.id = buffer.id, .status = ZX_ERR_NO_RESOURCES};
      continue;
    }
    auto return_request = fit::defer([&]() { bulk_in_ep_.PutRequest(std::move(tx_req.value())); });

    auto *stored_vmo = vmo_store_.GetVmo(region.vmo);
    if (!stored_vmo) {
      fdf::warn("No VMO found for id {}", region.vmo);
      results[results_count++] = {.id = buffer.id, .status = ZX_ERR_INVALID_ARGS};
      continue;
    }
    auto data = stored_vmo->data();
    if (region.length == 0) {
      results[results_count++] = {.id = buffer.id, .status = ZX_OK};
      continue;
    }
    if (region.length > data.size() || region.offset > data.size() - region.length) {
      fdf::warn("Invalid VMO region for id {}", region.vmo);
      results[results_count++] = {.id = buffer.id, .status = ZX_ERR_INVALID_ARGS};
      continue;
    }

    tx_req->clear_buffers();
    zx::result<std::vector<size_t>> actual = tx_req->CachedCopyTo(
        0, data.data() + region.offset, region.length, bulk_in_ep_.GetMapped());
    if (actual.is_error()) {
      fdf::warn("failed to copy data and flush cache: {}", actual.status_string());
      results[results_count++] = {.id = buffer.id, .status = actual.error_value()};
      continue;
    }
    size_t actual_total = 0;
    for (size_t i = 0; i < actual->size(); i++) {
      (*tx_req)->data()->at(i).size((*actual)[i]);
      actual_total += (*actual)[i];
    }
    // CDC always needs a short packet to terminate the transfer.
    (*tx_req)->short_(true);
    if (actual_total != region.length) {
      fdf::warn("failed to copy all data {} {}", actual_total, region.length);
      results[results_count++] = {.id = buffer.id, .status = ZX_ERR_INTERNAL};
      continue;
    }

    return_request.cancel();
    queued_ids[reqs_count] = buffer.id;
    reqs[reqs_count++] = fidl::ToWire(arena, tx_req->take_request());
  }

  if (results_count > 0 && netdevice_ifc_.is_valid()) {
    fidl::OneWayStatus status = netdevice_ifc_.buffer(arena)->CompleteTx(
        fidl::VectorView<fnetdev::wire::TxResult>::FromExternal(results.data(), results_count));
    if (!status.ok()) {
      fdf::error("failed to complete tx: {}", status.FormatDescription());
    }
  }

  if (reqs_count > 0) {
    fidl::OneWayStatus queue_status = bulk_in_ep_.client().wire()->QueueRequests(
        fidl::VectorView<frequest::wire::Request>::FromExternal(reqs.data(), reqs_count));

    if (!queue_status.ok()) {
      fdf::error("failed to queue tx requests: {}", queue_status.FormatDescription());
      for (size_t i = 0; i < reqs_count; ++i) {
        bulk_in_ep_.PutRequest(usb::FidlRequest(fidl::ToNatural(reqs[i])));
      }
      fidl::VectorView<fnetdev::wire::TxResult> fail_results(arena, reqs_count);
      for (size_t i = 0; i < reqs_count; ++i) {
        fail_results[i] = {.id = queued_ids[i], .status = ZX_ERR_INTERNAL};
      }
      if (netdevice_ifc_.is_valid()) {
        fidl::OneWayStatus fail_status = netdevice_ifc_.buffer(arena)->CompleteTx(fail_results);
        if (!fail_status.ok()) {
          fdf::error("failed to complete failed tx requests: {}", fail_status.FormatDescription());
        }
      }
    } else {
      for (size_t i = 0; i < reqs_count; ++i) {
        tx_completion_queue_.push(queued_ids[i]);
      }
    }
  }
  bulk_in_inspect_.UpdateTxQueue(bulk_in_ep_.GetInFlightCount());
}

void UsbCdcFunction::QueueRxSpace(fnetdev::wire::NetworkDeviceImplQueueRxSpaceRequest *request,
                                  fdf::Arena &arena, QueueRxSpaceCompleter::Sync &completer) {
  if (unbound_.load() || !online_) {
    const size_t count = request->buffers.size();
    fidl::VectorView<fnetdev::wire::RxBuffer> rx_buffers(arena, count);
    fidl::VectorView<fnetdev::wire::RxBufferPart> rx_buffers_parts(arena, count);
    for (size_t i = 0; i < count; ++i) {
      const auto &buffer = request->buffers[i];
      rx_buffers_parts[i] = {
          .id = buffer.id,
          .offset = 0,
          .length = 0,
      };
      rx_buffers[i] = {
          .meta =
              {
                  .port = kPortId,
                  .frame_type = fuchsia_hardware_network::FrameType::kEthernet,
              },
          .data =
              fidl::VectorView<fnetdev::wire::RxBufferPart>::FromExternal(&rx_buffers_parts[i], 1),
      };
    }
    if (count > 0 && netdevice_ifc_.is_valid()) {
      fidl::OneWayStatus fidl_status = netdevice_ifc_.buffer(arena)->CompleteRx(rx_buffers);
      if (!fidl_status.ok()) {
        fdf::error("Failed to complete rx: {}", fidl_status.FormatDescription());
      }
    }
    return;
  }

  for (const auto &buffer : request->buffers) {
    rx_space_buffers_.push(buffer);
  }

  if (rx_completion_queue_.empty()) {
    return;
  }
  // Take over all pending completions and process them. We'll re-queue if
  // not enough space available.
  ProcessRxCompletions(std::move(rx_completion_queue_));
}

void UsbCdcFunction::PrepareVmo(fnetdev::wire::NetworkDeviceImplPrepareVmoRequest *request,
                                fdf::Arena &arena, PrepareVmoCompleter::Sync &completer) {
  zx_status_t status = vmo_store_.RegisterWithKey(request->id, std::move(request->vmo));
  if (status != ZX_OK) {
    fdf::error("failed to register vmo {}: {}", request->id, zx_status_get_string(status));
  }
  completer.buffer(arena).Reply(status);
}

void UsbCdcFunction::ReleaseVmo(fnetdev::wire::NetworkDeviceImplReleaseVmoRequest *request,
                                fdf::Arena &arena, ReleaseVmoCompleter::Sync &completer) {
  zx::result status = vmo_store_.Unregister(request->id);
  if (!status.is_ok()) {
    fdf::error("failed to unregister vmo {}: {}", request->id, status.status_string());
  }
  completer.buffer(arena).Reply();
}

void UsbCdcFunction::GetInfo(
    fdf::Arena &arena, fdf::WireServer<fnetdev::NetworkPort>::GetInfoCompleter::Sync &completer) {
  static constexpr fuchsia_hardware_network::wire::FrameType kRxTypes[] = {
      fuchsia_hardware_network::wire::FrameType::kEthernet};
  static constexpr fuchsia_hardware_network::wire::FrameTypeSupport kTxTypes[] = {{
      .type = fuchsia_hardware_network::wire::FrameType::kEthernet,
      .features = fuchsia_hardware_network::wire::kFrameFeaturesRaw,
  }};

  fuchsia_hardware_network::wire::PortBaseInfo info =
      fuchsia_hardware_network::wire::PortBaseInfo::Builder(arena)
          .port_class(fuchsia_hardware_network::wire::PortClass::kEthernet)
          .rx_types(fidl::VectorView<fuchsia_hardware_network::wire::FrameType>::FromExternal(
              const_cast<fuchsia_hardware_network::wire::FrameType *>(kRxTypes), 1))
          .tx_types(
              fidl::VectorView<fuchsia_hardware_network::wire::FrameTypeSupport>::FromExternal(
                  const_cast<fuchsia_hardware_network::wire::FrameTypeSupport *>(kTxTypes), 1))
          .Build();

  completer.buffer(arena).Reply(info);
}

void UsbCdcFunction::GetStatus(fdf::Arena &arena, GetStatusCompleter::Sync &completer) {
  completer.buffer(arena).Reply(fidl::ToWire(arena, ReadStatus()));
}

void UsbCdcFunction::SetActive(fnetdev::wire::NetworkPortSetActiveRequest *request,
                               fdf::Arena &arena, SetActiveCompleter::Sync &completer) {}

void UsbCdcFunction::GetMac(fdf::Arena &arena, GetMacCompleter::Sync &completer) {
  auto [client, server] = fdf::Endpoints<fnetdev::MacAddr>::Create();
  fdf::BindServer(fdf::Dispatcher::GetCurrent()->get(), std::move(server), this);
  completer.buffer(arena).Reply(std::move(client));
}

void UsbCdcFunction::Removed(fdf::Arena &arena, RemovedCompleter::Sync &completer) {}

void UsbCdcFunction::GetAddress(fdf::Arena &arena, GetAddressCompleter::Sync &completer) {
  fuchsia_net::wire::MacAddress mac;
  memcpy(mac.octets.data(), mac_addr_.data(), mac_addr_.size());
  completer.buffer(arena).Reply(mac);
}

void UsbCdcFunction::GetFeatures(fdf::Arena &arena, GetFeaturesCompleter::Sync &completer) {
  fnetdev::wire::Features features =
      fnetdev::wire::Features::Builder(arena)
          .multicast_filter_count(0)
          .supported_modes(fnetdev::wire::SupportedMacFilterMode::kPromiscuous)
          .Build();
  completer.buffer(arena).Reply(features);
}

void UsbCdcFunction::SetMode(fnetdev::wire::MacAddrSetModeRequest *request, fdf::Arena &arena,
                             SetModeCompleter::Sync &completer) {
  completer.buffer(arena).Reply();
}

fuchsia_hardware_network::PortStatus UsbCdcFunction::ReadStatus() const {
  fuchsia_hardware_network::PortStatus status;

  status.mtu(ETH_MTU);
  fuchsia_hardware_network::StatusFlags flags;
  if (online_) {
    flags |= fuchsia_hardware_network::StatusFlags::kOnline;
  }
  status.flags(flags);
  return status;
}

void UsbCdcFunction::UpdatePortStatus() {
  if (netdevice_ifc_.is_valid()) {
    fdf::Arena arena(kArenaTag);
    fidl::OneWayStatus status =
        netdevice_ifc_.buffer(arena)->PortStatusChanged(kPortId, fidl::ToWire(arena, ReadStatus()));
    if (!status.ok()) {
      fdf::error("Failed to notify port status: {}", status.FormatDescription());
    }
  }
}

bool UsbCdcFunction::HasPendingRxCompletions() { return !rx_completion_queue_.empty(); }

void UsbCdcFunction::DisableAllEndpoints(fit::callback<void(zx_status_t)> callback) {
  fdf::info("Disabling all endpoints asynchronously...");
  if (!async_function_.is_valid()) {
    std::move(callback)(ZX_OK);
    return;
  }

  struct SharedState {
    size_t pending = 3;
    zx_status_t status = ZX_OK;
    fit::callback<void(zx_status_t)> callback;
  };
  auto state = std::make_shared<SharedState>();
  state->callback = std::move(callback);

  for (const uint8_t ep_addr : {InterruptAddress(), BulkOutAddress(), BulkInAddress()}) {
    async_function_->DisableEndpoint({ep_addr}).Then(
        [state, ep_addr](
            fidl::Result<fuchsia_hardware_usb_function::UsbFunction::DisableEndpoint> &result) {
          if (result.is_error()) {
            if (!IsExpectedFidlDisconnect(result.error_value())) {
              fdf::error("Failed to disable endpoint {}: {}", ep_addr,
                         result.error_value().FormatDescription());
              if (state->status == ZX_OK) {
                state->status = ZX_ERR_INTERNAL;
              }
            }
          }
          state->pending--;
          if (state->pending == 0 && state->callback) {
            std::move(state->callback)(state->status);
          }
        });
  }
}

}  // namespace usb_cdc_function

FUCHSIA_DRIVER_EXPORT2(usb_cdc_function::UsbCdcFunction);
