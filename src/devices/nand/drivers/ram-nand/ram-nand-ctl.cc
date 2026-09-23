// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ram-nand-ctl.h"

#include <lib/driver/component/cpp/driver_export.h>
#include <lib/zx/vmo.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/types.h>

#include <memory>

#include <fbl/alloc_checker.h>
#include <fbl/macros.h>

namespace {

void NandBanjoFromFidl(const fuchsia_hardware_nand::wire::Info& source, nand_info_t* destination) {
  destination->page_size = source.page_size;
  destination->pages_per_block = source.pages_per_block;
  destination->num_blocks = source.num_blocks;
  destination->ecc_bits = source.ecc_bits;
  destination->oob_size = source.oob_size;
  destination->nand_class = static_cast<nand_class_t>(source.nand_class);
  memcpy(&destination->partition_guid, source.partition_guid.data(), NAND_GUID_LEN);
}

}  // namespace

namespace ram_nand {

zx::result<> RamNandCtl::Start(fdf::DriverContext context) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  node_name_ = context.node_name();
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    fdf::error("Failed to bind devfs connector: {}", connector);
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs({
      .connector = std::move(connector.value()),
      .connector_supports = fuchsia_device_fs::ConnectionType::kDevice,
  });

  zx::result child = AddOwnedChild(kChildNodeName, devfs);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

void RamNandCtl::CreateDevice(CreateDeviceRequestView request,
                              CreateDeviceCompleter::Sync& completer) {
  nand_info_t temp_info;
  NandBanjoFromFidl(request->info.nand_info, &temp_info);
  const auto& params = static_cast<const NandParams>(temp_info);
  const NandDevice::Id device_id = next_device_id_++;
  auto device = std::make_unique<NandDevice>(params, dispatcher(), device_id,
                                             [this, device_id]() { devices_.erase(device_id); });

  const zx::result device_name =
      device->Init(request->info, child_.node_, incoming(), outgoing(), node_name_);
  if (device_name.is_error()) {
    fdf::error("Failed to initialize device: {}", device_name);
    completer.Reply(device_name.status_value(), fidl::StringView());
    return;
  }
  devices_.insert({device_id, std::move(device)});

  completer.Reply(ZX_OK, fidl::StringView::FromExternal(device_name.value()));
}

void RamNandCtl::DevfsConnect(fidl::ServerEnd<fuchsia_hardware_nand::RamNandCtl> server) {
  bindings_.AddBinding(dispatcher(), std::move(server), this, fidl::kIgnoreBindingClosure);
}

}  // namespace ram_nand

FUCHSIA_DRIVER_EXPORT2(ram_nand::RamNandCtl);
