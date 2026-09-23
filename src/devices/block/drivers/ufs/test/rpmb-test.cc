// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/ufs/rpmb.h"

#include <fidl/fuchsia.hardware.rpmb/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/scsi/controller.h>
#include <zircon/errors.h>

#include <cstdint>

#include <gtest/gtest.h>

#include "src/devices/block/drivers/ufs/transfer_request_descriptor.h"
#include "unit-lib.h"

namespace ufs {
using namespace ufs_mock_device;

class RpmbTest : public UfsTest {
 public:
  fidl::ClientEnd<fuchsia_hardware_rpmb::Rpmb> GetClient() {
    zx::result device = driver_test().Connect<fuchsia_hardware_rpmb::Service::Device>();
    EXPECT_EQ(ZX_OK, device.status_value());
    return std::move(device.value());
  }

  // Helper to create a VMO with data
  zx::vmo CreateVmo(size_t size, uint8_t fill_char) {
    zx::vmo vmo;
    EXPECT_OK(zx::vmo::create(size, 0, &vmo));
    if (size > 0) {
      std::vector<uint8_t> data(size, fill_char);
      EXPECT_OK(vmo.write(data.data(), 0, size));
    }
    return vmo;
  }

  // Helper to read VMO data
  std::vector<uint8_t> ReadVmo(const zx::vmo& vmo, size_t size) {
    std::vector<uint8_t> data(size);
    EXPECT_OK(vmo.read(data.data(), 0, size));
    return data;
  }
};

TEST_F(RpmbTest, GetDeviceInfo) {
  zx::result result = driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient()]() {
    const fidl::WireResult result = fidl::WireCall(client_end)->GetDeviceInfo();
    ASSERT_TRUE(result.ok());
    const auto& info = result.value().info;
    ASSERT_TRUE(info.is_emmc_info());
    ASSERT_EQ(info.emmc_info().rpmb_size, 4);  // 512KB
    ASSERT_EQ(info.emmc_info().reliable_write_sector_count, 1);
  });
  ASSERT_OK(result.status_value());
}

TEST_F(RpmbTest, RequestWriteAndReadSuccess) {
  constexpr size_t kFrameSize = 512;
  zx::vmo tx_vmo = CreateVmo(kFrameSize, 0xAB);
  zx::vmo rx_vmo = CreateVmo(kFrameSize, 0x00);

  // Hook SECURITY_PROTOCOL_OUT (Write)
  bool write_called = false;
  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::SECURITY_PROTOCOL_OUT,
      [&](UfsMockDevice& mock, CommandUpiuData& command_upiu, ResponseUpiuData& response_upiu,
          cpp20::span<PhysicalRegionDescriptionTableEntry>& prdt_upius)
          -> zx::result<std::vector<uint8_t>> {
        write_called = true;
        fdf::info("UFS RpmbTest: cdb addr = 0x{:x}", reinterpret_cast<uintptr_t>(command_upiu.cdb));
        fdf::info(
            "UFS RpmbTest: raw cdb = "
            "{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} "
            "{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
            command_upiu.cdb[0], command_upiu.cdb[1], command_upiu.cdb[2], command_upiu.cdb[3],
            command_upiu.cdb[4], command_upiu.cdb[5], command_upiu.cdb[6], command_upiu.cdb[7],
            command_upiu.cdb[8], command_upiu.cdb[9], command_upiu.cdb[10], command_upiu.cdb[11],
            command_upiu.cdb[12], command_upiu.cdb[13], command_upiu.cdb[14], command_upiu.cdb[15]);

        auto* cdb = reinterpret_cast<scsi::SecurityProtocolOutCDB*>(command_upiu.cdb);

        // Use CustomMemCpy to avoid alignment issues on uncached memory
        scsi::SecurityProtocolOutCDB local_cdb;
        CustomMemCpy(&local_cdb, cdb, sizeof(local_cdb));
        fdf::info("UFS RpmbTest Hook: copied cdb successfully");

        EXPECT_EQ(local_cdb.opcode, scsi::Opcode::SECURITY_PROTOCOL_OUT);
        EXPECT_EQ(local_cdb.security_protocol, 0xEC);
        EXPECT_EQ(betoh16(local_cdb.security_protocol_specific), 0x0001);
        EXPECT_FALSE(local_cdb.inc_512());
        EXPECT_EQ(betoh32(local_cdb.transfer_length), static_cast<uint32_t>(kFrameSize));

        // Verify data sent to mock device
        std::vector<uint8_t> received_data(kFrameSize);
        zx_status_t status = CopyPhysicalRegionToBuffer(mock, received_data, prdt_upius);
        EXPECT_OK(status);
        EXPECT_EQ(received_data, std::vector<uint8_t>(kFrameSize, 0xAB));

        return zx::ok(std::vector<uint8_t>());
      });

  // Hook SECURITY_PROTOCOL_IN (Read)
  bool read_called = false;
  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::SECURITY_PROTOCOL_IN,
      [&](UfsMockDevice& mock, CommandUpiuData& command_upiu, ResponseUpiuData& response_upiu,
          cpp20::span<PhysicalRegionDescriptionTableEntry>& prdt_upius)
          -> zx::result<std::vector<uint8_t>> {
        read_called = true;
        auto* cdb = reinterpret_cast<scsi::SecurityProtocolInCDB*>(command_upiu.cdb);

        // Use CustomMemCpy to avoid alignment issues on uncached memory
        scsi::SecurityProtocolInCDB local_cdb;
        CustomMemCpy(&local_cdb, cdb, sizeof(local_cdb));

        EXPECT_EQ(local_cdb.opcode, scsi::Opcode::SECURITY_PROTOCOL_IN);
        EXPECT_EQ(local_cdb.security_protocol, 0xEC);
        EXPECT_EQ(betoh16(local_cdb.security_protocol_specific), 0x0001);
        EXPECT_FALSE(local_cdb.inc_512());
        EXPECT_EQ(betoh32(local_cdb.allocation_length), static_cast<uint32_t>(kFrameSize));

        // Return mock data to driver
        return zx::ok(std::vector<uint8_t>(kFrameSize, 0xCD));
      });

  zx::result result =
      driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient(), &tx_vmo, &rx_vmo]() {
        fidl::Arena arena;
        zx::vmo tx_vmo_dup, rx_vmo_dup;
        ASSERT_OK(tx_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &tx_vmo_dup));
        ASSERT_OK(rx_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &rx_vmo_dup));

        auto rx_range = fuchsia_mem::wire::Range{
            .vmo = std::move(rx_vmo_dup),
            .offset = 0,
            .size = kFrameSize,
        };
        auto request = fuchsia_hardware_rpmb::wire::Request{
            .tx_frames =
                {
                    .vmo = std::move(tx_vmo_dup),
                    .offset = 0,
                    .size = kFrameSize,
                },
            .rx_frames = fidl::ObjectView<fuchsia_mem::wire::Range>::FromExternal(&rx_range),
        };

        auto response = fidl::WireCall(client_end)->Request(std::move(request));
        ASSERT_TRUE(response.ok());
        ASSERT_TRUE(response.value().is_ok());
      });
  ASSERT_OK(result.status_value());
  EXPECT_TRUE(write_called);
  EXPECT_TRUE(read_called);

  // Verify data read back to client VMO
  std::vector<uint8_t> read_data = ReadVmo(rx_vmo, kFrameSize);
  EXPECT_EQ(read_data, std::vector<uint8_t>(kFrameSize, 0xCD));
}

TEST_F(RpmbTest, RequestValidationFailures) {
  constexpr size_t kFrameSize = 512;

  // 1. TX size not multiple of 512
  {
    zx::vmo tx_vmo = CreateVmo(kFrameSize + 1, 0xAB);
    zx::result result =
        driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient(), &tx_vmo]() {
          zx::vmo tx_vmo_dup;
          ASSERT_OK(tx_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &tx_vmo_dup));
          auto request = fuchsia_hardware_rpmb::wire::Request{
              .tx_frames =
                  {
                      .vmo = std::move(tx_vmo_dup),
                      .offset = 0,
                      .size = kFrameSize + 1,
                  },
              .rx_frames = nullptr,
          };
          auto response = fidl::WireCall(client_end)->Request(std::move(request));
          ASSERT_TRUE(response.ok());
          ASSERT_TRUE(response.value().is_error());
          EXPECT_EQ(response.value().error_value(), ZX_ERR_INVALID_ARGS);
        });
    ASSERT_OK(result.status_value());
  }

  // 2. TX size too large
  {
    zx::vmo tx_vmo = CreateVmo(UfsRpmbDevice::kMaxRpmbTransferSize + 512, 0xAB);
    zx::result result =
        driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient(), &tx_vmo]() {
          zx::vmo tx_vmo_dup;
          ASSERT_OK(tx_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &tx_vmo_dup));
          auto request = fuchsia_hardware_rpmb::wire::Request{
              .tx_frames =
                  {
                      .vmo = std::move(tx_vmo_dup),
                      .offset = 0,
                      .size = UfsRpmbDevice::kMaxRpmbTransferSize + 512,
                  },
              .rx_frames = nullptr,
          };
          auto response = fidl::WireCall(client_end)->Request(std::move(request));
          ASSERT_TRUE(response.ok());
          ASSERT_TRUE(response.value().is_error());
          EXPECT_EQ(response.value().error_value(), ZX_ERR_OUT_OF_RANGE);
        });
    ASSERT_OK(result.status_value());
  }

  // 3. RX size not multiple of 512
  {
    zx::vmo tx_vmo = CreateVmo(kFrameSize, 0xAB);
    zx::vmo rx_vmo = CreateVmo(kFrameSize + 1, 0x00);
    zx::result result =
        driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient(), &tx_vmo, &rx_vmo]() {
          zx::vmo tx_vmo_dup, rx_vmo_dup;
          ASSERT_OK(tx_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &tx_vmo_dup));
          ASSERT_OK(rx_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &rx_vmo_dup));
          auto rx_range = fuchsia_mem::wire::Range{
              .vmo = std::move(rx_vmo_dup),
              .offset = 0,
              .size = kFrameSize + 1,
          };
          auto request = fuchsia_hardware_rpmb::wire::Request{
              .tx_frames =
                  {
                      .vmo = std::move(tx_vmo_dup),
                      .offset = 0,
                      .size = kFrameSize,
                  },
              .rx_frames = fidl::ObjectView<fuchsia_mem::wire::Range>::FromExternal(&rx_range),
          };
          auto response = fidl::WireCall(client_end)->Request(std::move(request));
          ASSERT_TRUE(response.ok());
          ASSERT_TRUE(response.value().is_error());
          EXPECT_EQ(response.value().error_value(), ZX_ERR_INVALID_ARGS);
        });
    ASSERT_OK(result.status_value());
  }

  // 4. RX size too large
  {
    zx::vmo tx_vmo = CreateVmo(kFrameSize, 0xAB);
    zx::vmo rx_vmo = CreateVmo(UfsRpmbDevice::kMaxRpmbTransferSize + 512, 0x00);
    zx::result result =
        driver_test().RunOnBackgroundDispatcherSync([client_end = GetClient(), &tx_vmo, &rx_vmo]() {
          zx::vmo tx_vmo_dup, rx_vmo_dup;
          ASSERT_OK(tx_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &tx_vmo_dup));
          ASSERT_OK(rx_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &rx_vmo_dup));
          auto rx_range = fuchsia_mem::wire::Range{
              .vmo = std::move(rx_vmo_dup),
              .offset = 0,
              .size = UfsRpmbDevice::kMaxRpmbTransferSize + 512,
          };
          auto request = fuchsia_hardware_rpmb::wire::Request{
              .tx_frames =
                  {
                      .vmo = std::move(tx_vmo_dup),
                      .offset = 0,
                      .size = kFrameSize,
                  },
              .rx_frames = fidl::ObjectView<fuchsia_mem::wire::Range>::FromExternal(&rx_range),
          };
          auto response = fidl::WireCall(client_end)->Request(std::move(request));
          ASSERT_TRUE(response.ok());
          ASSERT_TRUE(response.value().is_error());
          EXPECT_EQ(response.value().error_value(), ZX_ERR_OUT_OF_RANGE);
        });
    ASSERT_OK(result.status_value());
  }
}

}  // namespace ufs
