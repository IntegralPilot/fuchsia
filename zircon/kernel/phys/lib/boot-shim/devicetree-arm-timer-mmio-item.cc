// Copyright 2025 The Fuchsia Authors
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
namespace {

template <size_t Index>
std::optional<uint64_t> GetAddress(devicetree::RegProperty& reg,
                                   const devicetree::PropertyDecoder& decoder) {
  if (reg.size() <= Index) {
    return std::nullopt;
  }

  std::optional base_addr = reg[Index].address();
  if (!base_addr) {
    return std::nullopt;
  }

  // There should be a non zero address region where the registers are.
  std::optional size = reg[Index].size();
  if (!size || *size == 0) {
    return std::nullopt;
  }

  return decoder.TranslateAddress(*base_addr);
}

}  // namespace

devicetree::ScanState ArmDevicetreeTimerMmioItem::OnTimerNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  ZX_DEBUG_ASSERT(timer_ == nullptr);
  ZX_DEBUG_ASSERT(!payload());

  if (!decoder.is_status_ok()) {
    return devicetree::ScanState::kDoneWithSubtree;
  }

  std::optional compatible =
      decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
  if (!compatible ||
      std::find_first_of(compatible->begin(), compatible->end(), kCompatibleDevices.begin(),
                         kCompatibleDevices.end()) == compatible->end()) {
    return devicetree::ScanState::kActive;
  }

  auto [reg, frequency] = decoder.FindProperties("reg", "clock-frequency");

  if (!reg) {
    OnError("`reg` property missing.\n");
    return devicetree::ScanState::kDone;
  }

  auto reg_prop = reg->AsReg(decoder);
  if (reg_prop->size() < 1) {
    OnError("`reg` property is empty.\n");
    return devicetree::ScanState::kDone;
  }

  std::optional<uint64_t> translated_base_address = GetAddress<0>(*reg_prop, decoder);
  if (!translated_base_address) {
    OnError("Failed to parse base address for CNTCTLBase.\n");
    return devicetree::ScanState::kDone;
  }

  std::optional<uint32_t> clock_frequency;
  if (frequency) {
    clock_frequency = frequency->AsUint32();
    if (!clock_frequency) {
      Log("Failed to parse `clock-frequency`. Value: [");
      for (auto b : frequency->AsBytes()) {
        Log(" %d", b);
      }
      Log("]\n");
    }
  }

  // We are in the target node.
  std::optional<zbi_dcfg_arm_generic_timer_mmio_driver_t>& dcfg_timer_mmio = payload();
  dcfg_timer_mmio.emplace(zbi_dcfg_arm_generic_timer_mmio_driver_t{
      .mmio_phys = *translated_base_address,
      .frequency = clock_frequency.value_or(0),
      .active_frames_mask = 0,
      .frames = {},
  });
  timer_ = &path.back();
  return devicetree::ScanState::kActive;
}

devicetree::ScanState ArmDevicetreeTimerMmioItem::OnFrameNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  ZX_DEBUG_ASSERT(timer_);
  ZX_DEBUG_ASSERT(payload());

  auto [frame_index, irq_bytes, reg_prop] =
      decoder.FindProperties("frame-number", "interrupts", "reg");

  if (!frame_index || !irq_bytes || !reg_prop || decoder.status()->is_fail()) {
    Log("Ignoring invalid timer frame. Missing `frame-number`, `interrupts` or `reg` field.\n");
    return devicetree::ScanState::kActive;
  }

  std::optional parsed_frame_index = frame_index->AsUint32();
  if (!parsed_frame_index) {
    Log("Ignoring invalid timer frame. Failed to parse `frame-index`.\n");
    return devicetree::ScanState::kActive;
  }

  if (*parsed_frame_index >= 8) {
    Log("Ignoring invalid timer frame[%zu]. `frame-index` invalid value, valid range [0-7].\n",
        static_cast<size_t>(*parsed_frame_index));
    return devicetree::ScanState::kActive;
  }

  std::optional<devicetree::RegProperty> reg = reg_prop->AsReg(decoder);
  if (!reg) {
    Log("Ignoring invalid timer frame[%zu].  Failed to parse timer frame's `reg` property.\n",
        static_cast<size_t>(*parsed_frame_index));
    return devicetree::ScanState::kActive;
  }

  std::optional<uint64_t> privileged_base_address = GetAddress<0>(*reg, decoder);
  std::optional<uint64_t> unprivileged_base_address = GetAddress<1>(*reg, decoder);
  if (!privileged_base_address) {
    Log("Ignoring invalid timer frame[%zu]. Failed to parse frame base address.\n",
        static_cast<size_t>(*parsed_frame_index));
    return devicetree::ScanState::kActive;
  }

  auto& irq_resolver = irq_resolvers_[*parsed_frame_index];
  irq_resolver = DevicetreeIrqResolver(irq_bytes->AsBytes());
  uint8_t set_mask = static_cast<uint8_t>(1 << *parsed_frame_index);
  // Only non-disabled nodes are marked as enabled.
  if (decoder.is_status_ok()) {
    active_ |= set_mask;
  }
  present_ |= set_mask;
  unresolved_irqs_++;

  payload()->frames[*parsed_frame_index] = {};
  payload()->frames[*parsed_frame_index].mmio_phys_el1 = *privileged_base_address;
  payload()->frames[*parsed_frame_index].mmio_phys_el0 = unprivileged_base_address.value_or(0);
  // Let's try to resolve the IRQ on this node's hierarchy. By the end we either have found our
  // interrupt controller OR we have a phandle to resolve to the interrupt controller.
  if (auto res = irq_resolver.ResolveIrqController(decoder); res.is_error()) {
    // Bad entry - clear it.
    active_ &= ~set_mask;
    unresolved_irqs_--;
  } else if (res.value()) {
    SetFrameIrq(*parsed_frame_index);
  }

  return devicetree::ScanState::kActive;
}

devicetree::ScanState ArmDevicetreeTimerMmioItem::OnNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  // No frames discovered yet, look for the timer item.
  if (timer_ == nullptr && !payload()) {
    return OnTimerNode(path, decoder);
  }

  // We are inside the timer node children.
  if (timer_) {
    return OnFrameNode(path, decoder);
  }

  // If all frames' IRQs were resolved OR no `valid` frames were found, then we are done.
  if (unresolved_irqs_ == 0) {
    return devicetree::ScanState::kDone;
  }

  // While unlikely and highly improbably, is possible for the each frame node to be routed to
  // different interrupt controller, so we resolve them independently.
  for (size_t i = 0; i < frames().size(); ++i) {
    if (!IsFramePresent(i)) {
      continue;
    }
    auto& irq_resolver = irq_resolvers_[i];
    // We have already found `interrupt-parent`, this is attempting to resolve that phandle.
    if (irq_resolver.NeedsInterruptParent()) {
      if (auto res = irq_resolver.ResolveIrqController(decoder); res.is_ok()) {
        if (res.value()) {
          SetFrameIrq(i);
        }
      } else {
        active_ &= ~static_cast<uint8_t>(1 << i);
        unresolved_irqs_--;
      }
    }
  }

  // All IRQs have been resolved OR they all errored out.
  if (unresolved_irqs_ == 0) {
    return devicetree::ScanState::kDone;
  }

  // We found a handle to the interrupt parent which we need to resolve as we walk through the
  // devicetree.
  return devicetree::ScanState::kActive;
}

}  // namespace boot_shim
