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

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_device.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/status.h>

#include <wlan/drivers/log_instance.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/bus.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/debug.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/device.h"

namespace wlan::brcmfmac {

SimDevice::~SimDevice() { ShutdownImpl(); }

void SimDevice::ShutdownImpl() {
  // Keep a separate implementation for this that's not virtual so that it can be called from the
  // destructor.
  if (brcmf_bus_) {
    brcmf_sim_exit(brcmf_bus_.get());
    drvr()->bus_if = nullptr;
    brcmf_bus_.reset();
  }
}

zx::result<> SimDevice::Start(fdf::DriverContext context) {
  component_inspector_.emplace(context.CreateInspector(this));

  parent_node_.Bind(take_node(), dispatcher());

  zx::result<std::unique_ptr<DeviceInspect>> result =
      DeviceInspect::Create(dispatcher(), component_inspector_->root());
  if (result.is_error()) {
    fdf::error("ERROR calling DeviceInspect::Create(): {}", result);
    return result.take_error();
  }

  inspect_ = std::move(*result);
  wlan::drivers::log::Instance::Init(Debug::kBrcmfMsgFilter);
  return zx::ok();
}

void SimDevice::Stop(fdf::StopCompleter completer) {
  ShutdownImpl();
  Shutdown([completer = std::move(completer)]() mutable { completer(zx::ok()); });
}

zx_status_t SimDevice::InitWithEnv(
    simulation::Environment* env,
    fidl::UnownedClientEnd<fuchsia_io::Directory> outgoing_dir_client) {
  env_ = env;
  brcmf_bus_ = brcmf_sim_alloc(drvr(), env);

  outgoing_dir_client_ = outgoing_dir_client;
  return ZX_OK;
}

void SimDevice::Initialize(fit::callback<void(zx_status_t)>&& on_complete) {
  if (!outgoing_dir_client_.has_value()) {
    fdf::error("Missing outgoing directory client, call Initialize first");
    on_complete(ZX_ERR_BAD_STATE);
    return;
  }

  zx_status_t status = InitDevice(*outgoing(), std::shared_ptr<fdf::Namespace>{});
  if (status != ZX_OK) {
    fdf::error("Failed to initialize device: {}", zx_status_get_string(status));
    on_complete(status);
    return;
  }

  data_path_.Init(
      *outgoing_dir_client_, [on_complete = std::move(on_complete)](zx_status_t status) mutable {
        if (status != ZX_OK) {
          fdf::error("Failed to initialize data path: {}", zx_status_get_string(status));
        }
        on_complete(status);
      });
}

zx_status_t SimDevice::BusInit(const std::shared_ptr<fdf::Namespace>& incoming) {
  return brcmf_sim_register(drvr());
}

zx_status_t SimDevice::LoadFirmware(const char* path, zx_handle_t* fw, size_t* size) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx::result<fuchsia_wlan_broadcom::WifiConfig> SimDevice::GetWifiConfig() {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

brcmf_simdev* SimDevice::GetSim() { return brcmf_bus_->bus_priv.sim; }

}  // namespace wlan::brcmfmac
FUCHSIA_DRIVER_EXPORT2(::wlan::brcmfmac::SimDevice);
