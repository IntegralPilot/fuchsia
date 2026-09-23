// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree.h>
#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>

#include <algorithm>
#include <type_traits>

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

uint32_t GetUint32(const devicetree::PropertyDecoder& decoder, const char* name,
                   uint32_t default_value = 0) {
  std::optional<devicetree::PropertyValue> prop = decoder.FindProperties(name)[0];
  std::optional<uint32_t> maybe_value = prop ? prop->AsUint32() : std::nullopt;
  return maybe_value.value_or(default_value);
}

}  // namespace

devicetree::ScanState ArmDevicetreeSmmuItem::OnNode(const devicetree::NodePath& path,
                                                    const devicetree::PropertyDecoder& decoder) {
  if (!allocator_) {
    OnError("SMMU parser is missing its allocator!");
    return devicetree::ScanState::kDone;
  }

  // If we are on our second pass, and we have parsed as many entries as we
  // detected in our first pass, then we are done.
  if ((pass_cnt_ == 1) && (smmu_cnt_ == data_.size())) {
    return devicetree::ScanState::kDone;
  }

  // Check to see if this node is an SMMU node.
  std::optional compatible =
      decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");

  if (!compatible ||
      std::find_first_of(compatible->begin(), compatible->end(), kCompatibleDevices.begin(),
                         kCompatibleDevices.end()) == compatible->end()) {
    return devicetree::ScanState::kActive;
  }

  // Look for the base address register definition.  This is an _absolute_
  // minimum for this node to be a valid SMMU.
  auto [reg, phandle] = decoder.FindProperties("reg", "phandle");
  if (!reg) {
    OnError("SMMU `reg` property is missing.\n");
    return devicetree::ScanState::kDoneWithSubtree;
  }

  auto reg_prop = reg->AsReg(decoder);
  if (reg_prop->size() < 1) {
    OnError("SMMU `reg` property is empty.\n");
    return devicetree::ScanState::kDoneWithSubtree;
  }

  std::optional<uint64_t> base_address = GetAddress<0>(*reg_prop, decoder);
  if (!base_address) {
    OnError("Failed to parse base address for SMMU.\n");
    return devicetree::ScanState::kDoneWithSubtree;
  }

  // We found a valid SMMU node.  If this is our first pass, just bump our counter
  // and let the device tree scanner know that we are done with this subtree.
  if (pass_cnt_ == 0) {
    ++smmu_cnt_;
    return devicetree::ScanState::kDoneWithSubtree;
  }

  // Looks like this is the second pass.  This time, parse all of our
  // interesting info into the structures we allocated at the end of the first
  // pass.
  ZX_DEBUG_ASSERT(pass_cnt_ == 1);
  ZX_DEBUG_ASSERT(smmu_cnt_ < data_.size());
  zbi_dcfg_arm_smmu_driver_t& item = data_[smmu_cnt_++];

  // Start by recording our base address.
  item.mmio_phys = *base_address;

  // Look for the optional tags which instruct our driver to limit the number of
  // context banks and SMRs to use in operation.
  item.num_context_banks_override = GetUint32(decoder, "qcom,num-context-banks-override");
  item.num_smr_override = GetUint32(decoder, "qcom,num-smr-override");

  // Now figure out which interrupts we have been assigned.
  if (auto [interrupts] = decoder.FindProperties("interrupts"); interrupts) {
    using IrqElement = devicetree::PropEncodedArrayElement<3>;
    using IrqArray = devicetree::PropEncodedArray<IrqElement>;

    uint32_t global_irq_cnt = GetUint32(decoder, "#global-interrupts", 1);
    IrqArray irqs = IrqArray{interrupts->AsBytes(), 1, 1, 1};
    constexpr size_t max = std::extent_v<decltype(item.irqs)>;
    const size_t irq_cnt = irqs.size();

    static_assert(max <= std::numeric_limits<uint32_t>::max());
    if (irq_cnt > max) {
      Log("WARNING - Too many interrupts (%zu > %zu) for SMMU @0x%08" PRIx64 "\n", irq_cnt, max,
          item.mmio_phys);
      // Fall through to register however many we can.
    }

    item.irq_cnt = static_cast<uint32_t>(std::min(irq_cnt, max));

    if (item.irq_cnt >= global_irq_cnt) {
      Log("WARNING - Too many global interrupts (%" PRIu32 " > %" PRIu32 ") for SMMU @0x%08" PRIx64
          "\n",
          global_irq_cnt, item.irq_cnt, item.mmio_phys);
    }
    item.global_irq_cnt = static_cast<uint32_t>(std::min(global_irq_cnt, item.irq_cnt));

    for (uint32_t i = 0; i < item.irq_cnt; ++i) {
      const std::optional<uint64_t> num = irqs[i][1];
      const std::optional<uint64_t> flags = irqs[i][2];

      // Interrupts given in the SMMU definition are SPI interrupts, indexed
      // from zero.  Translate the value we find here to be absolute IRQ
      // indices.  The SPI range starts at 32, so this really just means adding
      // 32 to the index.
      auto INDX = [](uint64_t index) -> uint64_t { return index + 32; };

      // Note: if we detect something fishy about an entry in the interrupt
      // list, leave the irq number and flags as zero (indicating an invalid
      // interrupt), but do not repack the array. The specific position of the
      // interrupts in the array determine which interrupts are global
      // interrupts vs. context bank interrupts, and which context bank a CB
      // interrupt is matched to.
      if (!num || (INDX(*num) > std::numeric_limits<uint32_t>::max())) {
        Log("WARNING - Skipping irq ndx #%" PRIu32 " with bad IRQ number in SMMU @0x%08" PRIx64
            "\n",
            i, item.mmio_phys);
        continue;
      }

      item.irqs[i].num = static_cast<uint32_t>(INDX(*num));
      item.irqs[i].flags = static_cast<uint32_t>(*flags);
    }
  } else {
    Log("WARNING - Failed to resolve interrupt controller for SMMU @0x%08" PRIx64 "\n",
        item.mmio_phys);
  }

  // Finally, determine if we have any "handoff" SMR values.
  auto [handoff_smrs] = decoder.FindProperties("qcom,handoff-smrs");
  if (handoff_smrs) {
    using SmrElement = devicetree::PropEncodedArrayElement<2>;
    using SmrArray = devicetree::PropEncodedArray<SmrElement>;

    SmrArray smrs = SmrArray{handoff_smrs->AsBytes(), 1, 1};
    constexpr size_t max = std::extent_v<decltype(item.handoff_smrs)>;
    static_assert(max <= std::numeric_limits<uint32_t>::max());

    if (smrs.size() > max) {
      Log("WARNING - Too many handoff SMRs (%zu > %zu) for SMMU @0x%08" PRIx64 "\n", smrs.size(),
          max, item.mmio_phys);
    }

    uint32_t found_smrs{0};
    const uint32_t parse_smr_cnt = static_cast<uint32_t>(std::min(smrs.size(), max));
    for (uint32_t i = 0; i < parse_smr_cnt; ++i) {
      const std::optional<uint32_t> value = smrs[i][0];
      const std::optional<uint32_t> mask = smrs[i][1];

      if (!value || !mask) {
        Log("WARNING - Skipping malformed handoff-SMR #%" PRIu32 " in SMMU @0x%08" PRIx64 "\n", i,
            item.mmio_phys);
        continue;
      }

      if ((*value > 0xFFFF) || (*mask > 0xFFFF)) {
        Log("WARNING - Skipping bad handoff-SMR #%" PRIu32 " (0x%08" PRIx32 "/0x%08" PRIx32
            ") in SMMU @0x%08" PRIx64 "\n",
            i, *value, *mask, item.mmio_phys);
        continue;
      }

      item.handoff_smrs[found_smrs++] = (*value & 0xFFFF) | (*mask << 16);
    }
    item.handoff_smr_cnt = found_smrs;
  }

  // We are done with this SMMU now, we don't need to continue to parse anything
  // underneath here.
  return devicetree::ScanState::kDoneWithSubtree;
}

devicetree::ScanState ArmDevicetreeSmmuItem::OnScan() {
  // If we just finished the first pass, then we are done if we didn't find any
  // SMMUs during the scanning pass, or if we cannot manage to allocate the
  // memory we will need for them.
  if (++pass_cnt_ == 1) {
    if (!smmu_cnt_) {
      return devicetree::ScanState::kDone;
    }

    fbl::AllocChecker ac;
    data_ = Allocate<zbi_dcfg_arm_smmu_driver_t>(smmu_cnt_, ac);
    // Success or failure, it is time to reset our counter.  On failure, we will be done and should
    // report that we have 0 successfully parsed SMMUs.  On success, we are about to begin our
    // second pass and will use this field to keep track of which SMMU instance we are parsing at
    // any given point in time.
    smmu_cnt_ = 0;
    if (!ac.check()) {
      return devicetree::ScanState::kDone;
    }
  } else {
    return devicetree::ScanState::kDone;
  }

  return devicetree::ScanState::kActive;
}

fit::result<ArmDevicetreeSmmuItem::DataZbi::Error> ArmDevicetreeSmmuItem::AppendItems(
    DataZbi& zbi) const {
  if (data_.size()) {
    auto result = zbi.Append(
        {
            .type = ZBI_TYPE_KERNEL_DRIVER,
            .extra = ZBI_KERNEL_DRIVER_ARM_SMMU,
        },
        std::as_bytes(data_));

    if (!result.is_ok()) {
      return result.take_error();
    }
  }
  return fit::ok();
}

}  // namespace boot_shim
