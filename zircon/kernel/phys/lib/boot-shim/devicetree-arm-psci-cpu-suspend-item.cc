// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree.h>

namespace boot_shim {
namespace {

// See https://www.kernel.org/doc/Documentation/devicetree/bindings/arm/idle-states.txt
constexpr std::string_view kIdleStateCompatible = "arm,idle-state";

// See https://www.kernel.org/doc/Documentation/devicetree/bindings/power/domain-idle-state.yaml
constexpr std::string_view kDomainIdleStateCompatible = "domain-idle-state";

constexpr auto kIdleStateCompatibles =
    std::to_array({kIdleStateCompatible, kDomainIdleStateCompatible});

constexpr auto kDomainIdleStateIt = std::next(kIdleStateCompatibles.begin());

constexpr bool IsDomainIdleState(decltype(kIdleStateCompatibles)::const_iterator it) {
  return kDomainIdleStateIt == it;
}

}  // namespace

devicetree::ScanState ArmDevicetreePsciCpuSuspendItem::OnIdleStateCount(
    const devicetree::PropertyDecoder& decoder) {
  auto compatibles =
      decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
  if (!compatibles) {
    OnError("Failed to parse 'compatible' property");
    return devicetree::ScanState::kActive;
  }

  if (std::ranges::find_first_of(kIdleStateCompatibles, *compatibles) ==
      kIdleStateCompatibles.end()) {
    return devicetree::ScanState::kActive;
  }
  num_idle_states_++;
  return devicetree::ScanState::kActive;
}

devicetree::ScanState ArmDevicetreePsciCpuSuspendItem::OnIdleStateFill(
    const devicetree::PropertyDecoder& decoder) {
  auto compatibles =
      decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
  if (!compatibles) {
    OnError("Failed to parse 'compatible' property");
    return devicetree::ScanState::kActive;
  }
  auto it = std::find_first_of(kIdleStateCompatibles.begin(), kIdleStateCompatibles.end(),
                               compatibles->begin(), compatibles->end());
  if (it == kIdleStateCompatibles.end()) {
    return devicetree::ScanState::kActive;
  }

  uint32_t flags =
      IsDomainIdleState(it) ? ZBI_ARM_PSCI_CPU_SUSPEND_STATE_FLAGS_TARGETS_POWER_DOMAIN : 0;
  auto [local_timer_stop, psci_param, entry, exit, residency] =
      decoder.FindProperties("local-timer-stop", "arm,psci-suspend-param", "entry-latency-us",
                             "exit-latency-us", "min-residency-us");

  if (local_timer_stop) {
    flags |= ZBI_ARM_PSCI_CPU_SUSPEND_STATE_FLAGS_LOCAL_TIMER_STOPS;
  }

  std::optional<uint32_t> parsed_psci_param;
  if (!psci_param) {
    Log("idle-state without `arm,psci-suspend-param` property.");
    return devicetree::ScanState::kActive;
  }

  parsed_psci_param = psci_param->AsUint32();
  if (!parsed_psci_param) {
    Log("Failed to parse idle-state `arm,psci-suspend-param` property.");
    return devicetree::ScanState::kActive;
  }

  auto parse_u32 = [this](auto maybe_property,
                          std::string_view prop_name) -> std::optional<uint32_t> {
    if (!maybe_property) {
      Log("`%*s` property missing.", static_cast<int>(prop_name.size()), prop_name.data());
      return std::nullopt;
    }

    auto parsed = maybe_property->AsUint32();
    if (!parsed) {
      Log("Failed to parse `%*s` property.", static_cast<int>(prop_name.size()), prop_name.data());
    }
    return parsed;
  };

  std::optional<uint32_t> entry_latency = parse_u32(entry, "entry-latency-us");
  std::optional<uint32_t> exit_latency = parse_u32(exit, "exit-latency-us");
  std::optional<uint32_t> min_residency = parse_u32(residency, "min-residency-us");

  auto& idle_state = idle_states()[current_idle_state_];
  idle_state = {
      .id = current_idle_state_,
      .power_state = *parsed_psci_param,
      .flags = flags,
      .entry_latency_us = entry_latency.value_or(0),
      .exit_latency_us = exit_latency.value_or(0),
      .min_residency_us = min_residency.value_or(0),
  };
  ++current_idle_state_;
  return devicetree::ScanState::kActive;
}

devicetree::ScanState ArmDevicetreePsciCpuSuspendItem::OnNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  // Start counting nodes.
  if (path.back() == "idle-states") {
    idle_states_ = &path.back();
    return devicetree::ScanState::kActive;
  }

  if (path.back() == "domain-idle-states") {
    domain_idle_states_ = &path.back();
    return devicetree::ScanState::kActive;
  }

  if (idle_states_) {
    if (states_ == nullptr) {
      return OnIdleStateCount(decoder);
    }
    return OnIdleStateFill(decoder);
  }

  // No point in specializing, since domain idle states may be contained
  // in `idle-states` anyway or on a separate node.
  if (domain_idle_states_) {
    if (states_ == nullptr) {
      return OnIdleStateCount(decoder);
    }
    return OnIdleStateFill(decoder);
  }

  return devicetree::ScanState::kActive;
}

fit::result<ArmDevicetreePsciCpuSuspendItem::DataZbi::Error>
ArmDevicetreePsciCpuSuspendItem::AppendItems(DataZbi& zbi) {
  // `num_idle_states_` is an upperbound on the number of possible valid idle states.
  // At this stage, `current_idle_state_` points to one past the last filled idle state, meaning
  // it accounts for possible invalid idle states being ignored, while `num_idle_states_` does not.
  if (current_idle_state_ == 0) {
    return fit::ok();
  }
  return zbi.Append(
      {
          .type = ZBI_TYPE_KERNEL_DRIVER,
          .extra = ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND,
      },
      std::as_bytes(idle_states().subspan(0, current_idle_state_)));
}

devicetree::ScanState ArmDevicetreePsciCpuSuspendItem::OnSubtree(const devicetree::NodePath& root) {
  if (&root.back() == idle_states_) {
    subtree_visited_count_++;
    idle_states_ = nullptr;
  } else if (&root.back() == domain_idle_states_) {
    subtree_visited_count_++;
    domain_idle_states_ = nullptr;
  }

  // Done early if we visited all of them.
  if (subtree_visited_count_ == 2) {
    return devicetree::ScanState::kDoneWithSubtree;
  }
  return devicetree::ScanState::kActive;
}

devicetree::ScanState ArmDevicetreePsciCpuSuspendItem::OnScan() {
  if (states_ == nullptr && num_idle_states_ > 0) {
    fbl::AllocChecker ac;
    states_ = reinterpret_cast<zbi_dcfg_arm_psci_cpu_suspend_state_t*>(
        (*allocator_)(num_idle_states_ * sizeof(zbi_dcfg_arm_psci_cpu_suspend_state_t),
                      alignof(zbi_dcfg_arm_psci_cpu_suspend_state_t), ac));
    if (!ac.check()) {
      num_idle_states_ = 0;
      OnError("Failed to allocate buffer for idle_states.");
      return devicetree::ScanState::kDone;
    }
    current_idle_state_ = 0;
    return devicetree::ScanState::kActive;
  }
  return devicetree::ScanState::kDone;
}

}  // namespace boot_shim
