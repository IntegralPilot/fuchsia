// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "softmac_ifc_bridge.h"

#include <fidl/fuchsia.driver.framework/cpp/driver/fidl.h>
#include <fidl/fuchsia.wlan.softmac/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fidl/cpp/wire_natural_conversions.h>
#include <lib/fidl_driver/cpp/transport.h>
#include <lib/operation/ethernet.h>
#include <lib/sync/cpp/completion.h>
#include <lib/trace-engine/types.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <wlan/drivers/fidl_bridge.h>
#include <wlan/drivers/log.h>

#include "src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding/bindings.h"

namespace wlan::drivers::wlansoftmac {

using ::wlan::drivers::fidl_bridge::ForwardWireResult;

zx::result<std::unique_ptr<SoftmacIfcBridge>> SoftmacIfcBridge::New(
    fidl::SharedClient<fuchsia_driver_framework::Node> node_client,
    const ethernet_tx_t* ethernet_tx, const wlan_rx_t* wlan_rx,
    fdf::ServerEnd<fuchsia_wlan_softmac::WlanSoftmacIfc>&& server_endpoint,
    fidl::ClientEnd<fuchsia_wlan_softmac::WlanSoftmacIfcBridge>&&
        softmac_ifc_bridge_client_endpoint) {
  WLAN_TRACE_DURATION();
  auto softmac_ifc_bridge =
      std::unique_ptr<SoftmacIfcBridge>(new SoftmacIfcBridge(ethernet_tx, wlan_rx));

  {
    WLAN_LAMBDA_TRACE_DURATION("WlanSoftmacIfc server binding");
    softmac_ifc_bridge->softmac_ifc_server_binding_ =
        std::make_unique<fdf::ServerBinding<fuchsia_wlan_softmac::WlanSoftmacIfc>>(
            fdf::Dispatcher::GetCurrent()->get(), std::move(server_endpoint),
            softmac_ifc_bridge.get(),
            [node_client = std::move(node_client)](fidl::UnbindInfo info) mutable {
              WLAN_LAMBDA_TRACE_DURATION("WlanSoftmacIfc close_handler");
              if (info.is_user_initiated()) {
                fdf::info("WlanSoftmacIfc server closed.");
              } else {
                fdf::error("WlanSoftmacIfc unexpectedly closed: {}", info.lossy_description());

                // Initiate asynchronous teardown of the fuchsia.driver.framework/Node proxy
                // to cause the driver framework to stop this driver. Stopping this driver is
                // appropriate when this binding has an abnormal shutdown because otherwise this
                // driver would be in an unusable state.
                node_client.AsyncTeardown();
              }
            });
  }

  softmac_ifc_bridge->softmac_ifc_bridge_client_ =
      std::make_unique<fidl::Client<fuchsia_wlan_softmac::WlanSoftmacIfcBridge>>(
          std::move(softmac_ifc_bridge_client_endpoint),
          fdf::Dispatcher::GetCurrent()->async_dispatcher());

  return fit::ok(std::move(softmac_ifc_bridge));
}

void SoftmacIfcBridge::Recv(RecvRequestView fdf_request, fdf::Arena& arena,
                            RecvCompleter::Sync& completer) {
  trace_async_id_t async_id = TRACE_NONCE();
  WLAN_TRACE_ASYNC_BEGIN_RX(async_id);
  WLAN_TRACE_DURATION();

  auto raw_arena = arena.release();

  // Add a reference to the arena so the arena can be reused for the reply as well.
  fdf_arena_add_ref(raw_arena);
  fdf::Arena reply_arena(raw_arena);

  fuchsia_wlan_softmac::WlanRxTransferRequest fidl_request;
  fidl_request.packet_address(reinterpret_cast<uint64_t>(fdf_request->packet.mac_frame.data()));
  fidl_request.packet_size(reinterpret_cast<uint64_t>(fdf_request->packet.mac_frame.size()));
  fidl_request.packet_info(fidl::ToNatural(fdf_request->packet.info));
  fidl_request.async_id(async_id);
  fidl_request.arena(reinterpret_cast<uint64_t>(raw_arena));

  auto fidl_request_persisted = ::fidl::Persist(fidl_request);
  if (fidl_request_persisted.is_ok()) {
    wlan_rx_.transfer(wlan_rx_.ctx, fidl_request_persisted.value().data(),
                      fidl_request_persisted.value().size());
  } else {
    fdf::error("Failed to persist WlanRx.Tranfer fidl_request (FIDL error {})",
               fidl_request_persisted.error_value().status_string());
  }

  completer.buffer(reply_arena).Reply();
}

// Safety: This function type matches the requirement of
// fuchsia.wlan.softmac/EthernetTx.complete_borrowed_operation.
void complete_borrowed_operation(eth::BorrowedOperation<>* op, zx_status_t status) {
  op->Complete(status);
  delete op;
}

zx::result<> SoftmacIfcBridge::EthernetTx(std::unique_ptr<eth::BorrowedOperation<>> op,
                                          trace_async_id_t async_id) const {
  WLAN_TRACE_DURATION();
  fuchsia_wlan_softmac::EthernetTxTransferRequest request;
  request.packet_address(reinterpret_cast<uint64_t>(op->operation()->data_buffer));
  request.packet_size(reinterpret_cast<uint64_t>(op->operation()->data_size));
  request.async_id(async_id);

  // Safety: These fields are properly set according to the requirements of
  // fuchsia.wlan.softmac/EthernetTx.
  auto op_ptr = op.release();
  request.borrowed_operation(reinterpret_cast<uint64_t>(op_ptr));
  request.complete_borrowed_operation(reinterpret_cast<uint64_t>(complete_borrowed_operation));

  auto fidl_request_persisted = ::fidl::Persist(request);
  if (!fidl_request_persisted.is_ok()) {
    fdf::error("Failed to persist EthernetTx.Transfer request (FIDL error {})",
               fidl_request_persisted.error_value().status_string());
  }

  auto result =
      zx::make_result(ethernet_tx_.transfer(ethernet_tx_.ctx, fidl_request_persisted.value().data(),
                                            fidl_request_persisted.value().size()));

  // wlan_ffi_transport::EthernetTx::ethernet_tx_transfer returns ZX_ERR_BAD_STATE if and only if
  // it did not take ownership of the BorrowedOperation before returning.
  if (result.status_value() == ZX_ERR_BAD_STATE) {
    complete_borrowed_operation(op_ptr, result.status_value());
  }
  return result;
}

void SoftmacIfcBridge::ReportTxResult(ReportTxResultRequestView request, fdf::Arena& arena,
                                      ReportTxResultCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  (*softmac_ifc_bridge_client_)
      ->ReportTxResult(fidl::ToNatural(*request))
      .Then(ForwardWireResult<fuchsia_wlan_softmac::WlanSoftmacIfcBridge::ReportTxResult>(
          completer.ToAsync(), fdf::Arena(arena.release())));
}

void SoftmacIfcBridge::NotifyScanComplete(NotifyScanCompleteRequestView request, fdf::Arena& arena,
                                          NotifyScanCompleteCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  (*softmac_ifc_bridge_client_)
      ->NotifyScanComplete(fidl::ToNatural(*request))
      .Then(ForwardWireResult<fuchsia_wlan_softmac::WlanSoftmacIfcBridge::NotifyScanComplete>(
          completer.ToAsync(), fdf::Arena(arena.release())));
}

void SoftmacIfcBridge::StopBridgedDriver(fit::callback<void()> stop_completer) {
  WLAN_TRACE_DURATION();
  (*softmac_ifc_bridge_client_)
      ->StopBridgedDriver()
      .Then([stop_completer = std::move(stop_completer)](
                fidl::Result<fuchsia_wlan_softmac::WlanSoftmacIfcBridge::StopBridgedDriver>&
                    result) mutable {
        if (result.is_error()) {
          fdf::error("FIDL error calling StopBridgeDriver: {}",
                     result.error_value().status_string());
        }
        stop_completer();
      });
}

void SoftmacIfcBridge::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_wlan_softmac::WlanSoftmacIfc> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("SoftmacIfcBridge: Received unknown method with ordinal {}", metadata.method_ordinal);
}

}  // namespace wlan::drivers::wlansoftmac
