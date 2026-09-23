// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-power.h"

#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#include <algorithm>
#include <cstdint>
#include <memory>
#include <vector>

#include <fbl/alloc_checker.h>
#include <soc/aml-common/aml-pwm-regs.h>

namespace power {

namespace {

// Sleep for 200 microseconds inorder to let the voltage change
// take effect. Source: Amlogic SDK.
constexpr uint32_t kVoltageSettleTimeUs = 200;
// Step up or down 3 steps in the voltage table while changing
// voltage and not directly. Source: Amlogic SDK
constexpr int kMaxVoltageChangeSteps = 3;

zx_status_t InitPwmProtocolClient(const fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm>& client) {
  if (!client.is_valid()) {
    // Optional fragment. See comment in AmlPower::Create.
    return ZX_OK;
  }

  auto result = client->Enable();
  if (!result.ok()) {
    fdf::error("Failed to send Enable request: {}", result.status_string());
    return result.status();
  }
  if (result.value().is_error()) {
    fdf::error("Failed to not enable PWM: {}", zx_status_get_string(result.value().error_value()));
    return result.value().error_value();
  }
  return ZX_OK;
}

bool IsSortedDescending(std::span<const fuchsia_hardware_amlogic_metadata::VoltageTableEntry> vt) {
  for (size_t i = 0; i < vt.size() - 1; i++) {
    if (vt[i].microvolt() < vt[i + 1].microvolt())
      // Bail early if we find a voltage that isn't strictly descending.
      return false;
  }
  return true;
}

uint32_t CalculateVregVoltage(const uint32_t min_uv, const uint32_t step_size_uv,
                              const uint32_t idx) {
  return min_uv + idx * step_size_uv;
}

}  // namespace

zx_status_t AmlPower::PowerImplWritePmicCtrlReg(uint32_t index, uint32_t addr, uint32_t value) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t AmlPower::PowerImplReadPmicCtrlReg(uint32_t index, uint32_t addr, uint32_t* value) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t AmlPower::PowerImplDisablePowerDomain(uint32_t index) {
  if (index >= domain_info_.size()) {
    fdf::error("Requested Disable for a domain that doesn't exist, idx = {}", index);
    return ZX_ERR_OUT_OF_RANGE;
  }

  return ZX_OK;
}

zx_status_t AmlPower::PowerImplEnablePowerDomain(uint32_t index) {
  if (index >= domain_info_.size()) {
    fdf::error("Requested Enable for a domain that doesn't exist, idx = {}", index);
    return ZX_ERR_OUT_OF_RANGE;
  }

  return ZX_OK;
}

zx_status_t AmlPower::PowerImplGetPowerDomainStatus(uint32_t index,
                                                    power_domain_status_t* out_status) {
  if (index >= domain_info_.size()) {
    fdf::error("Requested PowerImplGetPowerDomainStatus for a domain that doesn't exist, idx = {}",
               index);
    return ZX_ERR_OUT_OF_RANGE;
  }

  if (out_status == nullptr) {
    fdf::error("out_status must not be null");
    return ZX_ERR_INVALID_ARGS;
  }

  // All domains are always enabled.
  *out_status = POWER_DOMAIN_STATUS_ENABLED;
  return ZX_OK;
}

zx_status_t AmlPower::PowerImplGetSupportedVoltageRange(uint32_t index, uint32_t* min_voltage,
                                                        uint32_t* max_voltage) {
  if (!min_voltage || !max_voltage) {
    fdf::error("Need non-nullptr for min_voltage and max_voltage");
    return ZX_ERR_INVALID_ARGS;
  }

  if (index >= domain_info_.size()) {
    fdf::error("Requested GetSupportedVoltageRange for a domain that doesn't exist, idx = {}",
               index);
    return ZX_ERR_OUT_OF_RANGE;
  }

  auto& domain = domain_info_[index];
  if (domain.vreg) {
    fidl::WireResult params = (*domain.vreg)->GetRegulatorParams();
    if (!params.ok()) {
      fdf::error("Failed to send request to get regulator params: {}", params.status_string());
      return params.status();
    }

    if (params->is_error()) {
      fdf::error("Failed to get regulator params: {}", zx_status_get_string(params->error_value()));
      return params->error_value();
    }

    *min_voltage = CalculateVregVoltage(params.value()->min_uv, params.value()->step_size_uv, 0);
    *max_voltage = CalculateVregVoltage(params.value()->min_uv, params.value()->step_size_uv,
                                        params.value()->num_steps);
    fdf::debug("Getting {} Cluster VReg Range max = {}, min = {}", index ? "Little" : "Big",
               *max_voltage, *min_voltage);

    return ZX_OK;
  }
  if (domain.pwm) {
    // Voltage table is sorted in descending order so the minimum voltage is the last element and
    // the maximum voltage is the first element.
    *min_voltage = domain.voltage_table.back().microvolt();
    *max_voltage = domain.voltage_table.front().microvolt();
    fdf::debug("Getting {} Cluster VReg Range max = {}, min = {}", index ? "Little" : "Big",
               *max_voltage, *min_voltage);

    return ZX_OK;
  }

  fdf::error("Neither Vreg nor PWM are supported for this cluster. This should never happen.",
             __func__);
  return ZX_ERR_INTERNAL;
}

zx_status_t AmlPower::GetTargetIndex(const fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm>& pwm,
                                     uint32_t u_volts, const DomainInfo& domain,
                                     uint32_t* target_index) {
  if (!target_index) {
    return ZX_ERR_INTERNAL;
  }

  // Find the largest voltage that does not exceed u_volts.
  const fuchsia_hardware_amlogic_metadata::VoltageTableEntry target_voltage(
      {.microvolt = u_volts, .duty_cycle = 0});

  const auto& target =
      std::ranges::lower_bound(domain.voltage_table, target_voltage,
                               [](const fuchsia_hardware_amlogic_metadata::VoltageTableEntry& l,
                                  const fuchsia_hardware_amlogic_metadata::VoltageTableEntry& r) {
                                 return l.microvolt() > r.microvolt();
                               });

  if (target == domain.voltage_table.cend()) {
    fdf::error("Could not find a voltage less than or equal to {}\n", u_volts);
    return ZX_ERR_NOT_SUPPORTED;
  }

  size_t target_idx = target - domain.voltage_table.cbegin();
  if (target_idx >= INT_MAX || target_idx >= domain.voltage_table.size()) {
    fdf::error("voltage target index out of bounds");
    return ZX_ERR_OUT_OF_RANGE;
  }
  *target_index = static_cast<int>(target_idx);

  return ZX_OK;
}

zx_status_t AmlPower::GetTargetIndex(const fidl::WireSyncClient<fuchsia_hardware_vreg::Vreg>& vreg,
                                     uint32_t u_volts, const DomainInfo& domain,
                                     uint32_t* target_index) {
  if (!target_index) {
    return ZX_ERR_INTERNAL;
  }

  fidl::WireResult params = vreg->GetRegulatorParams();
  if (!params.ok()) {
    fdf::error("Failed to send request to get regulator params: {}", params.status_string());
    return params.status();
  }

  if (params->is_error()) {
    fdf::error("Failed to get regulator params: {}", zx_status_get_string(params->error_value()));
    return params->error_value();
  }

  const auto min_voltage_uv =
      CalculateVregVoltage(params.value()->min_uv, params.value()->step_size_uv, 0);
  const auto max_voltage_uv = CalculateVregVoltage(
      params.value()->min_uv, params.value()->step_size_uv, params.value()->num_steps);
  // Find the step value that achieves the requested voltage.
  if (u_volts < min_voltage_uv || u_volts > max_voltage_uv) {
    fdf::error("Voltage must be between {} and {} microvolts", min_voltage_uv, max_voltage_uv);
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (params.value()->step_size_uv == 0) {
    *target_index = 0;
  } else {
    *target_index = (u_volts - min_voltage_uv) / params.value()->step_size_uv;
  }
  ZX_ASSERT(*target_index <= params.value()->num_steps);

  return ZX_OK;
}

zx_status_t AmlPower::Update(const fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm>& pwm,
                             DomainInfo& domain, uint32_t target_idx) {
  aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig cfg = {
      .polarity = false,
      .period_ns = static_cast<uint32_t>(domain.pwm_period.to_nsecs()),
      .duty_cycle = static_cast<float>(domain.voltage_table[target_idx].duty_cycle()),
      .mode_config =
          fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  auto result = pwm->SetConfig(cfg);
  if (!result.ok()) {
    return result.status();
  }
  if (result.value().is_error()) {
    return result.value().error_value();
  }
  usleep(kVoltageSettleTimeUs);
  domain.current_voltage_index = target_idx;
  return ZX_OK;
}

zx_status_t AmlPower::Update(const fidl::WireSyncClient<fuchsia_hardware_vreg::Vreg>& vreg,
                             DomainInfo& domain, uint32_t target_idx) {
  fidl::WireResult step = vreg->SetVoltageStep(target_idx);
  if (!step.ok()) {
    fdf::error("Failed to send request to set voltage step: {}", step.status_string());
    return step.status();
  }
  if (step->is_error()) {
    fdf::error("Failed to set voltage step: {}", zx_status_get_string(step->error_value()));
    return step->error_value();
  }
  usleep(kVoltageSettleTimeUs);
  domain.current_voltage_index = target_idx;
  return ZX_OK;
}

template <class ProtocolClient>
zx_status_t AmlPower::RequestVoltage(const ProtocolClient& client, uint32_t u_volts,
                                     DomainInfo& domain) {
  uint32_t target_idx;
  auto status = GetTargetIndex(client, u_volts, domain, &target_idx);
  if (status != ZX_OK) {
    fdf::error("Could not get target index\n");
    return status;
  }

  // If this is the first time we are setting up the voltage
  // we directly set it.
  if (domain.current_voltage_index == DomainInfo::kInvalidIndex) {
    status = Update(client, domain, target_idx);
    if (status != ZX_OK) {
      fdf::error("Could not update");
      return status;
    }
    return ZX_OK;
  }

  // Otherwise we adjust to the target voltage step by step.
  auto target_index = static_cast<int>(target_idx);
  while (domain.current_voltage_index != target_index) {
    if (domain.current_voltage_index < target_index) {
      if (domain.current_voltage_index < target_index - kMaxVoltageChangeSteps) {
        // Step up by 3 in the voltage table.
        domain.current_voltage_index += kMaxVoltageChangeSteps;
      } else {
        domain.current_voltage_index = target_index;
      }
    } else {
      if (domain.current_voltage_index > target_index + kMaxVoltageChangeSteps) {
        // Step down by 3 in the voltage table.
        domain.current_voltage_index -= kMaxVoltageChangeSteps;
      } else {
        domain.current_voltage_index = target_index;
      }
    }
    status = Update(client, domain, domain.current_voltage_index);
    if (status != ZX_OK) {
      fdf::error("Could not update");
      return status;
    }
  }
  return ZX_OK;
}

zx_status_t AmlPower::PowerImplRequestVoltage(uint32_t index, uint32_t voltage,
                                              uint32_t* actual_voltage) {
  if (index >= domain_info_.size()) {
    fdf::error("Requested voltage for a range that doesn't exist, idx = {}", index);
    return ZX_ERR_OUT_OF_RANGE;
  }

  auto& domain = domain_info_[index];
  if (domain.pwm) {
    zx_status_t st = RequestVoltage(domain.pwm.value(), voltage, domain);
    if ((st == ZX_OK) && actual_voltage) {
      *actual_voltage = domain.voltage_table[domain.current_voltage_index].microvolt();
    }
    return st;
  }

  if (domain.vreg) {
    zx_status_t st = RequestVoltage(domain.vreg.value(), voltage, domain);
    if ((st == ZX_OK) && actual_voltage) {
      fidl::WireResult params = (*domain.vreg)->GetRegulatorParams();
      if (!params.ok()) {
        fdf::error("Failed to send request to get regulator params: {}", params.status_string());
        return params.status();
      }

      if (params->is_error()) {
        fdf::error("Failed to get regulator params: {}",
                   zx_status_get_string(params->error_value()));
        return params->error_value();
      }

      *actual_voltage = CalculateVregVoltage(params.value()->min_uv, params.value()->step_size_uv,
                                             domain.current_voltage_index);
    }
    return st;
  }

  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t AmlPower::PowerImplGetCurrentVoltage(uint32_t index, uint32_t* current_voltage) {
  if (!current_voltage) {
    fdf::error("Cannot take nullptr for current_voltage");
    return ZX_ERR_INVALID_ARGS;
  }

  if (index >= domain_info_.size()) {
    fdf::error("Requested voltage for a range that doesn't exist, idx = {}", index);
    return ZX_ERR_OUT_OF_RANGE;
  }

  auto& domain = domain_info_[index];
  if (domain.pwm) {
    if (domain.current_voltage_index == DomainInfo::kInvalidIndex)
      return ZX_ERR_BAD_STATE;
    *current_voltage = domain.voltage_table[domain.current_voltage_index].microvolt();
  } else if (domain.vreg) {
    fidl::WireResult params = (*domain.vreg)->GetRegulatorParams();
    if (!params.ok()) {
      fdf::error("Failed to send request to get regulator params: {}", params.status_string());
      return params.status();
    }

    if (params->is_error()) {
      fdf::error("Failed to get regulator params: {}", zx_status_get_string(params->error_value()));
      return params->error_value();
    }

    *current_voltage = CalculateVregVoltage(params.value()->min_uv, params.value()->step_size_uv,
                                            domain.current_voltage_index);
  } else {
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

zx::result<> AmlPower::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result pdev_client = incoming->Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_client.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client);
    return pdev_client.take_error();
  }
  if (zx::result result =
          metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), pdev_client.value());
      result.is_error()) {
    fdf::error("Failed to forward and serve metadata: {}", result);
    return result.take_error();
  }
  fdf::PDev pdev(std::move(pdev_client.value()));

  compat::DeviceServer::BanjoConfig banjo_config{.default_proto_id = ZX_PROTOCOL_POWER_IMPL};
  banjo_config.callbacks[ZX_PROTOCOL_POWER_IMPL] = banjo_server_.callback();
  zx::result<> result =
      compat_server_.Initialize(incoming, outgoing(), context.node_name(), kChildNodeName,
                                compat::ForwardMetadata::None(), std::move(banjo_config));
  if (result.is_error()) {
    fdf::error("Failed to initialize compat server: {}", result);
    return result.take_error();
  }

  // Create tries to get all possible metadata and fragments. The required combination of metadata
  // and fragment is expected to be configured appropriately by the board driver. After gathering
  // all the available metadata and fragment DomainInfo for little core and big core (if exists) is
  // populated. DomainInfo vector is then used to construct AmlPower.
  zx::result metadata = pdev.GetFidlMetadata<fuchsia_hardware_amlogic_metadata::PowerMetadata>();
  if (metadata.is_ok()) {
    if (!metadata->voltage_table().has_value()) {
      fdf::error("Metadata missing `voltage_table` field");
      return zx::error(ZX_ERR_INTERNAL);
    }
    if (!metadata->voltage_pwm_period().has_value()) {
      fdf::error("Metadata missing `voltage_pwm_period` field");
      return zx::error(ZX_ERR_INTERNAL);
    }

    if (!IsSortedDescending(metadata->voltage_table().value())) {
      fdf::error("Voltage table was not sorted in strictly descending order");
      return zx::error(ZX_ERR_INTERNAL);
    }
  } else if (metadata.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::error("Failed to get metadata: {}", metadata);
    return metadata.take_error();
  }

  zx::result client_end =
      incoming->Connect<fuchsia_hardware_pwm::Service::Pwm>(kPwmPrimaryParentName);
  fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> primary_cluster_pwm;
  // The fragment may be optional, so we do not return on error.
  if (client_end.is_ok()) {
    fdf::info("Connected to primary pwm");
    primary_cluster_pwm.Bind(std::move(client_end.value()));
    zx_status_t status = InitPwmProtocolClient(primary_cluster_pwm);
    if (status != ZX_OK) {
      fdf::error("Failed to initialize Big Cluster PWM Client: {}", zx_status_get_string(status));
      return zx::error(status);
    }
  }

  zx::result little_cluster_vreg =
      incoming->Connect<fuchsia_hardware_vreg::Service::Vreg>(kVregPwmLittleParentName);

  zx::result big_cluster_vreg =
      incoming->Connect<fuchsia_hardware_vreg::Service::Vreg>(kVregPwmBigParentName);

  if (primary_cluster_pwm.is_valid() && metadata.is_ok()) {
    // For Astro.
    domain_info_.emplace_back(std::move(primary_cluster_pwm),
                              std::move(metadata->voltage_table().value()),
                              zx::duration(metadata->voltage_pwm_period().value()));
  } else {
    // For Vim3.
    if (big_cluster_vreg.is_error() || little_cluster_vreg.is_error()) {
      fdf::error("Unable to connect to VReg Devices: big={}, little={}", big_cluster_vreg,
                 little_cluster_vreg);
      return zx::error(ZX_ERR_INTERNAL);
    }

    // TODO(b/376751395): This conflates power domain IDs with indices in the domain_info vector.
    //                    In other words domain IDs are implicitly coupled to their index in this
    //                    vector which creates a fragile mapping from power domain ID to power.
    //                    We should reconsider this.
    domain_info_.emplace_back(
        fidl::WireSyncClient<fuchsia_hardware_vreg::Vreg>(std::move(big_cluster_vreg.value())));
    domain_info_.emplace_back(
        fidl::WireSyncClient<fuchsia_hardware_vreg::Vreg>(std::move(little_cluster_vreg.value())));
  }

  std::vector offers = compat_server_.CreateOffers2();
  std::optional metadata_offer = metadata_server_.CreateOffer();
  if (metadata_offer.has_value()) {
    offers.push_back(std::move(metadata_offer.value()));
  }
  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, static_cast<uint32_t>(ZX_PROTOCOL_POWER_IMPL))};
  zx::result child = AddChild(kChildNodeName, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }

  return zx::ok();
}

}  // namespace power

FUCHSIA_DRIVER_EXPORT2(power::AmlPower);
