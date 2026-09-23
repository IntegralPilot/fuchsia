// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/ufs/device_manager.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>

#include "src/devices/block/drivers/ufs/registers.h"
#include "src/devices/block/drivers/ufs/transfer_request_processor.h"
#include "src/devices/block/drivers/ufs/ufs.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"
#include "transfer_request_processor.h"

namespace ufs {
zx::result<std::unique_ptr<DeviceManager>> DeviceManager::Create(
    Ufs &controller, TransferRequestProcessor &transfer_request_processor,
    InspectProperties &properties) {
  fbl::AllocChecker ac;
  auto device_manager = fbl::make_unique_checked<DeviceManager>(
      &ac, controller, transfer_request_processor, properties);
  if (!ac.check()) {
    fdf::error("Failed to allocate device manager.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(device_manager));
}

zx::result<> DeviceManager::SendLinkStartUp() {
  if (zx::result<> result = controller_.Notify(NotifyEvent::kPreLinkStartup, 0);
      result.is_error()) {
    fdf::error("PreLinkStartup callback failed: {}", result);
    return result.take_error();
  }

  constexpr size_t kMaxLinkStartUpAttempts = 3;
  for (size_t attempt = 1; attempt <= kMaxLinkStartUpAttempts; ++attempt) {
    DmeLinkStartUpUicCommand link_startup_command(controller_);
    auto result = link_startup_command.SendCommand();
    if (result.is_ok()) {
      std::lock_guard<std::mutex> lock(power_lock_);
      current_link_state_ = LinkState::kActive;
      return zx::ok();
    }
    fdf::warn("UFS link startup attempt {}/{} failed: {}", attempt, kMaxLinkStartUpAttempts,
              result.status_string());
    if (attempt < kMaxLinkStartUpAttempts) {
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
  }

  fdf::error("Failed to startup UFS link after {} attempts", kMaxLinkStartUpAttempts);
  return zx::error(ZX_ERR_INTERNAL);
}

zx::result<> DeviceManager::DeviceInit() {
  zx::time device_init_start_time = zx::clock::get_monotonic();
  if (zx::result<> result = SetFlag(Flags::fDeviceInit); result.is_error()) {
    return result.take_error();
  }

  zx::time device_init_time_out = device_init_start_time + zx::usec(kDeviceInitTimeoutUs);
  while (true) {
    zx::result<uint8_t> flag = ReadFlag(Flags::fDeviceInit);
    if (flag.is_error()) {
      return flag.take_error();
    }

    if (!flag.value())
      break;

    if (zx::clock::get_monotonic() > device_init_time_out) {
      fdf::error("Wait for fDeviceInit timed out");
      return zx::error(ZX_ERR_TIMED_OUT);
    }
    usleep(10000);
  }
  return zx::ok();
}

zx::result<> DeviceManager::GetControllerDescriptor() {
  auto device_descriptor = ReadDescriptor<DeviceDescriptor>(DescriptorType::kDevice);
  if (device_descriptor.is_error()) {
    return device_descriptor.take_error();
  }
  device_descriptor_ = device_descriptor.value();

  // The field definitions for VersionReg and wSpecVersion are the same.
  // wSpecVersion use big-endian byte ordering.
  auto version = VersionReg::Get().FromValue(betoh16(device_descriptor_.wSpecVersion));
  fdf::info("UFS device version {}.{}{}", version.major_version_number(),
            version.minor_version_number(), version.version_suffix());

  fdf::info("{} enabled LUNs found", device_descriptor_.bNumberLU);

  auto geometry_descriptor = ReadDescriptor<GeometryDescriptor>(DescriptorType::kGeometry);
  if (geometry_descriptor.is_error()) {
    return geometry_descriptor.take_error();
  }
  geometry_descriptor_ = geometry_descriptor.value();

  // TODO(https://fxbug.dev/42075643): We need to functionalize the code to get max_lun_count_.
  if (geometry_descriptor_.bMaxNumberLU == 0) {
    max_lun_count_ = 8;
  } else if (geometry_descriptor_.bMaxNumberLU == 1) {
    max_lun_count_ = 32;
  } else {
    fdf::error("Invalid Geometry Descriptor bMaxNumberLU value={}",
               geometry_descriptor_.bMaxNumberLU);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // The kDeviceDensityUnit is defined in the spec as 512.
  // qTotalRawDeviceCapacity use big-endian byte ordering.
  constexpr uint32_t kDeviceDensityUnit = 512;
  fdf::info("UFS device total size is {} bytes",
            betoh64(geometry_descriptor_.qTotalRawDeviceCapacity) * kDeviceDensityUnit);

  return zx::ok();
}

zx::result<uint32_t> DeviceManager::ReadAttribute(Attributes attribute, uint8_t index) {
  ReadAttributeUpiu read_attribute_upiu(attribute, index);
  auto query_response = req_processor_.SendQueryRequestUpiu(read_attribute_upiu);
  if (query_response.is_error()) {
    return query_response.take_error();
  }
  return zx::ok(query_response->GetResponse<AttributeResponseUpiu>().GetAttribute());
}

zx::result<> DeviceManager::WriteAttribute(Attributes attribute, uint32_t value, uint8_t index) {
  WriteAttributeUpiu write_attribute_upiu(attribute, value, index);
  auto query_response = req_processor_.SendQueryRequestUpiu(write_attribute_upiu);
  if (query_response.is_error()) {
    return query_response.take_error();
  }
  return zx::ok();
}

template <typename DescriptorReturnType>
zx::result<DescriptorReturnType> DeviceManager::ReadDescriptor(DescriptorType descriptor,
                                                               uint8_t index) {
  ReadDescriptorUpiu read_descriptor_upiu(descriptor, index, sizeof(DescriptorReturnType));
  auto query_response = req_processor_.SendQueryRequestUpiu(read_descriptor_upiu);
  if (query_response.is_error()) {
    return query_response.take_error();
  }
  DescriptorReturnType data;
  CustomMemCpy(
      &data,
      &query_response->GetResponse<DescriptorResponseUpiu>().GetDescriptor<DescriptorReturnType>(),
      sizeof(data));
  return zx::ok(data);
}

zx::result<uint8_t> DeviceManager::ReadFlag(Flags type) {
  ReadFlagUpiu read_flag_upiu(type);
  auto query_response = req_processor_.SendQueryRequestUpiu(read_flag_upiu);
  if (query_response.is_error()) {
    return query_response.take_error();
  }
  return zx::ok(query_response->GetResponse<FlagResponseUpiu>().GetFlag());
}

zx::result<> DeviceManager::SetFlag(Flags type) {
  SetFlagUpiu set_flag_upiu(type);
  auto query_response = req_processor_.SendQueryRequestUpiu(set_flag_upiu);
  if (query_response.is_error()) {
    return query_response.take_error();
  }
  return zx::ok();
}

zx::result<> DeviceManager::ClearFlag(Flags type) {
  ClearFlagUpiu claer_flag_upiu(type);
  auto query_response = req_processor_.SendQueryRequestUpiu(claer_flag_upiu);
  if (query_response.is_error()) {
    return query_response.take_error();
  }
  return zx::ok();
}

zx::result<uint32_t> DeviceManager::DmeGet(uint16_t mib_attribute) {
  DmeGetUicCommand dme_get_command(controller_, mib_attribute, 0);
  auto value = dme_get_command.SendCommand();
  if (value.is_error()) {
    return value.take_error();
  }
  if (!value.value().has_value()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok(value.value().value());
}

zx::result<uint32_t> DeviceManager::DmePeerGet(uint16_t mib_attribute) {
  DmePeerGetUicCommand dme_peer_get_command(controller_, mib_attribute, 0);
  auto value = dme_peer_get_command.SendCommand();
  if (value.is_error()) {
    return value.take_error();
  }
  if (!value.value().has_value()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok(value.value().value());
}

zx::result<> DeviceManager::DmeSet(uint16_t mib_attribute, uint32_t value) {
  DmeSetUicCommand dme_set_command(controller_, mib_attribute, 0, AttrSetType::kVolatile, value);
  if (auto result = dme_set_command.SendCommand(); result.is_error()) {
    return result.take_error();
  }
  return zx::ok();
}

zx::result<uint32_t> DeviceManager::GetBootLunEnabled() {
  // Read bBootLunEn to confirm device interface is ok.
  zx::result<uint32_t> boot_lun_enabled = ReadAttribute(Attributes::bBootLunEn);
  if (boot_lun_enabled.is_error()) {
    return boot_lun_enabled.take_error();
  }
  fdf::debug("bBootLunEn 0x{:x}", boot_lun_enabled.value());
  return zx::ok(boot_lun_enabled.value());
}

zx::result<UnitDescriptor> DeviceManager::ReadUnitDescriptor(uint8_t lun) {
  return ReadDescriptor<UnitDescriptor>(DescriptorType::kUnit, lun);
}

zx::result<RpmbUnitDescriptor> DeviceManager::ReadRpmbUnitDescriptor() {
  return ReadDescriptor<RpmbUnitDescriptor>(DescriptorType::kUnit,
                                            static_cast<uint8_t>(WellKnownLuns::kRpmb));
}

zx::result<> DeviceManager::SetExceptionEventControl(ExceptionEventControl control) {
  if (exception_event_control_.value == control.value) {
    return zx::ok();
  }

  if (auto result = WriteAttribute(Attributes::wExceptionEventControl, control.value);
      result.is_error()) {
    return result.take_error();
  }
  exception_event_control_.value = control.value;

  return zx::ok();
}

zx::result<ExceptionEventStatus> DeviceManager::GetExceptionEventStatus() {
  auto ee_status_attribute = ReadAttribute(Attributes::wExceptionEventStatus);
  if (ee_status_attribute.is_error()) {
    return ee_status_attribute.take_error();
  }

  ExceptionEventStatus ee_status = {safemath::checked_cast<uint16_t>(ee_status_attribute.value())};
  return zx::ok(ee_status);
}

zx::result<> DeviceManager::PostExceptionEventsTask() {
  zx_status_t status = async::PostTask(controller_.exception_event_dispatcher().async_dispatcher(),
                                       [this] { HandleExceptionEvents(); });
  if (status != ZX_OK) {
    fdf::error("Failed to post Exception Event task: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok();
}

void DeviceManager::HandleExceptionEvents() {
  zx::result<ExceptionEventStatus> ee_status = GetExceptionEventStatus();
  if (ee_status.is_error()) {
    fdf::error("Failed to get Exception Event Status");
  }
  if (ee_status->urgent_bkops()) {
    if (auto result = HandleBackgroundOpEvent(); result.is_error()) {
      fdf::error("Failed to handle Background Operations Event");
    }
  }
  if (ee_status->too_high_temp() || ee_status->too_low_temp()) {
    // TODO(b/42075643): Implement temp exception handler.
    fdf::info("A temperature exception has occurred");
  }
}

zx::result<> DeviceManager::HandleBackgroundOpEvent() {
  zx::result<BackgroundOpStatus> bkop_status = GetBackgroundOpStatus();
  if (bkop_status.is_error()) {
    return bkop_status.take_error();
  }

  if (bkop_status.value() >= urgent_bkop_threshold_) {
    if (zx::result result = EnableBackgroundOp(); result.is_error()) {
      return result.take_error();
    }
  }
  return zx::ok();
}

zx::result<> DeviceManager::ConfigureWriteProtect(inspect::Node &wp_node) {
  zx::result<uint8_t> flag = ReadFlag(Flags::fPowerOnWPEn);
  if (flag.is_error()) {
    return flag.take_error();
  }
  is_power_on_write_protect_enabled_ = flag.value();
  properties_.is_power_on_write_protect_enabled =
      wp_node.CreateBool("is_power_on_write_protect_enabled", is_power_on_write_protect_enabled_);
  properties_.logical_lun_power_on_write_protect =
      wp_node.CreateBool("logical_lun_power_on_write_protect", logical_lun_power_on_write_protect_);
  return zx::ok();
}

void DeviceManager::SetLogicalLunPowerOnWriteProtect(bool value) {
  logical_lun_power_on_write_protect_ = value;
  properties_.logical_lun_power_on_write_protect.Set(value);
}

zx::result<> DeviceManager::ConfigureBackgroundOp(inspect::Node &bkop_node) {
  zx::result<uint8_t> flag = ReadFlag(Flags::fBackgroundOpsEn);
  if (flag.is_error()) {
    return flag.take_error();
  }
  is_background_op_enabled_ = flag.value();
  properties_.is_background_op_enabled =
      bkop_node.CreateBool("is_background_op_enabled", is_background_op_enabled_);

  // Currently we allow background operations in the active state. This may have a performance
  // penalty.
  // TODO(b/42075643): We should only perform background operations in the power suspended state.
  if (zx::result<> result = EnableBackgroundOp(); result.is_error()) {
    return result.take_error();
  }

  // For stable performance, set threshold of the Background Operation to `Required, not critical`.
  urgent_bkop_threshold_ = BackgroundOpStatus::kRequiredNotCritical;

  return zx::ok();
}

zx::result<> DeviceManager::EnableBackgroundOp() {
  if (is_background_op_enabled_) {
    return zx::ok();
  }

  if (zx::result<> result = SetFlag(Flags::fBackgroundOpsEn); result.is_error()) {
    return result.take_error();
  }
  is_background_op_enabled_ = true;
  properties_.is_background_op_enabled.Set(is_background_op_enabled_);

  // No need for urgent background operation exceptions.
  ExceptionEventControl control = {exception_event_control_.value};
  control.set_urgent_bkops_en(false);
  if (auto result = SetExceptionEventControl(control); result.is_error()) {
    fdf::warn("Failed to set exception event control: {}", result.status_string());
  }
  return zx::ok();
}

zx::result<> DeviceManager::DisableBackgroundOp() {
  if (!is_background_op_enabled_) {
    return zx::ok();
  }

  // Need urgent background operation exceptions.
  ExceptionEventControl control = {exception_event_control_.value};
  control.set_urgent_bkops_en(true);
  if (auto result = SetExceptionEventControl(control); result.is_error()) {
    fdf::warn("Failed to set exception event control: {}", result.status_string());
  }

  if (zx::result<> result = ClearFlag(Flags::fBackgroundOpsEn); result.is_error()) {
    return result.take_error();
  }
  is_background_op_enabled_ = false;
  properties_.is_background_op_enabled.Set(is_background_op_enabled_);

  return zx::ok();
}

zx::result<BackgroundOpStatus> DeviceManager::GetBackgroundOpStatus() {
  zx::result<uint32_t> bkop_status_attribute = ReadAttribute(Attributes::bBackgroundOpStatus);
  if (bkop_status_attribute.is_error()) {
    return bkop_status_attribute.take_error();
  }
  if (bkop_status_attribute.value() > BackgroundOpStatus::kCritical) {
    fdf::error("Invalid BackgroundOpStatus: {}", bkop_status_attribute.value());
    return zx::error(ZX_ERR_BAD_STATE);
  }
  BackgroundOpStatus background_op_status =
      static_cast<BackgroundOpStatus>(bkop_status_attribute.value());
  return zx::ok(background_op_status);
}

zx::result<> DeviceManager::ConfigureWriteBooster(inspect::Node &wb_node) {
  // Copy to access the unaligned value.
  const ExtendedUfsFeaturesSupport kExtendedUfsFeaturesSupport{
      betoh32(device_descriptor_.dExtendedUfsFeaturesSupport)};

  if (!kExtendedUfsFeaturesSupport.writebooster_support()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  if (zx::result<> result = EnableWriteBooster(wb_node); result.is_error()) {
    fdf::error("Failed to enable WriteBooster: {}", result);
    return result.take_error();
  }

  auto disable_write_booster = fit::defer([&] {
    if (is_write_booster_enabled_) {
      if (zx::result<> result = DisableWriteBooster(); result.is_error()) {
        fdf::error("Failed to disable WriteBooster: {}", result);
      } else {
        fdf::warn("WriteBooster is disabled");
      }
    }
  });

  // Get WriteBooster buffer parameters.
  write_booster_buffer_type_ =
      static_cast<WriteBoosterBufferType>(device_descriptor_.bWriteBoosterBufferType);
  properties_.write_booster_buffer_type = wb_node.CreateUint(
      "write_booster_buffer_type", static_cast<uint8_t>(write_booster_buffer_type_));

  user_space_configuration_option_ = static_cast<UserSpaceConfigurationOption>(
      device_descriptor_.bWriteBoosterBufferPreserveUserSpaceEn);
  properties_.user_space_configuration_option = wb_node.CreateUint(
      "user_space_configuration_option", static_cast<uint8_t>(user_space_configuration_option_));

  // Find the size of the write buffer.
  uint32_t alloc_units = 0;
  if (write_booster_buffer_type_ == WriteBoosterBufferType::kSharedBuffer) {
    alloc_units = betoh32(device_descriptor_.dNumSharedWriteBoosterBufferAllocUnits);
  } else if (write_booster_buffer_type_ == WriteBoosterBufferType::kLuDedicatedBuffer) {
    for (uint8_t lun = 0; lun < max_lun_count_; ++lun) {
      auto unit_descriptor = ReadDescriptor<UnitDescriptor>(DescriptorType::kUnit, lun);
      if (unit_descriptor.is_error()) {
        continue;
      }
      alloc_units = betoh32(unit_descriptor->dLUNumWriteBoosterBufferAllocUnits);
      if (alloc_units > 0) {
        // Found a dedicated buffer from LU.
        write_booster_dedicated_lu_ = lun;
        properties_.write_booster_dedicated_lu =
            wb_node.CreateUint("write_booster_dedicated_lu", write_booster_dedicated_lu_);
        break;
      }
    }
  } else {
    fdf::warn("Not supported WriteBooster buffer type: 0x{:x}",
              static_cast<uint8_t>(write_booster_buffer_type_));
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  if (alloc_units == 0) {
    // Unable to enable WriteBooster due to lack of resources.
    fdf::warn("The WriteBooster buffer size is zero.");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  uint32_t write_booster_buffer_size_in_bytes =
      alloc_units * static_cast<uint32_t>(geometry_descriptor_.bAllocationUnitSize) *
      (betoh32(geometry_descriptor_.dSegmentSize)) * kSectorSize;
  properties_.write_booster_buffer_size_in_bytes =
      wb_node.CreateUint("write_booster_buffer_size_in_bytes", write_booster_buffer_size_in_bytes);

  zx::result<bool> result = IsWriteBoosterBufferLifeTimeLeft();
  if (result.is_error()) {
    fdf::error("Failed to IsWriteBoosterBufferLifeTimeLeft(): {}", result);
    return result.take_error();
  }
  if (!result.value()) {
    // Unable to enable WriteBooster due to lack of resources.
    fdf::warn("Exceeded its maximum estimated WriteBooster Buffer life time");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // TODO(https://fxbug.dev/42075643): Need to handle WRITEBOOSTER_FLUSH_NEEDED exception case.

  disable_write_booster.cancel();
  fdf::info("WriteBooster is enabled");
  return zx::ok();
}

zx::result<bool> DeviceManager::IsWriteBoosterBufferLifeTimeLeft() {
  uint8_t buffer_lun = write_booster_buffer_type_ == WriteBoosterBufferType::kLuDedicatedBuffer
                           ? write_booster_dedicated_lu_
                           : 0;
  zx::result<uint32_t> life_time = ReadAttribute(Attributes::bWBBufferLifeTimeEst, buffer_lun);
  if (life_time.is_error()) {
    return life_time.take_error();
  }
  if (life_time.value()) {
    if (life_time == kExceededWriteBoosterBufferLifeTime) {
      return zx::ok(false);
    }
  }

  return zx::ok(true);
}

zx::result<> DeviceManager::EnableWriteBooster(inspect::Node &wb_node) {
  // Enable WriteBooster.
  if (zx::result<> result = SetFlag(Flags::fWriteBoosterEn); result.is_error()) {
    return result.take_error();
  }
  is_write_booster_enabled_ = true;
  properties_.is_write_booster_enabled =
      wb_node.CreateBool("is_write_booster_enabled", is_write_booster_enabled_);

  // Enable WriteBooster buffer flush during hibernate.
  if (zx::result<> result = SetFlag(Flags::fWBBufferFlushDuringHibernate); result.is_error()) {
    return result.take_error();
  }
  properties_.writebooster_buffer_flush_during_hibernate =
      wb_node.CreateBool("writebooster_buffer_flush_during_hibernate", true);

  // Enable WriteBooster buffer flush.
  // TODO(https://fxbug.dev/42075643): For Samsung Exynos, ignore this flush behaviour due to the
  // quirk of not supporting manual flush.
  if (zx::result<> result = SetFlag(Flags::fWBBufferFlushEn); result.is_error()) {
    return result.take_error();
  }
  is_write_booster_flush_enabled_ = true;
  properties_.writebooster_buffer_flush_enabled =
      wb_node.CreateBool("writebooster_buffer_flush_enabled", is_write_booster_flush_enabled_);

  return zx::ok();
}

zx::result<> DeviceManager::DisableWriteBooster() {
  if (is_write_booster_flush_enabled_) {
    // Disable WriteBooster buffer flush.
    if (zx::result<> result = ClearFlag(Flags::fWBBufferFlushEn); result.is_error()) {
      return result.take_error();
    }
    is_write_booster_flush_enabled_ = false;
    properties_.writebooster_buffer_flush_enabled.Set(is_write_booster_flush_enabled_);
  }

  // Disable WriteBooster buffer flush during hibernate.
  if (zx::result<> result = ClearFlag(Flags::fWBBufferFlushDuringHibernate); result.is_error()) {
    return result.take_error();
  }
  properties_.writebooster_buffer_flush_during_hibernate.Set(false);

  // Disable WriteBooster.
  if (zx::result<> result = ClearFlag(Flags::fWriteBoosterEn); result.is_error()) {
    return result.take_error();
  }
  is_write_booster_enabled_ = false;
  properties_.is_write_booster_enabled.Set(is_write_booster_enabled_);

  return zx::ok();
}

zx::result<bool> DeviceManager::NeedWriteBoosterBufferFlush() {
  if (!is_write_booster_enabled_) {
    return zx::ok(false);
  }

  auto result = IsWriteBoosterBufferLifeTimeLeft();
  if (result.is_error()) {
    return result.take_error();
  }
  if (!result.value()) {
    if (auto result = DisableWriteBooster(); result.is_error()) {
      fdf::error("Failed to disable WriteBooster: {}", result);
      return result.take_error();
    }
    return zx::ok(false);
  }

  uint8_t buffer_lun = write_booster_buffer_type_ == WriteBoosterBufferType::kLuDedicatedBuffer
                           ? write_booster_dedicated_lu_
                           : 0;
  auto available_buffer_size = ReadAttribute(Attributes::bAvailableWBBufferSize, buffer_lun);
  if (available_buffer_size.is_error()) {
    return available_buffer_size.take_error();
  }

  switch (user_space_configuration_option_) {
    case UserSpaceConfigurationOption::kUserSpaceReduction: {
      // In kUserSpaceReduction mode, flush should be performed when 10% or less of the buffer is
      // left.
      constexpr uint32_t k10PercentBufferRemains = 0x01;
      return zx::ok(available_buffer_size.value() <= k10PercentBufferRemains);
    }
    case UserSpaceConfigurationOption::kPreserveUserSpace: {
      // In kPreserveUserSpace mode, flush should be performed when the current buffer is greater
      // than 0 and the available buffer below write_booster_flush_threshold_ is left.
      auto current_buffer_size = ReadAttribute(Attributes::dCurrentWBBufferSize, buffer_lun);
      if (current_buffer_size.is_error()) {
        return current_buffer_size.take_error();
      }
      if (!current_buffer_size.value()) {
        return zx::ok(false);
      }
      return zx::ok(available_buffer_size.value() < write_booster_flush_threshold_);
    }
    default:
      return zx::error(ZX_ERR_INVALID_ARGS);
  }
}

zx::result<> DeviceManager::InitReferenceClock(inspect::Node &controller_node) {
  // Intel UFSHCI reference clock = 19.2MHz
  const AttributeReferenceClock reference_clock = controller_.intel_quirk()
                                                      ? AttributeReferenceClock::k19_2MHz
                                                      : AttributeReferenceClock::k38_4MHz;
  if (auto result = WriteAttribute(Attributes::bRefClkFreq, reference_clock); result.is_error()) {
    return result.take_error();
  }

  std::string reference_clock_string;
  switch (reference_clock) {
    case AttributeReferenceClock::k19_2MHz:
      reference_clock_string = "19.2 MHz";
      break;
    case AttributeReferenceClock::k26MHz:
      reference_clock_string = "26 MHz";
      break;
    case AttributeReferenceClock::k38_4MHz:
      reference_clock_string = "38.4 MHz";
      break;
    case AttributeReferenceClock::kObsolete:
      reference_clock_string = "52 MHz (Obsolete))";
      break;
    default:
      return zx::error(ZX_ERR_INVALID_ARGS);
  }
  properties_.reference_clock =
      controller_node.CreateString("reference_clock", reference_clock_string);

  return zx::ok();
}

zx::result<> DeviceManager::InitUniproAttributes(inspect::Node &unipro_node) {
  // UniPro Version
  // 7~15 = Above 2.0, 6 = 2.0, 5 = 1.8, 4 = 1.61, 3 = 1.6, 2 = 1.41, 1 = 1.40, 0 = Reserved
  zx::result<uint32_t> remote_version = DmeGet(PA_RemoteVerInfo);
  if (remote_version.is_error()) {
    return remote_version.take_error();
  }
  zx::result<uint32_t> local_version = DmeGet(PA_LocalVerInfo);
  if (local_version.is_error()) {
    return local_version.take_error();
  }

  // UniPro automatically sets timing information such as PA_TActivate through the
  // PACP_CAP_EXT1_ind command during Link Startup operation.
  zx::result<uint32_t> host_t_activate = DmeGet(PA_TActivate);
  if (host_t_activate.is_error()) {
    return host_t_activate.take_error();
  }

  if (controller_.intel_quirk()) {
    // Intel Lake-field UFSHCI has a quirk. We need to add 200us to the PEER's PA_TActivate.
    DmePeerSetUicCommand dme_peer_set_t_activate(
        controller_, PA_TActivate, 0, AttrSetType::kVolatile, host_t_activate.value() + 2);
    if (auto result = dme_peer_set_t_activate.SendCommand(); result.is_error()) {
      return result.take_error();
    }
  }

  zx::result<uint32_t> device_t_activate = DmePeerGet(PA_TActivate);
  if (device_t_activate.is_error()) {
    return device_t_activate.take_error();
  }
  // PA_Granularity = 100us (1=1us, 2=4us, 3=8us, 4=16us, 5=32us, 6=100us)
  zx::result<uint32_t> host_granularity = DmeGet(PA_Granularity);
  if (host_granularity.is_error()) {
    return host_granularity.take_error();
  }
  zx::result<uint32_t> device_granularity = DmePeerGet(PA_Granularity);
  if (device_granularity.is_error()) {
    return device_granularity.take_error();
  }

  properties_.remote_version = unipro_node.CreateUint("remote_version", remote_version.value());
  properties_.local_version = unipro_node.CreateUint("local_version", local_version.value());
  properties_.host_t_activate = unipro_node.CreateUint("host_t_activate", host_t_activate.value());
  properties_.device_t_activate =
      unipro_node.CreateUint("device_t_activate", device_t_activate.value());
  properties_.host_granularity =
      unipro_node.CreateUint("host_granularity", host_granularity.value());
  properties_.device_granularity =
      unipro_node.CreateUint("device_granularity", device_granularity.value());

  return zx::ok();
}

zx::result<> DeviceManager::InitUicPowerMode(inspect::Node &unipro_node) {
  if (zx::result<> result = controller_.Notify(NotifyEvent::kPrePowerModeChange, 0);
      result.is_error()) {
    return result.take_error();
  }

  // Update lanes with available TX/RX lanes.
  zx::result<uint32_t> tx_lanes = DmeGet(PA_AvailTxDataLanes);
  if (tx_lanes.is_error()) {
    return tx_lanes.take_error();
  }
  zx::result<uint32_t> rx_lanes = DmeGet(PA_AvailRxDataLanes);
  if (rx_lanes.is_error()) {
    return rx_lanes.take_error();
  }
  // Get max HS-GEAR.
  zx::result<uint32_t> max_rx_hs_gear = DmeGet(PA_MaxRxHSGear);
  if (max_rx_hs_gear.is_error()) {
    return max_rx_hs_gear.take_error();
  }

  // Set data lanes.
  if (zx::result<> result = DmeSet(PA_ActiveTxDataLanes, tx_lanes.value()); result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = DmeSet(PA_ActiveRxDataLanes, rx_lanes.value()); result.is_error()) {
    return result.take_error();
  }

  // Set HS-GEAR to max gear.
  if (zx::result<> result = DmeSet(PA_TxGear, max_rx_hs_gear.value()); result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = DmeSet(PA_RxGear, max_rx_hs_gear.value()); result.is_error()) {
    return result.take_error();
  }

  // Set termination.
  // HS-MODE = ON / LS-MODE = OFF
  if (zx::result<> result = DmeSet(PA_TxTermination, true); result.is_error()) {
    return result.take_error();
  }

  // HS-MODE = ON / LS-MODE = OFF
  if (zx::result<> result = DmeSet(PA_RxTermination, true); result.is_error()) {
    return result.take_error();
  }

  // Set HSSerise (A = 1, B = 2)
  constexpr uint32_t kHsSeries = 2;
  if (zx::result<> result = DmeSet(PA_HSSeries, kHsSeries); result.is_error()) {
    return result.take_error();
  }

  // Set Timeout values.
  if (zx::result<> result = DmeSet(PA_PWRModeUserData0, DL_FC0ProtectionTimeOutVal_Default);
      result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = DmeSet(PA_PWRModeUserData1, DL_TC0ReplayTimeOutVal_Default);
      result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = DmeSet(PA_PWRModeUserData2, DL_AFC0ReqTimeOutVal_Default);
      result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = DmeSet(PA_PWRModeUserData3, DL_FC0ProtectionTimeOutVal_Default);
      result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = DmeSet(PA_PWRModeUserData4, DL_TC0ReplayTimeOutVal_Default);
      result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = DmeSet(PA_PWRModeUserData5, DL_AFC0ReqTimeOutVal_Default);
      result.is_error()) {
    return result.take_error();
  }

  if (zx::result<> result =
          DmeSet(DME_LocalFC0ProtectionTimeOutVal, DL_FC0ProtectionTimeOutVal_Default);
      result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = DmeSet(DME_LocalTC0ReplayTimeOutVal, DL_TC0ReplayTimeOutVal_Default);
      result.is_error()) {
    return result.take_error();
  }
  if (zx::result<> result = DmeSet(DME_LocalAFC0ReqTimeOutVal, DL_AFC0ReqTimeOutVal_Default);
      result.is_error()) {
    return result.take_error();
  }

  // Set TX/RX PWRMode.
  // TX[3:0], RX[7:4]
  // Fast_Mode=1, Slow_Mode=2, FastAuto_Mode=4, SlowAuto_Mode=5
  constexpr uint32_t kFastMode = 1;
  constexpr uint32_t kRxBitShift = 4;
  constexpr uint32_t kPwrMode = kFastMode << kRxBitShift | kFastMode;
  if (zx::result<> result = DmeSet(PA_PWRMode, kPwrMode); result.is_error()) {
    return result.take_error();
  }

  // Wait for power mode changed.
  auto wait_for_completion = [&]() -> bool {
    return InterruptStatusReg::Get().ReadFrom(&controller_.GetMmio()).uic_power_mode_status();
  };
  fbl::String timeout_message = "Timeout waiting for Power Mode Change";
  if (zx_status_t status = controller_.WaitWithTimeout(
          wait_for_completion, zx::usec(kDeviceInitTimeoutUs), timeout_message);
      status != ZX_OK) {
    return zx::error(status);
  }
  // Clear 'Power Mode completion status'
  InterruptStatusReg::Get().FromValue(0).set_uic_power_mode_status(true).WriteTo(
      &controller_.GetMmio());

  HostControllerStatusReg::PowerModeStatus power_mode_status =
      HostControllerStatusReg::Get()
          .ReadFrom(&controller_.GetMmio())
          .uic_power_mode_change_request_status();
  if (power_mode_status != HostControllerStatusReg::PowerModeStatus::kPowerLocal) {
    fdf::error("Failed to change power mode: power_mode_status = 0x{:x}",
               static_cast<uint32_t>(power_mode_status));
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (zx::result<> result = controller_.Notify(NotifyEvent::kPostPowerModeChange, 0);
      result.is_error()) {
    return result.take_error();
  }

  if (controller_.intel_quirk()) {
    //  Intel Lake-field UFSHCI has a quirk. We need to wait 1250us and clear dme error.
    usleep(1250);
    // Test with dme_peer_get to make sure there are no errors.
    zx::result<uint32_t> device_granularity = DmePeerGet(PA_Granularity);
    if (device_granularity.is_error()) {
      return device_granularity.take_error();
    }
  }

  properties_.pa_active_tx_data_lanes =
      unipro_node.CreateUint("PA_ActiveTxDataLanes", tx_lanes.value());
  properties_.pa_active_rx_data_lanes =
      unipro_node.CreateUint("PA_ActiveRxDataLanes", rx_lanes.value());
  properties_.pa_max_rx_hs_gear = unipro_node.CreateUint("PA_MaxRxHSGear", max_rx_hs_gear.value());
  properties_.pa_tx_gear = unipro_node.CreateUint("PA_TxGear", max_rx_hs_gear.value());
  properties_.pa_rx_gear = unipro_node.CreateUint("PA_RxGear", max_rx_hs_gear.value());
  properties_.tx_termination = unipro_node.CreateBool("tx_termination", true);
  properties_.rx_termination = unipro_node.CreateBool("rx_termination", true);
  properties_.pa_hs_series = unipro_node.CreateUint("PA_HSSeries", kHsSeries);
  properties_.power_mode = unipro_node.CreateUint("power_mode", kPwrMode);

  return zx::ok();
}

zx::result<> DeviceManager::SetPowerCondition(scsi::PowerCondition target_power_condition) {
  if (current_power_condition_ == target_power_condition) {
    return zx::ok();
  }

  auto scsi_lun = Ufs::TranslateUfsLunToScsiLun(static_cast<uint8_t>(WellKnownLuns::kUfsDevice));
  if (scsi_lun.is_error()) {
    return scsi_lun.take_error();
  }

  // Send START STOP UNIT to change power mode
  zx_status_t status = controller_.StartStopUnit(kPlaceholderTarget, scsi_lun.value(),
                                                 /*immed*/ false, target_power_condition);
  if (status != ZX_OK) {
    fdf::error("Failed to send START STOP UNIT SCSI command: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  current_power_condition_ = target_power_condition;
  return zx::ok();
}

zx::result<> DeviceManager::SuspendPower() {
  const UfsPowerMode target_power_mode = UfsPowerMode::kSleep;
  const scsi::PowerCondition target_power_condition =
      GetPowerModeInfo(target_power_mode).power_condition;
  const LinkState target_link_state = GetPowerModeInfo(target_power_mode).link_state;

  std::lock_guard<std::mutex> lock(power_lock_);

  if (current_power_mode_ == target_power_mode &&
      current_power_condition_ == target_power_condition &&
      current_link_state_ == target_link_state) {
    return zx::ok();
  }

  if (current_power_mode_ != UfsPowerMode::kActive ||
      current_power_condition_ != scsi::PowerCondition::kActive ||
      current_link_state_ != LinkState::kActive) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // TODO(b/42075643): We need to wait for the in flight I/O.

  // TODO(b/42075643): If we turn off the power(vcc off) while LogicalLunPowerOnWriteProtect is
  // enabled, we will lose write protection. To avoid this, power should be maintained when write
  // protect is enabled. This requires more fine-grained power control(VCC, VCCQ, VCCQ2).

  // TODO(b/42075643): In the case of power suspended state, we can apply a policy to perform
  // background operations in the suspended state. Currently, background operations are not
  // performed when suspend.
  if (zx::result<> result = DisableBackgroundOp(); result.is_error()) {
    return result.take_error();
  }

  // We should check if WriteBooster Flush is needed. If so, we should postpone changing the power
  // mode.
  zx::result<bool> result = NeedWriteBoosterBufferFlush();
  if (result.is_error()) {
    return result.take_error();
  }
  if (result.value()) {
    // TODO(b/42075643): We need to keep the power mode active until the
    // Writebooster flush is complete.
    fdf::warn("WriteBooster buffer flush is needed");
    return zx::ok();
  }

  if (zx::result<> result = controller_.Notify(NotifyEvent::kPrePowerModeChange, 0);
      result.is_error()) {
    return result.take_error();
  }

  if (zx::result<> result = SetPowerCondition(target_power_condition); result.is_error()) {
    return result.take_error();
  }

  DmeHibernateEnterCommand dme_hibernate_enter_command(controller_);
  if (auto result = dme_hibernate_enter_command.SendCommand(); result.is_error()) {
    // TODO(b/42075643): Link has a problem and needs to perform error recovery.
    return result.take_error();
  }

  if (zx::result<> result = controller_.Notify(NotifyEvent::kPostPowerModeChange, 0);
      result.is_error()) {
    return result.take_error();
  }

  current_link_state_ = target_link_state;
  current_power_mode_ = target_power_mode;
  current_power_condition_ = target_power_condition;
  properties_.power_suspended.Set(true);

  fdf::info("Power suspended.");
  return zx::ok();
}

zx::result<> DeviceManager::ResumePower() {
  const UfsPowerMode target_power_mode = UfsPowerMode::kActive;
  const scsi::PowerCondition target_power_condition =
      GetPowerModeInfo(target_power_mode).power_condition;
  const LinkState target_link_state = GetPowerModeInfo(target_power_mode).link_state;

  std::lock_guard<std::mutex> lock(power_lock_);

  if (current_power_mode_ == target_power_mode &&
      current_power_condition_ == target_power_condition &&
      current_link_state_ == target_link_state) {
    return zx::ok();
  }

  if (current_power_mode_ != UfsPowerMode::kSleep ||
      current_power_condition_ != scsi::PowerCondition::kIdle ||
      current_link_state_ != LinkState::kHibernate) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (zx::result<> result = controller_.Notify(NotifyEvent::kPrePowerModeChange, 0);
      result.is_error()) {
    return result.take_error();
  }

  DmeHibernateExitCommand dme_hibernate_exit_command(controller_);
  if (auto result = dme_hibernate_exit_command.SendCommand(); result.is_error()) {
    // TODO(https://fxbug.dev/42075643): Link has a problem and needs to perform error recovery.
    return result.take_error();
  }

  if (zx::result<> result = SetPowerCondition(target_power_condition); result.is_error()) {
    return result.take_error();
  }

  current_link_state_ = target_link_state;
  current_power_mode_ = target_power_mode;
  current_power_condition_ = target_power_condition;
  properties_.power_suspended.Set(false);

  if (zx::result<> result = controller_.Notify(NotifyEvent::kPostPowerModeChange, 0);
      result.is_error()) {
    return result.take_error();
  }

  // TODO(b/42075643): We should only perform background operations in the power suspended state.
  if (zx::result<> result = EnableBackgroundOp(); result.is_error()) {
    return result.take_error();
  }

  fdf::info("Power resumed.");
  return zx::ok();
}

zx::result<> DeviceManager::InitUfsPowerMode(inspect::Node &controller_node,
                                             inspect::Node &attributes_node) {
  // NOTE: ReadAttribute and WriteAttribute issue synchronous query UPIU commands across the bus.
  // We MUST execute bus I/O outside power_lock_ to avoid deadlocking with concurrent IO/dispatcher
  // threads (e.g. ProcessIo) that acquire power_lock_ while checking power state.
  // Thread safety and serialization for bus I/O are guaranteed by admin_slot_lock_ in
  // TransferRequestProcessor, so executing attribute queries outside power_lock_ carries no risk
  // of conflicting I/O.
  zx::result<uint32_t> power_mode = ReadAttribute(Attributes::bCurrentPowerMode);
  if (power_mode.is_error()) {
    return power_mode.take_error();
  }
  UfsPowerMode target_power_mode = static_cast<UfsPowerMode>(power_mode.value());
  if (target_power_mode != UfsPowerMode::kActive) {
    fdf::error("Initial power mode is not active: 0x{:x}", static_cast<uint8_t>(target_power_mode));
    return zx::error(ZX_ERR_BAD_STATE);
  }
  fdf::debug("bCurrentPowerMode 0x{:x}", power_mode.value());

  // Write active ICC level outside power_lock_ for the same anti-deadlock reason.
  // TODO(https://fxbug.dev/42075643): Calculate and set the maximum ICC level. Currently, this
  // value is temporarily set to 0x0F, which is the highest active ICC level.
  if (auto result = WriteAttribute(Attributes::bActiveIccLevel, kHighestActiveIcclevel);
      result.is_error()) {
    return result.take_error();
  }

  // Acquire power_lock_ only for the immediate, non-blocking in-memory state update.
  std::lock_guard<std::mutex> lock(power_lock_);
  current_power_mode_ = target_power_mode;
  current_power_condition_ = GetPowerModeInfo(current_power_mode_).power_condition;
  current_link_state_ = GetPowerModeInfo(current_power_mode_).link_state;

  // TODO(https://fxbug.dev/42075643): Enable auto hibernate

  properties_.b_current_power_mode =
      attributes_node.CreateUint("bCurrentPowerMode", static_cast<uint8_t>(current_power_mode_));
  properties_.b_active_icc_level =
      attributes_node.CreateUint("bActiveICCLevel", kHighestActiveIcclevel);
  properties_.power_condition =
      controller_node.CreateUint("PowerCondition", static_cast<uint8_t>(current_power_condition_));
  properties_.link_state =
      controller_node.CreateUint("LinkState", static_cast<uint8_t>(current_link_state_));

  return zx::ok();
}

}  // namespace ufs
