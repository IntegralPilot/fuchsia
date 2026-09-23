// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree.h>
#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>
#include <lib/fit/defer.h>

#include <algorithm>

namespace boot_shim {

// Interrupt list for secure, non-secure, virtual and hypervisor timers, in that order.
// See https://www.kernel.org/doc/Documentation/devicetree/bindings/arm/arch_timer.txt
static constexpr size_t kSecureIrqIndex = 0;
static constexpr size_t kNonSecureIrqIndex = 1;
static constexpr size_t kVirtualIrqIndex = 2;
// Not needed for the item, but is sole purpose is completion.
// static constexpr size_t kHypervisorIrqIndex = 3;

devicetree::ScanState ArmDevicetreeTimerItem::OnNode(const devicetree::NodePath& path,
                                                     const devicetree::PropertyDecoder& decoder) {
  if (path == "/") {
    return devicetree::ScanState::kActive;
  }

  auto set_payload = [this]() {
    this->set_payload(zbi_dcfg_arm_generic_timer_driver_t{
        .irq_phys = irq_.GetIrqConfig(kNonSecureIrqIndex).irq,
        .irq_virt = irq_.GetIrqConfig(kVirtualIrqIndex).irq,
        .irq_sphys = irq_.GetIrqConfig(kSecureIrqIndex).irq,
        .freq_override = static_cast<uint32_t>(frequency_.value_or(0)),
    });
  };

  if (irq_.NeedsInterruptParent()) {
    if (auto result = irq_.ResolveIrqController(decoder); result.is_ok()) {
      if (!*result) {
        return devicetree::ScanState::kActive;
      }
      set_payload();
    }
    return devicetree::ScanState::kDone;
  }

  auto compatibles =
      decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
  // Look for a compatible timer device.
  if (compatibles &&
      std::find_first_of(kCompatibleDevices.begin(), kCompatibleDevices.end(), compatibles->begin(),
                         compatibles->end()) != kCompatibleDevices.end()) {
    found_timer_ = true;
    auto interrupt = decoder.FindProperty("interrupts");
    if (!interrupt) {
      OnError("'timer' node did not contain interrupt information.");
      return devicetree::ScanState::kDone;
    }
    irq_ = DevicetreeIrqResolver(interrupt->AsBytes());
    frequency_ =
        decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsUint32>("clock-frequency");

    if (auto result = irq_.ResolveIrqController(decoder); result.is_ok()) {
      if (!*result) {
        return devicetree::ScanState::kActive;
      }
      set_payload();
    }
    return devicetree::ScanState::kDone;
  }

  return devicetree::ScanState::kActive;
}

}  // namespace boot_shim
