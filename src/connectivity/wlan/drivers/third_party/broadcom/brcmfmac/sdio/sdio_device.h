// Copyright (c) 2019 The Fuchsia Authors
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

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SDIO_SDIO_DEVICE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SDIO_SDIO_DEVICE_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/device.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}

namespace wlan::brcmfmac {

class DeviceInspect;

// This class uses the DriverBase class to manage the lifetime of a brcmfmac driver instance.
class SdioDevice final : public Device, public fdf::DriverBase2 {
 public:
  SdioDevice();
  SdioDevice(const SdioDevice& device) = delete;
  SdioDevice& operator=(const SdioDevice& other) = delete;
  ~SdioDevice();

  static constexpr const char* Name() { return "brcmfmac-sdio"; }
  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;
  void Stop(fdf::StopCompleter completer) override;
  zx_status_t BusInit(const std::shared_ptr<fdf::Namespace>& incoming) override;

  // Virtual state accessor implementation.
  async_dispatcher_t* GetTimerDispatcher() override { return dispatcher(); }
  fdf_dispatcher_t* GetDriverDispatcher() override { return driver_dispatcher()->get(); }
  DeviceInspect* GetInspect() override { return inspect_.get(); }
  fidl::WireClient<fdf::Node>& GetParentNode() override { return parent_node_; }
  std::shared_ptr<fdf::OutgoingDirectory>& Outgoing() override { return outgoing(); }

  zx_status_t LoadFirmware(const char* path, zx_handle_t* fw, size_t* size) override;
  void on_fidl_error(fidl::UnbindInfo error) override {
    BRCMF_WARN("Fidl Error: %s", error.FormatDescription().c_str());
  }

  zx::result<fuchsia_wlan_broadcom::WifiConfig> GetWifiConfig() override;

  zx::result<std::vector<uint8_t>> DeviceGetPersistedMetadata(
      std::string_view metadata_serializable_name) override;

 private:
  void Shutdown(fit::callback<void()>&& on_complete);
  fidl::WireClient<fdf::Node> parent_node_;
  std::unique_ptr<DeviceInspect> inspect_;
  std::unique_ptr<brcmf_bus> brcmf_bus_;
  fdf::PDev pdev_;
  std::optional<inspect::ComponentInspector> component_inspector_;
  std::shared_ptr<fdf::Namespace> incoming_;
};

}  // namespace wlan::brcmfmac

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SDIO_SDIO_DEVICE_H_
