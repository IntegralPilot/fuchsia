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

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sdio/sdio_device.h"

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.wlan.fullmac/cpp/driver/wire.h>
#include <fuchsia/hardware/sdio/cpp/banjo.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/compat/cpp/symbols.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/fit/defer.h>
#include <lib/zircon-internal/align.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <string>

#include <wlan/drivers/log_instance.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/bus.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/core.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/inspect/device_inspect.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sdio/sdio.h"

constexpr auto kOpenFlags = fuchsia_io::Flags::kPermReadBytes | fuchsia_io::Flags::kProtocolFile;
namespace wlan {
namespace brcmfmac {

SdioDevice::SdioDevice() : fdf::DriverBase2("brcmfmac") {}

SdioDevice::~SdioDevice() = default;

void SdioDevice::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  wlan::drivers::log::Instance::Init(Debug::kBrcmfMsgFilter);
  component_inspector_.emplace(context.CreateInspector(this));
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  parent_node_.Bind(take_node(), dispatcher());

  zx::result pdev = incoming_->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev.is_error()) {
    BRCMF_ERR("Failed to connect to platform device: %s", pdev.status_string());
    completer(pdev.take_error());
    return;
  }
  pdev_ = fdf::PDev{std::move(pdev.value())};

  zx::result<> result = [&]() -> zx::result<> {
    fidl::Arena arena;
    zx_status_t status = InitDevice(*outgoing(), incoming_);
    if (status != ZX_OK) {
      BRCMF_ERR("Init failed: %s", zx_status_get_string(status));
      return zx::error(status);
    }

    zx::result<std::unique_ptr<DeviceInspect>> result = DeviceInspect::Create(
        fdf_dispatcher_get_async_dispatcher(GetDriverDispatcher()), component_inspector_->root());
    if (result.is_error()) {
      BRCMF_ERR("Device Inspect create failed: %s", zx_status_get_string(status));
      return result.take_error();
    }
    inspect_ = std::move(*result);
    return zx::ok();
  }();

  if (result.is_error()) {
    // If initialization failed we need to shut down any parts that were initialized.
    SdioDevice::Shutdown(
        [result, completer = std::move(completer)]() mutable { completer(result.take_error()); });
    return;
  }

  completer(zx::ok());
}

void SdioDevice::Stop(fdf::StopCompleter completer) {
  SdioDevice::Shutdown([completer = std::move(completer)]() mutable { completer(zx::ok()); });
}

void SdioDevice::Shutdown(fit::callback<void()>&& on_complete) {
  if (brcmf_bus_) {
    brcmf_sdio_exit(brcmf_bus_.get());
    brcmf_bus_.reset();
  }

  Device::Shutdown([on_complete = std::move(on_complete)]() mutable { on_complete(); });
}

zx_status_t SdioDevice::BusInit(const std::shared_ptr<fdf::Namespace>& incoming) {
  zx_status_t status = ZX_OK;

  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> fidl_gpios[GPIO_COUNT] = {};

  auto client_end = incoming->Connect<fuchsia_hardware_gpio::Service::Device>("gpio-oob");
  if (client_end.is_error() || !client_end->is_valid()) {
    BRCMF_ERR("Failed to connect to oob GPIO service: %s", client_end.status_string());
    return client_end.status_value();
  }
  fidl_gpios[WIFI_OOB_IRQ_GPIO_INDEX] =
      fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio>(std::move(client_end.value()));
  if (!fidl_gpios[WIFI_OOB_IRQ_GPIO_INDEX]->Read().ok()) {
    fidl_gpios[WIFI_OOB_IRQ_GPIO_INDEX] = {};
    BRCMF_ERR("OOB IRQ GPIO Read() failed");
    return ZX_ERR_INTERNAL;
  }

  // Attempt to connect to the DEBUG GPIO, ignore if not available.
  client_end = incoming->Connect<fuchsia_hardware_gpio::Service::Device>("gpio-debug");
  if (client_end.is_error() || !client_end->is_valid()) {
    BRCMF_DBG(SDIO, "Failed to connect to debug GPIO service: %s", client_end.status_string());
  } else {
    fidl_gpios[DEBUG_GPIO_INDEX] =
        fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio>(std::move(client_end.value()));
    if (!fidl_gpios[DEBUG_GPIO_INDEX]->Read().ok()) {
      fidl_gpios[DEBUG_GPIO_INDEX] = {};
    }
  }
  ddk::SdioProtocolClient banjo_sdios[SDIO_FN_COUNT];

  zx::result sdio_client =
      compat::ConnectBanjo<ddk::SdioProtocolClient>(incoming, "sdio-function-1");
  if (sdio_client.is_error()) {
    BRCMF_ERR("Failed to connect client status %s", sdio_client.status_string());
    return sdio_client.status_value();
  }
  banjo_sdios[SDIO_FN1_INDEX] = *sdio_client;

  sdio_client = compat::ConnectBanjo<ddk::SdioProtocolClient>(incoming, "sdio-function-2");
  if (sdio_client.is_error()) {
    BRCMF_ERR("Failed to connect client status %s", sdio_client.status_string());
    return sdio_client.status_value();
  }
  banjo_sdios[SDIO_FN2_INDEX] = *sdio_client;

  std::unique_ptr<brcmf_bus> bus;
  if ((status = brcmf_sdio_register(drvr(), fidl_gpios, banjo_sdios, &bus)) != ZX_OK) {
    BRCMF_ERR("brcmf_sdio_register failed: %s", zx_status_get_string(status));
    return status;
  }

  // If the method fails after this point the bus object has to be destroyed by calling
  // brcmf_sdio_exit to ensure proper destruction of all objects created during registration.
  auto on_error = fit::defer([bus = bus.get()] { brcmf_sdio_exit(bus); });

  if ((status = brcmf_sdio_load_files(drvr(), false)) != ZX_OK) {
    BRCMF_ERR("brcmf_sdio_load_files failed: %s", zx_status_get_string(status));
    return status;
  }

  if ((status = brcmf_bus_started(drvr(), false)) != ZX_OK) {
    BRCMF_ERR("brcmf_bus_started failed: %s", zx_status_get_string(status));
    return status;
  }

  // Now that everything has succeeded we should not call brcmf_sdio_exit anymore. The bus object
  // will be destroyed as part of SdioDevice shutting down.
  on_error.cancel();

  brcmf_bus_ = std::move(bus);
  return ZX_OK;
}

zx_status_t SdioDevice::LoadFirmware(const char* path, zx_handle_t* fw, size_t* size) {
  std::string full_filename = "/pkg/lib/firmware/";
  full_filename.append(path);
  auto client = incoming_->Open<fuchsia_io::File>(full_filename.c_str(), kOpenFlags);
  if (client.is_error()) {
    BRCMF_INFO("Open firmware file failed: %s", zx_status_get_string(client.error_value()));
    return client.error_value();
  }

  fidl::WireResult backing_memory_result =
      fidl::WireCall(*client)->GetBackingMemory(fuchsia_io::wire::VmoFlags::kRead);
  if (!backing_memory_result.ok()) {
    if (backing_memory_result.is_peer_closed()) {
      BRCMF_WARN("Failed to get backing memory: Peer closed");
      return ZX_ERR_NOT_FOUND;
    }
    BRCMF_WARN("Failed to get backing memory: %s",
               zx_status_get_string(backing_memory_result.status()));
    return backing_memory_result.status();
  }

  const auto* backing_memory = backing_memory_result.Unwrap();
  if (backing_memory->is_error()) {
    BRCMF_WARN("Failed to get backing memory: %s",
               zx_status_get_string(backing_memory->error_value()));
    return backing_memory->error_value();
  }

  zx::vmo& backing_vmo = backing_memory->value()->vmo;
  if (zx_status_t status = backing_vmo.get_prop_content_size(size); status != ZX_OK) {
    BRCMF_WARN("Failed to get vmo size: %s", zx_status_get_string(status));
    return status;
  }
  *fw = backing_vmo.release();
  return ZX_OK;
}

zx::result<std::vector<uint8_t>> SdioDevice::DeviceGetPersistedMetadata(
    std::string_view metadata_serializable_name) {
  if (metadata_serializable_name == fuchsia_boot_metadata::MacAddressMetadata::kSerializableName) {
    zx::result metadata =
        fdf_metadata::GetMetadata<fuchsia_boot_metadata::MacAddressMetadata>(incoming_, "pdev");
    if (metadata.is_error()) {
      BRCMF_ERR("Failed to get mac address metadata: %s", metadata.status_string());
      return metadata.take_error();
    }
    fit::result persisted_metadata = fidl::Persist(metadata.value());
    if (persisted_metadata.is_error()) {
      BRCMF_ERR("Failed to persist mac address metadata: %s",
                persisted_metadata.error_value().FormatDescription().c_str());
      return zx::error(persisted_metadata.error_value().status());
    }
    return zx::ok(std::move(persisted_metadata.value()));
  }

  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<fuchsia_wlan_broadcom::WifiConfig> SdioDevice::GetWifiConfig() {
  zx::result metadata = pdev_.GetFidlMetadata<fuchsia_wlan_broadcom::WifiConfig>();
  if (metadata.is_error()) {
    BRCMF_ERR("Failed to get metadata: %s", metadata.status_string());
    return metadata.take_error();
  }
  return zx::ok(std::move(metadata.value()));
}

}  // namespace brcmfmac
}  // namespace wlan
FUCHSIA_DRIVER_EXPORT2(::wlan::brcmfmac::SdioDevice);
