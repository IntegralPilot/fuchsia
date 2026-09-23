// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "rpmb.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/scsi/controller.h>

#include "ufs.h"

namespace ufs {

zx_status_t UfsRpmbDevice::AddDevice() {
  {
    const std::string path_from_parent =
        std::string(ufs_parent_->driver_name()) + "/" + kDeviceName + "/";
    auto result =
        compat_server_.Initialize(ufs_parent_->driver_incoming(), ufs_parent_->driver_outgoing(),
                                  ufs_parent_->driver_node_name(), kDeviceName,
                                  compat::ForwardMetadata::None(), std::nullopt, path_from_parent);
    if (result.is_error()) {
      return result.status_value();
    }
  }

  {
    fuchsia_hardware_rpmb::Service::InstanceHandler handler({
        .device = fit::bind_member<&UfsRpmbDevice::Serve>(this),
    });
    auto result = ufs_parent_->driver_outgoing()->AddService<fuchsia_hardware_rpmb::Service>(
        std::move(handler));
    if (result.is_error()) {
      fdf::error("Failed to add RPMB service: {}", result.status_string());
      return result.status_value();
    }
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  controller_.Bind(std::move(controller_client_end));

  fidl::Arena arena;
  std::vector<fuchsia_driver_framework::wire::Offer> offers = compat_server_.CreateOffers2(arena);
  offers.push_back(fdf::MakeOffer2<fuchsia_hardware_rpmb::Service>(arena));

  const auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                        .name(arena, kDeviceName)
                        .offers2(arena, std::move(offers))
                        .Build();

  auto result = ufs_parent_->root_node()->AddChild(args, std::move(controller_server_end), {});
  if (!result.ok()) {
    fdf::error("Failed to add child partition device: {}", result.status_string());
    return result.status();
  }
  return ZX_OK;
}

void UfsRpmbDevice::Serve(fidl::ServerEnd<fuchsia_hardware_rpmb::Rpmb> request) {
  bindings_.AddBinding(ufs_parent_->driver_async_dispatcher(), std::move(request), this,
                       fidl::kIgnoreBindingClosure);
}

// TODO(https://fxbug.dev/527604372): Currently, this returns hardcoded/static values for CID.
// In the future, this should be updated to query the actual UFS device's Card Identification (CID)
// data.
void UfsRpmbDevice::GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) {
  uint8_t rpmb_size = 4;  // Default: 512KB (4 * 128KB)
  uint8_t reliable_write_sector_count = 1;

  auto& device_manager = ufs_parent_->GetDeviceManager();
  auto rpmb_desc_res = device_manager.ReadRpmbUnitDescriptor();
  if (rpmb_desc_res.is_ok()) {
    const auto& rpmb_desc = rpmb_desc_res.value();
    uint64_t block_count = betoh64(rpmb_desc.qLogicalBlockCount);
    // bLogicalBlockSize is the exponent of 2
    uint32_t block_size = 1 << rpmb_desc.bLogicalBlockSize;
    uint64_t rpmb_size_bytes = block_count * block_size;

    // Convert to 128KB units for EmmcDeviceInfo
    uint64_t size_128k = rpmb_size_bytes / (128 * 1024);
    if (size_128k > 0 && size_128k <= UINT8_MAX) {
      rpmb_size = static_cast<uint8_t>(size_128k);
    } else {
      fdf::warn("UfsRpmbDevice: Calculated RPMB size ({}) is invalid, using default 512KB",
                rpmb_size_bytes);
    }

    const auto& geometry = device_manager.GetGeometryDescriptor();
    if (geometry.bRPMB_ReadWriteSize > 0) {
      reliable_write_sector_count = geometry.bRPMB_ReadWriteSize;
    }
    fdf::info("UfsRpmbDevice: bRPMB_ReadWriteSize = {}, reliable_write_sector_count = {}",
              geometry.bRPMB_ReadWriteSize, reliable_write_sector_count);
  } else {
    fdf::warn("UfsRpmbDevice: Failed to read RPMB Unit Descriptor: {}. Using default values.",
              rpmb_desc_res.status_string());
  }

  // TODO(https://fxbug.dev/527604372): Rename this FIDL type to be more agnostic (e.g. not have
  // eMMC).
  fuchsia_hardware_rpmb::wire::EmmcDeviceInfo emmc_info = {
      .cid = {},
      .rpmb_size = rpmb_size,
      .reliable_write_sector_count = reliable_write_sector_count,
  };
  auto device_info = fuchsia_hardware_rpmb::wire::DeviceInfo::WithEmmcInfo(
      fidl::ObjectView<fuchsia_hardware_rpmb::wire::EmmcDeviceInfo>::FromExternal(&emmc_info));
  completer.Reply(device_info);
}

void UfsRpmbDevice::Request(RequestRequestView request, RequestCompleter::Sync& completer) {
  // Serialize all RPMB requests to protect the shared pre-allocated DMA buffer (dma_vmo_).
  // Since RPMB operations are low-throughput and typically used only during boot or
  // specific security events, serializing them is an acceptable trade-off to save memory.
  std::lock_guard<std::mutex> lock(lock_);
  const uint64_t page_size = zx_system_get_page_size();
  const uint8_t rpmb_lun = static_cast<uint8_t>(WellKnownLuns::kRpmb);
  const uint64_t tx_size = request->request.tx_frames.size;

  // 1. Validation
  if (tx_size < 512) {
    fdf::error("UfsRpmbDevice: tx_frames size ({}) must be at least 512 bytes", tx_size);
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  if (tx_size % 512 != 0) {
    fdf::error("UfsRpmbDevice: tx_frames size ({}) is not a multiple of 512", tx_size);
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  if (tx_size > kMaxRpmbTransferSize) {
    fdf::error("UfsRpmbDevice: tx_frames size ({}) exceeds max transfer size ({})", tx_size,
               kMaxRpmbTransferSize);
    completer.Reply(zx::error(ZX_ERR_OUT_OF_RANGE));
    return;
  }

  if (request->request.rx_frames) {
    uint64_t rx_size = request->request.rx_frames->size;
    if (rx_size > 0) {
      if (rx_size % 512 != 0) {
        fdf::error("UfsRpmbDevice: rx_frames size ({}) is not a multiple of 512", rx_size);
        completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
        return;
      }
      if (rx_size > kMaxRpmbTransferSize) {
        fdf::error("UfsRpmbDevice: rx_frames size ({}) exceeds max transfer size ({})", rx_size,
                   kMaxRpmbTransferSize);
        completer.Reply(zx::error(ZX_ERR_OUT_OF_RANGE));
        return;
      }
    }
  }

  // Lazily allocate and map the shared DMA VMO.
  // TODO(https://fxbug.dev/527604372): Consider decommitting this VMO or creating it for each
  // request to reduce persistent memory overhead.
  if (!dma_vmo_.is_valid()) {
    uint64_t vmo_size = fbl::round_up(kMaxRpmbTransferSize, page_size);
    zx_status_t status =
        dma_mapper_.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &dma_vmo_);
    if (status != ZX_OK) {
      fdf::error("UfsRpmbDevice: Failed to create and map shared DMA VMO: {}",
                 zx_status_get_string(status));
      completer.Reply(zx::error(status));
      return;
    }
  }

  // 2. Process tx_frames (Write) if size > 0
  if (tx_size > 0) {
    uint64_t tx_frame_count = tx_size / kRpmbFrameSize;

    for (uint64_t frames_sent = 0; frames_sent < tx_frame_count;) {
      uint64_t remaining_frames = tx_frame_count - frames_sent;
      uint64_t frames_to_send = std::min(remaining_frames, kMaxFramesPerTransfer);
      uint64_t chunk_size = frames_to_send * kRpmbFrameSize;
      uint64_t chunk_offset = frames_sent * kRpmbFrameSize;

      // Read directly from client VMO into mapped DMA memory
      zx_status_t status = request->request.tx_frames.vmo.read(
          dma_mapper_.start(), request->request.tx_frames.offset + chunk_offset, chunk_size);
      if (status != ZX_OK) {
        fdf::error("UfsRpmbDevice: Failed to read tx_frames VMO: {}", zx_status_get_string(status));
        completer.Reply(zx::error(status));
        return;
      }

      // Construct CDB for SECURITY PROTOCOL OUT
      scsi::SecurityProtocolOutCDB cdb = {};
      cdb.opcode = scsi::Opcode::SECURITY_PROTOCOL_OUT;
      cdb.security_protocol = 0xEC;
      cdb.security_protocol_specific = htobe16(0x0001);
      cdb.set_inc_512(false);
      cdb.transfer_length = htobe32(static_cast<uint32_t>(chunk_size));

      ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t*>(&cdb), sizeof(cdb),
                           DataDirection::kHostToDevice, static_cast<uint32_t>(chunk_size));

      auto response = ufs_parent_->GetTransferRequestProcessor().SendAdminScsiCmd(
          upiu, rpmb_lun, zx::unowned_vmo(dma_vmo_));
      if (response.is_error()) {
        fdf::error("UfsRpmbDevice: SECURITY PROTOCOL OUT failed: {}", response.status_string());
        completer.Reply(response.take_error());
        return;
      }

      frames_sent += frames_to_send;
    }
  }

  // 3. Process rx_frames (Read) if rx_frames is present
  if (request->request.rx_frames) {
    uint64_t rx_size = request->request.rx_frames->size;
    if (rx_size > 0) {
      // Construct CDB for SECURITY PROTOCOL IN
      scsi::SecurityProtocolInCDB cdb = {};
      cdb.opcode = scsi::Opcode::SECURITY_PROTOCOL_IN;
      cdb.security_protocol = 0xEC;
      cdb.security_protocol_specific = htobe16(0x0001);
      cdb.set_inc_512(false);
      // allocation_length is in bytes when inc_512 is false
      cdb.allocation_length = htobe32(static_cast<uint32_t>(rx_size));

      ScsiCommandUpiu upiu(reinterpret_cast<const uint8_t*>(&cdb), sizeof(cdb),
                           DataDirection::kDeviceToHost, static_cast<uint32_t>(rx_size));

      auto response = ufs_parent_->GetTransferRequestProcessor().SendAdminScsiCmd(
          upiu, rpmb_lun, zx::unowned_vmo(dma_vmo_));
      if (response.is_error()) {
        fdf::error("UfsRpmbDevice: SECURITY PROTOCOL IN failed: {}", response.status_string());
        completer.Reply(response.take_error());
        return;
      }

      // Write directly from mapped DMA memory to client VMO
      zx_status_t status = request->request.rx_frames->vmo.write(
          dma_mapper_.start(), request->request.rx_frames->offset, rx_size);
      if (status != ZX_OK) {
        fdf::error("UfsRpmbDevice: Failed to write rx_frames VMO: {}",
                   zx_status_get_string(status));
        completer.Reply(zx::error(status));
        return;
      }
    }
  }

  completer.Reply(zx::ok());
}

}  // namespace ufs
