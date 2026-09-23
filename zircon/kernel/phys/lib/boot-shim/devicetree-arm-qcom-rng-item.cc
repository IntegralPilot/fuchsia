// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/boot-shim/devicetree.h"

namespace boot_shim {

devicetree::ScanState ArmDevicetreeQcomRngItem::OnNode(const devicetree::NodePath& path,
                                                       const devicetree::PropertyDecoder& decoder) {
  auto compatibles =
      decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
  if (!compatibles) {
    return devicetree::ScanState::kActive;
  }

  if (std::find(compatibles->begin(), compatibles->end(), kCompatible) == compatibles->end()) {
    return devicetree::ScanState::kActive;
  }

  auto [reg_property, req_config] = decoder.FindProperties("reg", "qcom,no-qrng-config");
  if (!reg_property) {
    return devicetree::ScanState::kDone;
  }

  auto reg = reg_property->AsReg(decoder);
  if (!reg) {
    return devicetree::ScanState::kDone;
  }

  std::optional<uint64_t> base_address = (*reg)[0].address();
  std::optional<uint64_t> size = (*reg)[0].size();
  if (!base_address || !size) {
    return devicetree::ScanState::kDone;
  }

  std::optional<uint64_t> translated_base_address = decoder.TranslateAddress(*base_address);
  if (!translated_base_address) {
    return devicetree::ScanState::kDone;
  }

  // When this property is present, the hw has been configured already, and is ready to be used.
  uint32_t flags = req_config.has_value() ? ZBI_QCOM_RNG_FLAGS_ENABLED : 0;
  set_payload({.mmio_phys = *translated_base_address, .flags = flags});
  (*mmio_observer_)(MmioRange{.address = *translated_base_address, .size = *size});
  return devicetree::ScanState::kDone;
}

}  // namespace boot_shim
