// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/arm64/apple-aic.h>
#include <lib/boot-shim/apple-aic.h>

namespace boot_shim {

devicetree::ScanState AppleDevicetreeAic3Item::OnNode(const devicetree::NodePath& path,
                                                      const devicetree::PropertyDecoder& decoder) {
  auto compatible =
      decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
  if (!compatible ||
      std::find(compatible->begin(), compatible->end(), "apple,t8122-aic3") == compatible->end()) {
    return devicetree::ScanState::kActive;
  }
  auto property = decoder.FindProperty("reg");
  auto reg = property ? property->AsReg(decoder) : std::nullopt;
  if (!reg || reg->size() != 2 || !(*reg)[0].address() || !(*reg)[0].size() ||
      !(*reg)[1].address() || (*reg)[1].size() != 4) {
    OnError("AIC3 requires core and event register ranges");
    return devicetree::ScanState::kDone;
  }
  auto base = decoder.TranslateAddress(*(*reg)[0].address());
  auto event = decoder.TranslateAddress(*(*reg)[1].address());
  const auto size = *(*reg)[0].size();
  if (!base || !event || size > UINT32_MAX || *event < *base || *event - *base > UINT32_MAX ||
      !arch::apple::Aic3Layout::ValidMmio(*base, static_cast<uint32_t>(size),
                                          static_cast<uint32_t>(*event - *base))) {
    OnError("AIC3 invalid core/event register range");
    return devicetree::ScanState::kDone;
  }
  set_payload({.mmio_phys = *base,
               .mmio_size = static_cast<uint32_t>(size),
               .event_offset = static_cast<uint32_t>(*event - *base)});
  (*mmio_observer_)({.address = *base, .size = static_cast<size_t>(size)});
  return devicetree::ScanState::kDone;
}

}  // namespace boot_shim
