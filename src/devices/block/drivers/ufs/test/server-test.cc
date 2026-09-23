// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.rpmb/cpp/wire.h>
#include <fidl/fuchsia.hardware.ufs/cpp/common_types.h>
#include <fidl/fuchsia.hardware.ufs/cpp/markers.h>
#include <fidl/fuchsia.hardware.ufs/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.ufs/cpp/wire_types.h>
#include <fidl/fuchsia.mem/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/fidl/cpp/wire/wire_types.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <zircon/errors.h>

#include <cstdint>

#include <gtest/gtest.h>

#include "src/devices/block/drivers/ufs/device_manager.h"
#include "src/devices/block/drivers/ufs/upiu/attributes.h"
#include "src/devices/block/drivers/ufs/upiu/descriptors.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"
#include "unit-lib.h"

namespace ufs {
using namespace ufs_mock_device;

class ServerTest : public UfsTest {
 public:
  fidl::ClientEnd<fuchsia_hardware_ufs::Ufs> GetClient() {
    zx::result device = driver_test().Connect<fuchsia_hardware_ufs::Service::Device>();
    EXPECT_EQ(ZX_OK, device.status_value());
    return std::move(device.value());
  }

  fdf::Arena arena_{'test'};
};

TEST_F(ServerTest, ReadDescriptor) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        auto desc = fuchsia_hardware_ufs::wire::Descriptor::Builder(arena)
                        .type(fuchsia_hardware_ufs::DescriptorType::kDevice)
                        .Build();

        const fidl::WireResult result = fidl::WireCall(client_end)->ReadDescriptor(desc);
        ASSERT_TRUE(result.ok());
        const fit::result response = result.value();
        ASSERT_TRUE(response.is_ok());

        DeviceDescriptor descriptor;
        std::memset(&descriptor, 0, sizeof(DeviceDescriptor));
        auto response_data = response->data;
        std::memcpy(&descriptor, response_data.data(), sizeof(DeviceDescriptor));

        auto mock_desc = mock_device.GetDeviceDesc();
        ASSERT_EQ(descriptor.bLength, mock_desc.bLength);
        ASSERT_EQ(descriptor.bDescriptorIDN, mock_desc.bDescriptorIDN);
        ASSERT_EQ(descriptor.bDeviceSubClass, mock_desc.bDeviceSubClass);
        ASSERT_EQ(descriptor.bNumberWLU, mock_desc.bNumberWLU);
        ASSERT_EQ(descriptor.bInitPowerMode, mock_desc.bInitPowerMode);
        ASSERT_EQ(descriptor.bHighPriorityLUN, mock_desc.bHighPriorityLUN);
        ASSERT_EQ(descriptor.wSpecVersion, mock_desc.wSpecVersion);
        ASSERT_EQ(descriptor.bUD0BaseOffset, mock_desc.bUD0BaseOffset);
        ASSERT_EQ(descriptor.bUDConfigPLength, mock_desc.bUDConfigPLength);
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, WriteDescriptorRejectsConfigurationDescriptor) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient(),
                                                                   &mock_device = mock_device_]() {
    uint8_t initial_priority = mock_device.GetDeviceDesc().bHighPriorityLUN;
    ConfigurationDescriptor descriptor;
    std::memset(&descriptor, 0, sizeof(ConfigurationDescriptor));
    descriptor.bLength = 0xE6;
    descriptor.bDescriptorIDN = 0x01;
    descriptor.bConfDescContinue = 0x00;
    descriptor.bHighPriorityLUN = initial_priority == 0 ? 1 : 0;

    fidl::Arena arena;
    auto desc = fuchsia_hardware_ufs::wire::Descriptor::Builder(arena)
                    .type(fuchsia_hardware_ufs::DescriptorType::kConfiguration)
                    .Build();
    std::vector<uint8_t> data_segment(sizeof(ConfigurationDescriptor));
    std::memcpy(data_segment.data(), &descriptor, sizeof(ConfigurationDescriptor));

    const fidl::WireResult result =
        fidl::WireCall(client_end)
            ->WriteDescriptor(desc, fidl::VectorView<uint8_t>(arena, data_segment));
    ASSERT_TRUE(result.ok());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_error());
    ASSERT_EQ(response.error_value(), fuchsia_hardware_ufs::QueryErrorCode::kParameterNotWriteable);
    EXPECT_EQ(initial_priority, mock_device.GetDeviceDesc().bHighPriorityLUN);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, WriteDescriptorBufferTooShort) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    fidl::Arena arena;
    auto desc = fuchsia_hardware_ufs::wire::Descriptor::Builder(arena)
                    .type(fuchsia_hardware_ufs::DescriptorType::kDevice)
                    .Build();
    // Create a buffer significantly smaller than DeviceDescriptor.
    std::vector<uint8_t> short_data_segment(10, 0);

    const fidl::WireResult result =
        fidl::WireCall(client_end)
            ->WriteDescriptor(desc, fidl::VectorView<uint8_t>(arena, short_data_segment));
    ASSERT_TRUE(result.ok());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_error());
    ASSERT_EQ(response.error_value(), fuchsia_hardware_ufs::QueryErrorCode::kGeneralFailure);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ReadFlag) {
  bool device_init = false;
  mock_device_.SetFlag(Flags::fDeviceInit, device_init);
  auto result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        auto flag = fuchsia_hardware_ufs::wire::Flag::Builder(arena)
                        .type(fuchsia_hardware_ufs::FlagType::kDeviceInit)
                        .Build();

        const fidl::WireResult result = fidl::WireCall(client_end)->ReadFlag(flag);
        ASSERT_TRUE(result.ok());
        const fit::result response = result.value();
        ASSERT_TRUE(response.is_ok());
        ASSERT_FALSE(response->value);
        ASSERT_EQ(response->value, mock_device.GetFlag(Flags::fDeviceInit));
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, SetFlag) {
  bool refresh_enable = false;
  mock_device_.SetFlag(Flags::fRefreshEnable, refresh_enable);
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        auto flag = fuchsia_hardware_ufs::wire::Flag::Builder(arena)
                        .type(fuchsia_hardware_ufs::FlagType::kRefreshEnable)
                        .Build();

        const fidl::WireResult result = fidl::WireCall(client_end)->SetFlag(flag);
        ASSERT_TRUE(result.ok());
        const fit::result response = result.value();
        ASSERT_TRUE(response.is_ok());
        ASSERT_TRUE(response->value);
        ASSERT_EQ(response->value, mock_device.GetFlag(Flags::fRefreshEnable));
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, SetFlagRejectsWriteOnceFlags) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        for (auto flag_type : {fuchsia_hardware_ufs::FlagType::kPermanentWpEn,
                               fuchsia_hardware_ufs::FlagType::kPermanentlyDisableFwUpdate,
                               fuchsia_hardware_ufs::FlagType::kPhyResourceRemoval}) {
          auto flag = fuchsia_hardware_ufs::wire::Flag::Builder(arena).type(flag_type).Build();

          const fidl::WireResult result = fidl::WireCall(client_end)->SetFlag(flag);
          ASSERT_TRUE(result.ok());
          const fit::result response = result.value();
          ASSERT_TRUE(response.is_error());
          ASSERT_EQ(response.error_value(),
                    fuchsia_hardware_ufs::QueryErrorCode::kParameterNotWriteable);
        }
        ASSERT_FALSE(mock_device.GetFlag(Flags::fPermanentWPEn));
        ASSERT_FALSE(mock_device.GetFlag(Flags::fPermanentlyDisableFwUpdate));
        ASSERT_FALSE(mock_device.GetFlag(Flags::fPhyResourceRemoval));
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ToggleFlagRejectsWriteOnceFlags) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        for (auto flag_type : {fuchsia_hardware_ufs::FlagType::kPermanentWpEn,
                               fuchsia_hardware_ufs::FlagType::kPermanentlyDisableFwUpdate,
                               fuchsia_hardware_ufs::FlagType::kPhyResourceRemoval}) {
          auto flag = fuchsia_hardware_ufs::wire::Flag::Builder(arena).type(flag_type).Build();

          const fidl::WireResult result = fidl::WireCall(client_end)->ToggleFlag(flag);
          ASSERT_TRUE(result.ok());
          const fit::result response = result.value();
          ASSERT_TRUE(response.is_error());
          ASSERT_EQ(response.error_value(),
                    fuchsia_hardware_ufs::QueryErrorCode::kParameterNotWriteable);
        }
        ASSERT_FALSE(mock_device.GetFlag(Flags::fPermanentWPEn));
        ASSERT_FALSE(mock_device.GetFlag(Flags::fPermanentlyDisableFwUpdate));
        ASSERT_FALSE(mock_device.GetFlag(Flags::fPhyResourceRemoval));
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ClearFlagRejectsWriteOnceFlags) {
  mock_device_.SetFlag(Flags::fPermanentWPEn, true);
  mock_device_.SetFlag(Flags::fPermanentlyDisableFwUpdate, true);
  mock_device_.SetFlag(Flags::fPhyResourceRemoval, true);
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        for (auto flag_type : {fuchsia_hardware_ufs::FlagType::kPermanentWpEn,
                               fuchsia_hardware_ufs::FlagType::kPermanentlyDisableFwUpdate,
                               fuchsia_hardware_ufs::FlagType::kPhyResourceRemoval}) {
          auto flag = fuchsia_hardware_ufs::wire::Flag::Builder(arena).type(flag_type).Build();

          const fidl::WireResult result = fidl::WireCall(client_end)->ClearFlag(flag);
          ASSERT_TRUE(result.ok());
          const fit::result response = result.value();
          ASSERT_TRUE(response.is_error());
          ASSERT_EQ(response.error_value(),
                    fuchsia_hardware_ufs::QueryErrorCode::kParameterNotWriteable);
        }
        ASSERT_TRUE(mock_device.GetFlag(Flags::fPermanentWPEn));
        ASSERT_TRUE(mock_device.GetFlag(Flags::fPermanentlyDisableFwUpdate));
        ASSERT_TRUE(mock_device.GetFlag(Flags::fPhyResourceRemoval));
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ToggleFlag) {
  bool refresh_enable = true;
  mock_device_.SetFlag(Flags::fRefreshEnable, refresh_enable);
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        auto flag = fuchsia_hardware_ufs::wire::Flag::Builder(arena)
                        .type(fuchsia_hardware_ufs::FlagType::kRefreshEnable)
                        .Build();

        const fidl::WireResult result = fidl::WireCall(client_end)->ToggleFlag(flag);
        ASSERT_TRUE(result.ok());
        const fit::result response = result.value();
        ASSERT_TRUE(response.is_ok());
        ASSERT_FALSE(response->value);
        ASSERT_EQ(response->value, mock_device.GetFlag(Flags::fRefreshEnable));
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ClearFlag) {
  bool refresh_enable = true;
  mock_device_.SetFlag(Flags::fRefreshEnable, refresh_enable);
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        auto flag = fuchsia_hardware_ufs::wire::Flag::Builder(arena)
                        .type(fuchsia_hardware_ufs::FlagType::kRefreshEnable)
                        .Build();

        const fidl::WireResult result = fidl::WireCall(client_end)->ClearFlag(flag);
        ASSERT_TRUE(result.ok());
        const fit::result response = result.value();
        ASSERT_TRUE(response.is_ok());
        ASSERT_FALSE(response->value);
        ASSERT_EQ(response->value, mock_device.GetFlag(Flags::fRefreshEnable));
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ReadAttribute) {
  mock_device_.SetAttribute(Attributes::bCurrentPowerMode,
                            static_cast<uint32_t>(UfsPowerMode::kActive));
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        auto attr = fuchsia_hardware_ufs::wire::Attribute::Builder(arena)
                        .type(fuchsia_hardware_ufs::AttributeType::kCurrentPowerMode)
                        .Build();
        const fidl::WireResult result = fidl::WireCall(client_end)->ReadAttribute(attr);
        ASSERT_TRUE(result.ok());
        const fit::result response = result.value();
        ASSERT_TRUE(response.is_ok());
        ASSERT_EQ(response->value, mock_device.GetAttribute(Attributes::bCurrentPowerMode));
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, WriteAttribute) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient(),
                                                                   &mock_device = mock_device_]() {
    fidl::Arena arena;
    auto attr = fuchsia_hardware_ufs::wire::Attribute::Builder(arena)
                    .type(fuchsia_hardware_ufs::AttributeType::kBootLunEn)
                    .Build();

    uint32_t changed_value = 0x01;
    const fidl::WireResult result = fidl::WireCall(client_end)->WriteAttribute(attr, changed_value);
    ASSERT_TRUE(result.ok());

    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    ASSERT_EQ(changed_value, mock_device.GetAttribute(Attributes::bBootLunEn));
  });

  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, WriteAttributeRejectsWriteOnceAttributes) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        fidl::Arena arena;
        for (auto attr_type : {fuchsia_hardware_ufs::AttributeType::kConfigDescrLock,
                               fuchsia_hardware_ufs::AttributeType::kPsaState}) {
          auto attr = fuchsia_hardware_ufs::wire::Attribute::Builder(arena).type(attr_type).Build();

          const fidl::WireResult result = fidl::WireCall(client_end)->WriteAttribute(attr, 1u);
          ASSERT_TRUE(result.ok());
          const fit::result response = result.value();
          ASSERT_TRUE(response.is_error());
          ASSERT_EQ(response.error_value(),
                    fuchsia_hardware_ufs::QueryErrorCode::kParameterNotWriteable);
        }
        ASSERT_EQ(0u, mock_device.GetAttribute(Attributes::bConfigDescrLock));
        ASSERT_EQ(0u, mock_device.GetAttribute(Attributes::bPSAState));
      });

  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, WriteAttributeRejectsUnapprovedAttributes) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient(),
                                                                   &mock_device = mock_device_]() {
    fidl::Arena arena;
    auto attr = fuchsia_hardware_ufs::wire::Attribute::Builder(arena)
                    .type(fuchsia_hardware_ufs::AttributeType::kActiveIccLevel)
                    .Build();

    const fidl::WireResult result = fidl::WireCall(client_end)->WriteAttribute(attr, 1u);
    ASSERT_TRUE(result.ok());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_error());
    ASSERT_EQ(response.error_value(), fuchsia_hardware_ufs::QueryErrorCode::kParameterNotWriteable);
    ASSERT_EQ(static_cast<uint32_t>(kHighestActiveIcclevel),
              mock_device.GetAttribute(Attributes::bActiveIccLevel));
  });

  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, UicCommandDmeGet) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    auto uic_arg_attr_sel = [](uint16_t attr, uint16_t sel) {
      return (static_cast<uint32_t>(attr) << 16) | static_cast<uint32_t>(sel);
    };

    fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{};
    args[0] = uic_arg_attr_sel(PA_MaxRxHSGear, 0);
    const fidl::WireResult result =
        fidl::WireCall(client_end)
            ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmeGet, args);
    ASSERT_TRUE(result.ok());

    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    ASSERT_EQ(response->result, ufs_mock_device::kMaxGear);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, UicCommandDmeSet) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    auto uic_arg_attr_sel = [](uint16_t attr, uint16_t sel) {
      return (static_cast<uint32_t>(attr) << 16) | static_cast<uint32_t>(sel);
    };

    fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{};
    args[0] = uic_arg_attr_sel(PA_Granularity, 0);
    const fidl::WireResult result =
        fidl::WireCall(client_end)
            ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmeSet, args);
    ASSERT_TRUE(result.ok());

    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    ASSERT_EQ(response->result, 0U);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, UicCommandDmePeerGet) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    auto uic_arg_attr_sel = [](uint16_t attr, uint16_t sel) {
      return (static_cast<uint32_t>(attr) << 16) | static_cast<uint32_t>(sel);
    };

    fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{};
    args[0] = uic_arg_attr_sel(PA_TActivate, 0);
    const fidl::WireResult result =
        fidl::WireCall(client_end)
            ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmePeerGet, args);
    ASSERT_TRUE(result.ok());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    ASSERT_EQ(response->result, ufs_mock_device::kTActivate);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, UicCommandDmePeerSet) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    auto uic_arg_attr_sel = [](uint16_t attr, uint16_t sel) {
      return (static_cast<uint32_t>(attr) << 16) | static_cast<uint32_t>(sel);
    };

    fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{};
    args[0] = uic_arg_attr_sel(PA_TActivate, 0);
    const fidl::WireResult result =
        fidl::WireCall(client_end)
            ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmePeerSet, args);
    ASSERT_TRUE(result.ok());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    ASSERT_EQ(response->result, 0U);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, SendUicCommandRejectsNonAllowlistedAttribute) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    auto uic_arg_attr_sel = [](uint16_t attr, uint16_t sel) {
      return (static_cast<uint32_t>(attr) << 16) | static_cast<uint32_t>(sel);
    };

    // DME_SET with non-allowlisted attribute (PA_MaxRxHSGear = 0x1587 or 0xD0FE)
    {
      fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{};
      args[0] = uic_arg_attr_sel(0xD0FE, 0);
      const fidl::WireResult r =
          fidl::WireCall(client_end)
              ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmeSet, args);
      ASSERT_TRUE(r.ok());
      ASSERT_TRUE(r.value().is_error());
      EXPECT_EQ(r.value().error_value(), ZX_ERR_NOT_SUPPORTED);
    }

    // DME_PEER_SET with non-allowlisted attribute
    {
      fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{};
      args[0] = uic_arg_attr_sel(PA_MaxRxHSGear, 0);
      const fidl::WireResult r =
          fidl::WireCall(client_end)
              ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmePeerSet, args);
      ASSERT_TRUE(r.ok());
      ASSERT_TRUE(r.value().is_error());
      EXPECT_EQ(r.value().error_value(), ZX_ERR_NOT_SUPPORTED);
    }
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, SendUicCommandRejectsNonVolatileAttrSetType) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    auto uic_arg_attr_sel = [](uint16_t attr, uint16_t sel) {
      return (static_cast<uint32_t>(attr) << 16) | static_cast<uint32_t>(sel);
    };
    auto uic_arg_attr_set_type = [](uint8_t set_type) {
      return static_cast<uint32_t>(set_type) << 16;
    };

    // DME_SET with static attr_set_type (1)
    {
      fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{};
      args[0] = uic_arg_attr_sel(PA_Granularity, 0);
      args[1] = uic_arg_attr_set_type(static_cast<uint8_t>(AttrSetType::kStatic));
      const fidl::WireResult r =
          fidl::WireCall(client_end)
              ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmeSet, args);
      ASSERT_TRUE(r.ok());
      ASSERT_TRUE(r.value().is_error());
      EXPECT_EQ(r.value().error_value(), ZX_ERR_NOT_SUPPORTED);
    }

    // DME_SET with invalid attr_set_type (0xFF)
    {
      fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{};
      args[0] = uic_arg_attr_sel(PA_Granularity, 0);
      args[1] = uic_arg_attr_set_type(0xFF);
      const fidl::WireResult r =
          fidl::WireCall(client_end)
              ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmeSet, args);
      ASSERT_TRUE(r.ok());
      ASSERT_TRUE(r.value().is_error());
      EXPECT_EQ(r.value().error_value(), ZX_ERR_NOT_SUPPORTED);
    }

    // DME_PEER_SET with static attr_set_type (1)
    {
      fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{};
      args[0] = uic_arg_attr_sel(PA_TActivate, 0);
      args[1] = uic_arg_attr_set_type(static_cast<uint8_t>(AttrSetType::kStatic));
      const fidl::WireResult r =
          fidl::WireCall(client_end)
              ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmePeerSet, args);
      ASSERT_TRUE(r.ok());
      ASSERT_TRUE(r.value().is_error());
      EXPECT_EQ(r.value().error_value(), ZX_ERR_NOT_SUPPORTED);
    }
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ReadDescriptorWithInvalidIdn) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    // Checks if an error occurs when the request type is missing.
    fidl::Arena arena;
    auto desc = fuchsia_hardware_ufs::wire::Descriptor::Builder(arena).Build();
    const fidl::WireResult result = fidl::WireCall(client_end)->ReadDescriptor(desc);
    ASSERT_TRUE(result.ok());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_error());
    ASSERT_EQ(response.error_value(), fuchsia_hardware_ufs::QueryErrorCode::kInvalidIdn);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, RequestInternalFail) {
  // Reserve admin slot.
  ASSERT_OK(ReserveAdminSlot());
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    fidl::Arena arena;
    auto attr = fuchsia_hardware_ufs::wire::Attribute::Builder(arena)
                    .type(fuchsia_hardware_ufs::AttributeType::kCurrentPowerMode)
                    .Build();
    const fidl::WireResult result = fidl::WireCall(client_end)->ReadAttribute(attr);
    ASSERT_TRUE(result.ok());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_error());
    ASSERT_EQ(response.error_value(), fuchsia_hardware_ufs::QueryErrorCode::kGeneralFailure);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, SendUicCommandWithUnsupportedOpcode) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    fidl::Array<uint32_t, fuchsia_hardware_ufs::kUicCommandArgumentCount> args{0};
    const fidl::WireResult result =
        fidl::WireCall(client_end)
            ->SendUicCommand(fuchsia_hardware_ufs::UicCommandOpcode::kDmeHiberEnter, args);
    ASSERT_TRUE(result.ok());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_error());
    ASSERT_EQ(response.error_value(), ZX_ERR_NOT_SUPPORTED);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ReadBuffer) {
  const uint8_t kTestLun = 0;
  uint32_t offset = 0;

  // Write test data to the mock device buffer
  char buf[kMockBlockSize];
  std::memset(buf, 0xf0, sizeof(buf));
  mock_device_.WriteToDeviceBuffer(kTestLun, buf, offset, sizeof(buf));

  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        uint32_t buffer_offset = 1024;
        uint32_t length = 256;

        fzl::OwnedVmoMapper mapper;
        ASSERT_OK(mapper.CreateAndMap(zx_system_get_page_size(), "read-buffer-test-vmo"));
        zx::vmo dup;
        ASSERT_OK(mapper.vmo().duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
        const fidl::WireResult result =
            fidl::WireCall(client_end)
                ->ReadBuffer(0, fuchsia_hardware_scsi::wire::ReadBufferMode::kData, 0,
                             buffer_offset, length, std::move(dup));
        ASSERT_TRUE(result.ok());
        const fit::result response = result.value();
        ASSERT_TRUE(response.is_ok());

        // Reads data from mock_device and compares it to the mapper.
        char buf[kMockBlockSize];
        mock_device.ReadFromDeviceBuffer(0, buf, buffer_offset, length);
        ASSERT_EQ(memcmp(mapper.start(), buf, length), 0);
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, WriteBuffer) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = GetClient(), &mock_device = mock_device_]() {
        uint32_t buffer_offset = 0;
        uint32_t length = 1024;
        zx::vmo vmo;
        ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

        char write_buf[kMockBlockSize];
        std::memset(write_buf, 0xf0, sizeof(write_buf));
        vmo.write(&write_buf, 0, length);

        const fidl::WireResult result =
            fidl::WireCall(client_end)
                ->WriteBuffer(0, fuchsia_hardware_scsi::wire::WriteBufferMode::kData, 0,
                              buffer_offset, length, std::move(vmo));
        ASSERT_TRUE(result.ok());
        const fit::result response = result.value();
        ASSERT_TRUE(response.is_ok());

        char buf[kMockBlockSize];
        mock_device.ReadFromDeviceBuffer(0, buf, buffer_offset, length);
        ASSERT_EQ(memcmp(&write_buf, buf, length), 0);
      });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, WriteBufferRejectsFfuModes) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    using WriteBufferMode = fuchsia_hardware_scsi::wire::WriteBufferMode;
    uint32_t length = 1024;
    for (WriteBufferMode mode :
         {WriteBufferMode::kVendorSpecific, WriteBufferMode::kFfuOffsetSaveDefer,
          WriteBufferMode::kErrorHistory}) {
      zx::vmo vmo;
      ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
      zx::vmo dup;
      ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

      const fidl::WireResult result =
          fidl::WireCall(client_end)->WriteBuffer(0, mode, 0, 0, length, std::move(dup));
      ASSERT_TRUE(result.ok());
      const fit::result response = result.value();
      ASSERT_TRUE(response.is_error());
      ASSERT_EQ(response.error_value(), ZX_ERR_ACCESS_DENIED);
    }
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ReadWirteBufferOutOfRange) {
  const uint8_t kTestLun = 0;
  uint32_t offset = 0;

  // Write test data to the mock device buffer
  char buf[kMockBlockSize];
  std::memset(buf, 0xf0, sizeof(buf));
  mock_device_.WriteToDeviceBuffer(kTestLun, buf, offset, sizeof(buf));

  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    uint32_t buffer_offset = kMaxDeviceBufferSize;
    uint32_t length = 256;

    // Read buffer
    {
      fzl::OwnedVmoMapper mapper;
      ASSERT_OK(mapper.CreateAndMap(zx_system_get_page_size(), "read-buffer-test-vmo"));
      zx::vmo dup;
      ASSERT_OK(mapper.vmo().duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
      const fidl::WireResult result =
          fidl::WireCall(client_end)
              ->ReadBuffer(0, fuchsia_hardware_scsi::wire::ReadBufferMode::kData, 0, buffer_offset,
                           length, std::move(dup));
      ASSERT_TRUE(result.ok());
      const fit::result response = result.value();
      ASSERT_TRUE(response.is_error());
      ASSERT_EQ(response.error_value(), ZX_ERR_BAD_STATE);
    }

    // Write buffer
    {
      zx::vmo vmo;
      ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

      char write_buf[kMockBlockSize];
      std::memset(write_buf, 0xf0, sizeof(write_buf));
      vmo.write(&write_buf, 0, length);

      const fidl::WireResult result =
          fidl::WireCall(client_end)
              ->WriteBuffer(0, fuchsia_hardware_scsi::wire::WriteBufferMode::kData, 0,
                            buffer_offset, length, std::move(vmo));
      ASSERT_TRUE(result.ok());
      const fit::result response = result.value();
      ASSERT_TRUE(response.is_error());
      ASSERT_EQ(response.error_value(), ZX_ERR_BAD_STATE);
    }
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ReadWriteBufferInvalidVmoSize) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    uint32_t buffer_offset = 0;
    uint32_t length = kMockBlockSize * 2;

    // Read buffer with invalid vmo size
    {
      fzl::OwnedVmoMapper mapper;
      ASSERT_OK(mapper.CreateAndMap(kMockBlockSize, "read-buffer-test-vmo"));
      zx::vmo dup;
      ASSERT_OK(mapper.vmo().duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
      const fidl::WireResult result =
          fidl::WireCall(client_end)
              ->ReadBuffer(0, fuchsia_hardware_scsi::wire::ReadBufferMode::kData, 0, buffer_offset,
                           length, std::move(dup));
      ASSERT_TRUE(result.ok());
      const fit::result response = result.value();
      ASSERT_EQ(response.error_value(), ZX_ERR_OUT_OF_RANGE);
    }

    // Write buffer with invalid vmo size
    {
      zx::vmo vmo;
      ASSERT_OK(zx::vmo::create(kMockBlockSize, 0, &vmo));

      char write_buf[kMockBlockSize * 2];
      std::memset(write_buf, 0xf0, sizeof(write_buf));
      vmo.write(&write_buf, 0, length);

      const fidl::WireResult result =
          fidl::WireCall(client_end)
              ->WriteBuffer(0, fuchsia_hardware_scsi::wire::WriteBufferMode::kData, 0,
                            buffer_offset, length, std::move(vmo));
      ASSERT_TRUE(result.ok());
      const fit::result response = result.value();
      ASSERT_EQ(response.error_value(), ZX_ERR_OUT_OF_RANGE);
    }
  });
  ASSERT_OK(result.status_value());
}

TEST_F(ServerTest, ReadWriteBufferWithInvalidLun) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    uint32_t buffer_offset = 0;
    uint32_t length = kMockBlockSize;
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(kMockBlockSize, 0, &vmo));

    const uint8_t kTestLun = 1;
    // Request to write buffer with invalid lun
    {
      char write_buf[kMockBlockSize];
      std::memset(write_buf, 0xf0, sizeof(write_buf));
      vmo.write(&write_buf, 0, length);

      const fidl::WireResult result =
          fidl::WireCall(client_end)
              ->WriteBuffer(kTestLun, fuchsia_hardware_scsi::wire::WriteBufferMode::kData, 0,
                            buffer_offset, length, std::move(vmo));
      ASSERT_TRUE(result.ok());
      const fit::result response = result.value();
      ASSERT_EQ(response.error_value(), ZX_ERR_BAD_STATE);
    }

    // Request to read buffer with invalid lun
    {
      fzl::OwnedVmoMapper mapper;
      ASSERT_OK(mapper.CreateAndMap(zx_system_get_page_size(), "read-buffer-test-vmo"));
      zx::vmo dup;
      ASSERT_OK(mapper.vmo().duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
      const fidl::WireResult result =
          fidl::WireCall(client_end)
              ->ReadBuffer(kTestLun, fuchsia_hardware_scsi::wire::ReadBufferMode::kData, 0,
                           buffer_offset, length, std::move(dup));
      ASSERT_TRUE(result.ok());
      const fit::result response = result.value();
      ASSERT_EQ(response.error_value(), ZX_ERR_BAD_STATE);
    }
  });
  ASSERT_OK(result.status_value());
}

class UfsRpmbTest : public UfsTest {
 public:
  fidl::ClientEnd<fuchsia_hardware_rpmb::Rpmb> GetRpmbClient() {
    zx::result device = driver_test().Connect<fuchsia_hardware_rpmb::Service::Device>();
    EXPECT_EQ(ZX_OK, device.status_value());
    return std::move(device.value());
  }
};

TEST_F(UfsRpmbTest, RpmbGetDeviceInfo) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetRpmbClient()]() {
    const fidl::WireResult result = fidl::WireCall(client_end)->GetDeviceInfo();
    ASSERT_TRUE(result.ok());

    const fuchsia_hardware_rpmb::wire::DeviceInfo& device_info = result.value().info;
    ASSERT_TRUE(device_info.is_emmc_info());
    ASSERT_EQ(device_info.emmc_info().rpmb_size, 4);
    ASSERT_EQ(device_info.emmc_info().reliable_write_sector_count, 1);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(UfsRpmbTest, RpmbRequest) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetRpmbClient()]() {
    // Create VMOs for request
    zx::vmo tx_vmo;
    ASSERT_OK(zx::vmo::create(512, 0, &tx_vmo));
    std::vector<uint8_t> tx_data(512, 0x5A);
    ASSERT_OK(tx_vmo.write(tx_data.data(), 0, 512));

    zx::vmo rx_vmo;
    ASSERT_OK(zx::vmo::create(512, 0, &rx_vmo));

    zx::vmo rx_vmo_dup;
    ASSERT_OK(rx_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &rx_vmo_dup));

    fuchsia_mem::wire::Range rx_range = {
        .vmo = std::move(rx_vmo_dup),
        .offset = 0,
        .size = 512,
    };
    fuchsia_hardware_rpmb::wire::Request req{
        .tx_frames =
            fuchsia_mem::wire::Range{
                .vmo = std::move(tx_vmo),
                .offset = 0,
                .size = 512,
            },
        .rx_frames = fidl::ObjectView<fuchsia_mem::wire::Range>::FromExternal(&rx_range),
    };

    const fidl::WireResult result = fidl::WireCall(client_end)->Request(std::move(req));
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result.value().is_ok());

    // Verify response data is populated (DefaultSecurityProtocolInHandler returns 0xAB)
    std::vector<uint8_t> rx_data(512);
    ASSERT_OK(rx_vmo.read(rx_data.data(), 0, 512));
    for (uint8_t val : rx_data) {
      ASSERT_EQ(val, 0xAB);
    }
  });
  ASSERT_OK(result.status_value());
}

}  // namespace ufs
