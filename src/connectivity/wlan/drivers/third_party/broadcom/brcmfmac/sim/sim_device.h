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

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SIM_SIM_DEVICE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SIM_SIM_DEVICE_H_

#include <lib/driver/component/cpp/driver_base2.h>

#include <memory>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-env.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/device.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/inspect/device_inspect.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_data_path.h"

struct brcmf_bus;

namespace wlan::brcmfmac {

class SimDevice final : public fdf::DriverBase2, public Device {
 public:
  SimDevice() : DriverBase2("sim-brcmfmac"), data_path_(*this) {}

  SimDevice(const SimDevice& device) = delete;
  SimDevice& operator=(const SimDevice& other) = delete;
  ~SimDevice() override;

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) override {}

  zx_status_t BusInit(const std::shared_ptr<fdf::Namespace>& incoming) override;

  // Set the `simulation::Environment` instance and outgoing directory client (from start_args) that
  // the SimDevice will use. This should be called after `Start()` is called, but before any test
  // logic.
  zx_status_t InitWithEnv(simulation::Environment* env,
                          fidl::UnownedClientEnd<fuchsia_io::Directory> outgoing_dir_client);
  // Call to InitDevice on the Device base class which in turn will kick off all initialization.
  // This exists so that code outside of SimDevice can initialize the device without having access
  // to the protected members in fdf::DriverBase.
  void Initialize(fit::callback<void(zx_status_t)>&& on_complete);

  async_dispatcher_t* GetTimerDispatcher() override { return env_->GetDispatcher(); }
  fdf_dispatcher_t* GetDriverDispatcher() override { return driver_dispatcher()->get(); }
  DeviceInspect* GetInspect() override { return inspect_.get(); }
  fidl::WireClient<fdf::Node>& GetParentNode() override { return parent_node_; }
  std::shared_ptr<fdf::OutgoingDirectory>& Outgoing() override { return outgoing(); }

  // Trampolines for DDK functions, for platforms that support them.
  zx_status_t LoadFirmware(const char* path, zx_handle_t* fw, size_t* size) override;
  zx::result<fuchsia_wlan_broadcom::WifiConfig> GetWifiConfig() override;

  void OnRecoveryComplete() override { recovery_complete_.Signal(); }
  void WaitForRecoveryComplete() {
    recovery_complete_.Wait();
    recovery_complete_.Reset();
  }

  brcmf_simdev* GetSim();

  SimDataPath& DataPath() { return data_path_; }

  inspect::Inspector& GetInspector() { return component_inspector_->inspector(); }

 private:
  void ShutdownImpl();

  simulation::Environment* env_;
  // This is the client end of the outgoing directory that is provided by outgoing(). Any services
  // added to outgoing() will be available for discovery through this client end.
  std::optional<fidl::UnownedClientEnd<fuchsia_io::Directory>> outgoing_dir_client_;
  std::unique_ptr<DeviceInspect> inspect_;
  std::unique_ptr<brcmf_bus> brcmf_bus_;

  SimDataPath data_path_;
  fidl::WireClient<fdf::Node> parent_node_;
  libsync::Completion recovery_complete_;

  std::optional<inspect::ComponentInspector> component_inspector_;
};

}  // namespace wlan::brcmfmac

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SIM_SIM_DEVICE_H_
