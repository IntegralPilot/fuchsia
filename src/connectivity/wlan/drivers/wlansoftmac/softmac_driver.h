// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_SOFTMAC_DRIVER_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_SOFTMAC_DRIVER_H_

#include <fidl/fuchsia.wlan.softmac/cpp/driver/fidl.h>
#include <fuchsia/hardware/ethernet/cpp/banjo.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/compat/cpp/banjo_server.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/fdf/cpp/channel_read.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/operation/ethernet.h>
#include <lib/trace/event.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <memory>
#include <mutex>

#include <ddktl/device.h>
#include <fbl/ref_ptr.h>
#include <wlan/drivers/log.h>

#include "fuchsia/hardware/ethernet/c/banjo.h"
#include "softmac_bridge.h"

namespace wlan::drivers::wlansoftmac {

class SoftmacDriver : public fdf::DriverBase2, public ddk::EthernetImplProtocol<SoftmacDriver> {
 public:
  SoftmacDriver();
  ~SoftmacDriver() override = default;

  static constexpr inline SoftmacDriver* from(void* ctx) {
    return static_cast<SoftmacDriver*>(ctx);
  }

  zx_status_t EthernetImplQuery(uint32_t options, ethernet_info_t* out_info);
  zx_status_t EthernetImplStart(const ethernet_ifc_protocol_t* ifc)
      __TA_EXCLUDES(ethernet_proxy_lock_);
  void EthernetImplStop() __TA_EXCLUDES(ethernet_proxy_lock_);
  void EthernetImplQueueTx(uint32_t options, ethernet_netbuf_t* netbuf,
                           ethernet_impl_queue_tx_callback callback, void* cookie);
  zx_status_t EthernetImplSetParam(uint32_t param, int32_t value, const uint8_t* data_buffer,
                                   size_t data_size);
  void EthernetImplGetBti(zx::bti* out_bti2);

 private:
  compat::BanjoServer banjo_server_;
  compat::AsyncInitializedDeviceServer compat_server_;

  fidl::SharedClient<fuchsia_driver_framework::Node> node_client_;

  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;
  void Stop(fdf::StopCompleter completer) override;

  // Mark `ethernet_proxy_lock_` as a mutable member of this class to allow const functions
  // to acquire it.
  mutable std::shared_ptr<std::mutex> ethernet_proxy_lock_;
  ddk::EthernetIfcProtocolClient ethernet_proxy_ __TA_GUARDED(ethernet_proxy_lock_);

  // If MLME calls `WlanSoftmacBridge.SetEthernetStatus` before the ethernet device calls
  // `EthernetImpl.Start`, the status from MLME is stored in `cached_ethernet_status_`.
  // When the ethernet device calls `EthernetImpl.Start`, the cached status, if any, will be
  // forwarded to `EthernetImplIfc.Start`.
  //
  // When MLME calls `WlanSoftmacBridge.SetEthernetStatus` multiple times before the
  // ethernet device calls `EthernetImpl.Start`, the `cached_ethernet_status_` is always
  // overwritten with the most recent status from MLME.
  //
  // This field is shared with the `SoftmacBridge` instance (in `softmac_bridge_`) which
  // writes to `cached_ethernet_status_` when `SoftmacBridge::SetEthernetStatus` is called.
  mutable std::optional<uint32_t> cached_ethernet_status_ __TA_GUARDED(ethernet_proxy_lock_);

  std::unique_ptr<SoftmacBridge> softmac_bridge_;

  // The FIDL client to communicate with the parent driver's WlanSoftmac server.
  fdf::SharedClient<fuchsia_wlan_softmac::WlanSoftmac> softmac_client_;

  ethernet_impl_protocol_ops_t ethernet_impl_protocol_ops_;
};

}  // namespace wlan::drivers::wlansoftmac

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_SOFTMAC_DRIVER_H_
