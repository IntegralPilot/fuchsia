// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "device.h"

#include <fuchsia/hardware/usb/cpp/banjo.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/zx/vmo.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <fbl/string_printf.h>

#include "firmware_loader.h"
#include "logging.h"
#include "packets.emb.h"

namespace bt_hci_intel {
namespace fhbt = fuchsia_hardware_bluetooth;

// USB Product IDs that use the "secure" firmware method.
static constexpr uint16_t sfi_product_ids[] = {
    0x0025,  // Thunder Peak (9160/9260)
    0x0a2b,  // Snowfield Peak (8260)
    0x0aaa,  // Jefferson Peak (9460/9560)
    0x0026,  // Harrison Peak (AX201)
    0x0032,  // Sun Peak (AX210)
};

// USB Product IDs that use the "legacy" firmware loading method.
static constexpr uint16_t legacy_firmware_loading_ids[] = {
    0x0025,  // Thunder Peak (9160/9260)
    0x0a2b,  // Snowfield Peak (8260)
    0x0aaa,  // Jefferson Peak (9460/9560)
    0x0026,  // Harrison Peak (AX201)
};

zx::result<> Device::Start(fdf::DriverContext context) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  zx::result<ddk::UsbProtocolClient> usb_client =
      compat::ConnectBanjo<ddk::UsbProtocolClient>(incoming());

  if (usb_client.is_error()) {
    errorf("Failed to connect usb client: {}", usb_client);
    return zx::error(usb_client.status_value());
  }
  ddk::UsbProtocolClient usb = *usb_client;

  usb_device_descriptor_t dev_desc;
  usb.GetDeviceDescriptor(&dev_desc);

  // Whether this device uses the "secure" firmware method.
  for (uint16_t id : sfi_product_ids) {
    if (dev_desc.id_product == id) {
      secure_ = true;
      break;
    }
  }

  // Whether this device uses the "legacy" firmware loading method.
  for (uint16_t id : legacy_firmware_loading_ids) {
    if (dev_desc.id_product == id) {
      legacy_firmware_loading_ = true;
      break;
    }
  }

  auto dispatcher =
      fdf::SynchronizedDispatcher::Create({}, "", [](fdf_dispatcher_t* dispatcher) {});
  if (dispatcher.is_error()) {
    errorf("Failed to create dispatcher: {}", dispatcher);
    return zx::error(dispatcher.status_value());
  }
  hci_client_dispatcher_ = std::move(*dispatcher);

  zx::result<fidl::ClientEnd<fhbt::HciTransport>> client_end =
      incoming()->Connect<fhbt::HciService::HciTransport>();
  if (client_end.is_error()) {
    errorf("Connect to fhbt::HciTransport protocol failed: {}", client_end);
    return zx::error(client_end.status_value());
  }

  hci_transport_client_ = fidl::SharedClient(
      *std::move(client_end), hci_client_dispatcher_.async_dispatcher(), &hci_event_handler_);

  if (zx_status_t status = Init(secure_); status != ZX_OK) {
    errorf("Initialization failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  hci_transport_client_.AsyncTeardown();

  return zx::ok();
}

void Device::Stop(fdf::StopCompleter completer) { completer(zx::ok()); }

zx_status_t Device::Init(bool secure) {
  infof("Init(secure: {}, firmware_loading: {})", (secure_ ? "yes" : "no"),
        (legacy_firmware_loading_ ? "legacy" : "new"));

  // TODO(armansito): Track metrics for initialization failures.

  zx_status_t status;
  if (secure_) {
    status = LoadSecureFirmware();
  } else {
    status = LoadLegacyFirmware();
  }

  if (status != ZX_OK) {
    return InitFailed(status, "failed to initialize controller");
  }

  firmware_loaded_ = true;

  fuchsia_hardware_bluetooth::Service::InstanceHandler handler({
      .vendor =
          vendor_binding_group_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });
  zx::result result =
      outgoing()->AddService<fuchsia_hardware_bluetooth::Service>(std::move(handler));
  if (result.is_error()) {
    errorf("Failed to add service: {}", result.status_string());
    return result.status_value();
  }

  return ZX_OK;
}

zx_status_t Device::InitFailed(zx_status_t status, const char* note) {
  errorf("{}: {}", note, zx_status_get_string(status));
  return status;
}

constexpr auto kOpenFlags = fuchsia_io::Flags::kPermReadBytes | fuchsia_io::Flags::kProtocolFile;

zx_handle_t Device::MapFirmware(const char* name, uintptr_t* fw_addr, size_t* fw_size) {
  zx_handle_t vmo = ZX_HANDLE_INVALID;
  size_t size;
  std::string fw_path = "/pkg/lib/firmware/";
  fw_path.append(name);
  auto client = incoming()->Open<fuchsia_io::File>(fw_path.c_str(), kOpenFlags);
  if (client.is_error()) {
    warnf("Open firmware file failed: {}", zx_status_get_string(client.error_value()));
    return ZX_HANDLE_INVALID;
  }

  fidl::WireResult backing_memory_result =
      fidl::WireCall(*client)->GetBackingMemory(fuchsia_io::wire::VmoFlags::kRead);
  if (!backing_memory_result.ok()) {
    if (backing_memory_result.is_peer_closed()) {
      warnf("Failed to get backing memory: Peer closed");
      return ZX_HANDLE_INVALID;
    }
    warnf("Failed to get backing memory: {}", zx_status_get_string(backing_memory_result.status()));
    return ZX_HANDLE_INVALID;
  }

  const auto* backing_memory = backing_memory_result.Unwrap();
  if (backing_memory->is_error()) {
    warnf("Failed to get backing memory: {}", zx_status_get_string(backing_memory->error_value()));
    return ZX_HANDLE_INVALID;
  }

  zx::vmo& backing_vmo = backing_memory->value()->vmo;
  if (zx_status_t status = backing_vmo.get_prop_content_size(&size); status != ZX_OK) {
    warnf("Failed to get vmo size: {}", zx_status_get_string(status));
    return ZX_HANDLE_INVALID;
  }
  vmo = backing_vmo.release();

  zx_status_t status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ, 0, vmo, 0, size, fw_addr);
  if (status != ZX_OK) {
    errorf("firmware map failed: {}", zx_status_get_string(status));
    return ZX_HANDLE_INVALID;
  }
  *fw_size = size;
  return vmo;
}

void Device::GetFeatures(GetFeaturesCompleter::Sync& completer) {
  completer.Reply(fuchsia_hardware_bluetooth::wire::VendorFeatures());
}

void Device::EncodeCommand(EncodeCommandRequestView request,
                           EncodeCommandCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}
void Device::OpenHci(OpenHciCompleter::Sync& completer) {
  auto endpoints = fidl::CreateEndpoints<fhbt::Hci>();
  if (endpoints.is_error()) {
    errorf("Failed to create endpoints: {}", zx_status_get_string(endpoints.error_value()));
    completer.ReplyError(endpoints.error_value());
    return;
  }

  completer.ReplySuccess(std::move(endpoints->client));
}

void Device::OpenHciTransport(OpenHciTransportCompleter::Sync& completer) {
  zx::result<fidl::ClientEnd<fhbt::HciTransport>> client_end =
      incoming()->Connect<fhbt::HciService::HciTransport>();
  if (client_end.is_error()) {
    errorf("Connect to fhbt::HciTransport protocol failed: {}", client_end);
    completer.ReplyError(client_end.status_value());
    return;
  }

  completer.ReplySuccess(std::move(*client_end));
}

void Device::OpenSnoop(OpenSnoopCompleter::Sync& completer) {
  zx::result<fidl::ClientEnd<fhbt::Snoop>> client_end =
      incoming()->Connect<fhbt::HciService::Snoop>();
  if (client_end.is_error()) {
    errorf("Connect to fhbt::Snoop protocol failed: {}", client_end);
    completer.ReplyError(client_end.status_value());
    return;
  }

  completer.ReplySuccess(std::move(*client_end));
}

void Device::GetCrashParameters(GetCrashParametersCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void Device::handle_unknown_method(fidl::UnknownMethodMetadata<fhbt::Vendor> metadata,
                                   fidl::UnknownMethodCompleter::Sync& completer) {
  errorf("Unknown method in Vendor request, closing with ZX_ERR_NOT_SUPPORTED");
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t Device::LoadSecureFirmware() {
  ZX_DEBUG_ASSERT(hci_transport_client_.is_valid());

  VendorHci hci(hci_transport_client_, hci_event_handler_);

  // Bring the controller to a well-defined default state.
  // Send an initial reset. If the controller sends a "command not supported"
  // event on newer models, this likely means that the controller is in
  // bootloader mode and we can ignore the error.
  auto hci_status = hci.SendHciReset();
  if (hci_status == pw::bluetooth::emboss::StatusCode::UNKNOWN_COMMAND) {
    infof("Ignoring \"Unknown Command\" error while in bootloader mode");
  } else if (hci_status != pw::bluetooth::emboss::StatusCode::SUCCESS) {
    errorf("HCI_Reset failed (status: 0x{:02x})", static_cast<unsigned char>(hci_status));
    return ZX_ERR_BAD_STATE;
  }

  bt_hci_intel::SecureBootEngineType engine_type = bt_hci_intel::SecureBootEngineType::kRSA;

  fbl::String fw_filename;
  if (legacy_firmware_loading_) {
    std::vector<uint8_t> version = hci.SendReadVersion();
    auto version_view = MakeReadVersionCommandCompleteEventView(&version);
    if (!version_view.Ok() ||
        version_view.status().Read() != pw::bluetooth::emboss::StatusCode::SUCCESS) {
      errorf("failed to obtain version information");
      return ZX_ERR_BAD_STATE;
    }

    // If we're already in firmware mode, we're done.
    if (version_view.fw_variant().Read() == kFirmwareFirmwareVariant) {
      infof("firmware loaded (variant {}, revision {})", version_view.hw_variant().Read(),
            version_view.hw_revision().Read());
      return ZX_OK;
    }

    // If we reached here then the controller must be in bootloader mode.
    if (version_view.fw_variant().Read() != kBootloaderFirmwareVariant) {
      errorf("unsupported firmware variant ({:#x})", version_view.fw_variant().Read());
      return ZX_ERR_NOT_SUPPORTED;
    }

    std::vector<uint8_t> boot_params = hci.SendReadBootParams();
    auto boot_view = MakeReadBootParamsCommandCompleteEventView(&boot_params);
    if (!boot_view.Ok() ||
        boot_view.status().Read() != pw::bluetooth::emboss::StatusCode::SUCCESS) {
      errorf("failed to read boot parameters");
      return ZX_ERR_BAD_STATE;
    }

    // Map the firmware file into memory.
    fw_filename = fbl::StringPrintf("ibt-%d-%d.sfi", version_view.hw_variant().Read(),
                                    boot_view.dev_revid().Read());
  } else {
    std::optional<ReadVersionReturnParamsTlv> version = hci.SendReadVersionTlv();
    if (!version) {
      errorf("failed to obtain version information");
      return ZX_ERR_BAD_STATE;
    }

    engine_type = (version->secure_boot_engine_type == 0x01)
                      ? bt_hci_intel::SecureBootEngineType::kECDSA
                      : bt_hci_intel::SecureBootEngineType::kRSA;

    // If we're already in firmware mode, we're done.
    if (version->current_mode_of_operation == kCurrentModeOfOperationOperationalFirmware) {
      infof("firmware already loaded");
      return ZX_OK;
    }

    fw_filename = fbl::StringPrintf("ibt-%04x-%04x.sfi", 0x0041, 0x0041);
  }

  zx::vmo fw_vmo;
  uintptr_t fw_addr;
  size_t fw_size;
  fw_vmo.reset(MapFirmware(fw_filename.c_str(), &fw_addr, &fw_size));
  if (!fw_vmo) {
    errorf("failed to map firmware");
    return ZX_ERR_NOT_SUPPORTED;
  }

  FirmwareLoader loader(hci_transport_client_, hci_event_handler_);

  // The boot addr differs for different firmware.  Save it for later.
  uint32_t boot_addr = 0x00000000;
  auto status = loader.LoadSfi(reinterpret_cast<void*>(fw_addr), fw_size, engine_type, &boot_addr);
  zx_vmar_unmap(zx_vmar_root_self(), fw_addr, fw_size);
  if (status == FirmwareLoader::LoadStatus::kError) {
    errorf("failed to load SFI firmware");
    return ZX_ERR_BAD_STATE;
  }

  // Switch the controller into firmware mode.
  hci.SendVendorReset(boot_addr);

  infof("firmware loaded using {}", fw_filename.c_str());
  return ZX_OK;
}

zx_status_t Device::LoadLegacyFirmware() {
  ZX_DEBUG_ASSERT(hci_transport_client_.is_valid());

  VendorHci hci(hci_transport_client_, hci_event_handler_);

  // Bring the controller to a well-defined default state.
  auto hci_status = hci.SendHciReset();
  if (hci_status != pw::bluetooth::emboss::StatusCode::SUCCESS) {
    errorf("HCI_Reset failed (status: 0x{:02x})", static_cast<unsigned char>(hci_status));
    return ZX_ERR_BAD_STATE;
  }

  std::vector<uint8_t> version = hci.SendReadVersion();
  auto version_view = MakeReadVersionCommandCompleteEventView(&version);
  if (!version_view.Ok() ||
      version_view.status().Read() != pw::bluetooth::emboss::StatusCode::SUCCESS) {
    errorf("failed to obtain version information");
    return ZX_ERR_BAD_STATE;
  }

  if (version_view.fw_patch_num().Read() > 0) {
    infof("controller already patched");
    return ZX_OK;
  }

  auto fw_filename =
      fbl::StringPrintf("ibt-hw-%x.%x.%x-fw-%x.%x.%x.%x.%x.bseq", version_view.hw_platform().Read(),
                        version_view.hw_variant().Read(), version_view.hw_revision().Read(),
                        version_view.fw_variant().Read(), version_view.fw_revision().Read(),
                        version_view.fw_build_num().Read(), version_view.fw_build_week().Read(),
                        version_view.fw_build_year().Read());

  zx::vmo fw_vmo;
  uintptr_t fw_addr;
  size_t fw_size;
  fw_vmo.reset(MapFirmware(fw_filename.c_str(), &fw_addr, &fw_size));

  // Try the fallback patch file on initial failure.
  if (!fw_vmo) {
    // Try the fallback patch file
    fw_filename = fbl::StringPrintf("ibt-hw-%x.%x.bseq", version_view.hw_platform().Read(),
                                    version_view.hw_variant().Read());
    fw_vmo.reset(MapFirmware(fw_filename.c_str(), &fw_addr, &fw_size));
  }

  // Abort if the fallback file failed to load too.
  if (!fw_vmo) {
    errorf("failed to map firmware");
    return ZX_ERR_NOT_SUPPORTED;
  }

  FirmwareLoader loader(hci_transport_client_, hci_event_handler_);
  hci.EnterManufacturerMode();
  auto result = loader.LoadBseq(reinterpret_cast<void*>(fw_addr), fw_size);
  hci.ExitManufacturerMode(result == FirmwareLoader::LoadStatus::kPatched
                               ? MfgDisableMode::PATCHES_ENABLED
                               : MfgDisableMode::NO_PATCHES);
  zx_vmar_unmap(zx_vmar_root_self(), fw_addr, fw_size);

  if (result == FirmwareLoader::LoadStatus::kError) {
    errorf("failed to patch controller");
    return ZX_ERR_BAD_STATE;
  }

  infof("controller patched using {}", fw_filename.c_str());
  return ZX_OK;
}

}  // namespace bt_hci_intel

FUCHSIA_DRIVER_EXPORT2(bt_hci_intel::Device);
