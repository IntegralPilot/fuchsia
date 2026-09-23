// Copyright (c) 2021 The Fuchsia Authors
//
// Permission to use, copy, modify, and/or distribute this software for any purpose with or without
// fee is hereby granted, provided that the above copyright notice and this permission notice
// appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
// SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
// AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
// NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE
// OF THIS SOFTWARE.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_INSPECT_DEVICE_INSPECT_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_INSPECT_DEVICE_INSPECT_H_

#include <lib/async/dispatcher.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <memory>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/inspect/windowed_uint_property.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/timer.h"

namespace wlan::brcmfmac {

// TODO(https://fxbug.dev/42149564) - Avoid duplicates for windowed and non-windowed property.
struct DeviceConnMetrics {
  inspect::Node root;
  inspect::UintProperty success;
  WindowedUintProperty success_24hrs;
  inspect::UintProperty no_network_fail;
  WindowedUintProperty no_network_fail_24hrs;
  inspect::UintProperty auth_fail;
  WindowedUintProperty auth_fail_24hrs;
  inspect::UintProperty other_fail;
  WindowedUintProperty other_fail_24hrs;
};

class DeviceInspect {
 public:
  // Factory creation function.
  static zx::result<std::unique_ptr<DeviceInspect>> Create(async_dispatcher_t* dispatcher,
                                                           inspect::Node& root_node);
  ~DeviceInspect() = default;

  // Metrics APIs.
  void LogTxQueueFull();
  void LogFwRecoveryTriggered();
  void LogFwRecovered();
  void LogConnSuccess();
  void LogConnNoNetworkFail();
  void LogConnAuthFail();
  void LogConnOtherFail();
  void LogRxFreeze();
  void LogSdioMaxTxSeqErr();
  void LogApSetSsidErr();
  void LogLowDataRate();
  void LogHighWmeRxErrorRate();

 private:
  // Only constructible through Create().
  DeviceInspect() = default;

  inspect::Node root_;
  std::unique_ptr<Timer> timer_hr_;

  // Metrics
  DeviceConnMetrics conn_metrics_;
  inspect::UintProperty tx_qfull_;
  WindowedUintProperty tx_qfull_24hrs_;

  inspect::UintProperty fw_recovery_triggered_;
  WindowedUintProperty fw_recovery_triggered_24hrs_;
  // Lapis copies |fw_recovered_| and |fw_recovered_24hrs_| inspect nodes to the Cobalt metrics.
  // Cobalt metric definition:
  // https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/cobalt_config/fuchsia/connectivity_wlan/metrics.yaml;l=475
  inspect::UintProperty fw_recovered_;
  WindowedUintProperty fw_recovered_24hrs_;
  inspect::UintProperty rx_freeze_;
  WindowedUintProperty rx_freeze_24hrs_;
  inspect::UintProperty sdio_max_tx_seq_err_;
  WindowedUintProperty sdio_max_tx_seq_err_24hrs_;
  inspect::UintProperty ap_set_ssid_err_;
  WindowedUintProperty ap_set_ssid_err_24hrs_;
  inspect::UintProperty low_data_rate_;
  WindowedUintProperty low_data_rate_24hrs_;
  inspect::UintProperty high_wme_rx_error_rate_;
  WindowedUintProperty high_wme_rx_error_rate_24hrs_;
};

}  // namespace wlan::brcmfmac

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_INSPECT_DEVICE_INSPECT_H_
