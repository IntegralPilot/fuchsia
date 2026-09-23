// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/boot-shim/devicetree.h"

namespace boot_shim {

devicetree::ScanState RiscvDevicetreePlicItem::OnNode(const devicetree::NodePath& path,
                                                      const devicetree::PropertyDecoder& decoder) {
  auto compatibles = decoder.FindProperty("compatible");

  if (!compatibles) {
    return devicetree::ScanState::kActive;
  }

  auto supported_devices = compatibles->AsStringList();
  if (!supported_devices) {
    return devicetree::ScanState::kActive;
  }

  for (auto compatible_device : kCompatibleDevices) {
    for (auto supported_device : *supported_devices) {
      if (compatible_device == supported_device) {
        return HandlePlicNode(path, decoder);
      }
    }
  }

  return devicetree::ScanState::kActive;
}

devicetree::ScanState RiscvDevicetreePlicItem::HandlePlicNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  auto [reg, num_irqs] = decoder.FindProperties("reg", "riscv,ndev");
  if (!reg) {
    OnError("PLIC Node did not contain a 'reg' property.");
    return devicetree::ScanState::kDone;
  }

  zbi_dcfg_riscv_plic_driver_t dcfg{};
  bool base_address_ok = false;  // Avoids the assumption that a paddr of 0 is invalid
  if (auto reg_val = reg->AsReg(decoder); reg_val) {
    if (auto address = (*reg_val)[0].address()) {
      if (auto root_address = decoder.TranslateAddress(*address)) {
        dcfg.mmio_phys = *root_address;
        base_address_ok = true;
      }
    }
    if (auto size = (*reg_val)[0].size()) {
      dcfg.size_bytes = static_cast<uint32_t>(*size);
    }
  }

  if (!base_address_ok) {
    OnError("Error parsing PLIC node's reg address.");
    return devicetree::ScanState::kDone;
  }

  if (dcfg.size_bytes == 0) {
    OnError("Error parsing PLIC node's reg length.");
    return devicetree::ScanState::kDone;
  }

  if (!num_irqs) {
    OnError("PLIC Node did not contain 'riscv,ndev' property.");
    return devicetree::ScanState::kDone;
  }

  if (auto num_irqs_val = num_irqs->AsUint32()) {
    dcfg.num_irqs = *num_irqs_val;
  } else {
    OnError("Error parsing PLIC node's 'riscv,ndev' property.");
    return devicetree::ScanState::kDone;
  }

  set_payload(dcfg);

  return devicetree::ScanState::kDone;
}

}  // namespace boot_shim
