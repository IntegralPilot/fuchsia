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

devicetree::ScanState ArmDevicetreeTimerItem::OnNode(const devicetree::NodePath& path,
                                                     const devicetree::PropertyDecoder& decoder) {
  if (path == "/") {
    return devicetree::ScanState::kActive;
  }

  auto set_payload = [this]() {
    auto irq = [this](std::optional<size_t> index) {
      return index && *index < irq_.num_entries() ? irq_.GetIrqConfig(*index).irq : 0;
    };
    this->set_payload(zbi_dcfg_arm_generic_timer_driver_t{
        .irq_phys = irq(physical_irq_),
        .irq_virt = irq(virtual_irq_),
        .irq_sphys = irq(secure_irq_),
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
    if (auto names_property = decoder.FindProperty("interrupt-names")) {
      auto names = names_property->AsStringList();
      if (!names) {
        OnError("Invalid timer interrupt-names");
        return devicetree::ScanState::kDone;
      }
      secure_irq_.reset();
      physical_irq_.reset();
      virtual_irq_.reset();
      size_t index = 0;
      for (auto name : *names) {
        std::optional<size_t>* entry = nullptr;
        if (name == "sec-phys") {
          entry = &secure_irq_;
        } else if (name == "phys") {
          entry = &physical_irq_;
        } else if (name == "virt") {
          entry = &virtual_irq_;
        }
        if (entry) {
          if (entry->has_value()) {
            OnError("Duplicate timer interrupt name");
            return devicetree::ScanState::kDone;
          }
          *entry = index;
        }
        ++index;
      }
    }
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
