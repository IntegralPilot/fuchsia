// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <endian.h>
#include <fidl/fuchsia.hardware.rpmb/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fit/function.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/vmo.h>

#include <format>
#include <memory>
#include <vector>

#include <gtest/gtest.h>

namespace {

constexpr uint16_t kRequestReadWriteCounter = 0x0002;
constexpr uint16_t kResponseReadWriteCounter = 0x0200;
// Mask off the write counter expired bit.
constexpr uint16_t kResultMask = 0xff7f;
constexpr uint16_t kResultOk = 0;

// All fields big-endian.
struct Frame {
  uint8_t stuff[196];
  uint8_t mac[32];
  uint8_t data[256];
  uint8_t nonce[16];
  uint32_t write_counter;
  uint16_t address;
  uint16_t block_count;
  uint16_t result;
  uint16_t request;
};
static_assert(sizeof(Frame) == fuchsia_hardware_rpmb::kFrameSize);

void LogFrame(const Frame& frame) {
  const uint8_t* f = reinterpret_cast<const uint8_t*>(&frame);
  for (size_t i = 0; i < sizeof(frame) / 16; i++) {
    FX_LOGS(INFO) << std::format(
        "{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
        f[(i * 16) + 0], f[(i * 16) + 1], f[(i * 16) + 2], f[(i * 16) + 3], f[(i * 16) + 4],
        f[(i * 16) + 5], f[(i * 16) + 6], f[(i * 16) + 7], f[(i * 16) + 8], f[(i * 16) + 9],
        f[(i * 16) + 10], f[(i * 16) + 11], f[(i * 16) + 12], f[(i * 16) + 13], f[(i * 16) + 14],
        f[(i * 16) + 15]);
  }
}

TEST(RpmbIntegrationTest, GetDeviceInfo) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  std::vector<fpromise::promise<>> promises;

  fit::function<void(fidl::ClientEnd<fuchsia_hardware_rpmb::Rpmb>)> service_handler =
      [&promises, &loop](fidl::ClientEnd<fuchsia_hardware_rpmb::Rpmb> client_end) {
        std::unique_ptr client = std::make_unique<fidl::Client<fuchsia_hardware_rpmb::Rpmb>>(
            std::move(client_end), loop.dispatcher());

        fpromise::bridge<> bridge;
        promises.push_back(bridge.consumer.promise());
        (*client)->GetDeviceInfo().ThenExactlyOnce(
            [client = std::move(client), completer = std::move(bridge.completer)](
                fidl::Result<fuchsia_hardware_rpmb::Rpmb::GetDeviceInfo>& result) mutable {
              completer.complete_ok();

              ASSERT_TRUE(result.is_ok());
              ASSERT_TRUE(result->info().emmc_info().has_value());
              // RPMB_SIZE_MULT must be greater than zero if the RPMB service is available.
              EXPECT_GT(result->info().emmc_info()->rpmb_size(), 0);
              // REL_WR_SEC_C must be one per the eMMC specification.
              EXPECT_EQ(result->info().emmc_info()->reliable_write_sector_count(), 1);
            });
      };

  async::Executor executor(loop.dispatcher());
  fit::callback<void()> idle_handler = [&promises, &loop, &executor]() mutable {
    // Make sure we found at least one RPMB device.
    EXPECT_GT(promises.size(), 0u) << "No RPMB devices found";

    // Wait for all outstanding FIDL requests to complete before stopping the dispatcher.
    auto task =
        fpromise::join_promise_vector(std::move(promises))
            .then([&](fpromise::result<std::vector<fpromise::result<>>>& results) { loop.Quit(); });
    executor.schedule_task(std::move(task));
  };

  component::ServiceMemberWatcher<fuchsia_hardware_rpmb::Service::Device> watcher;
  EXPECT_TRUE(watcher.Begin(loop.dispatcher(), std::move(service_handler), std::move(idle_handler))
                  .is_ok());
  loop.Run();
}

TEST(RpmbIntegrationTest, ReadCounterValue) {
  const std::vector<uint8_t> kTestNonce{0xb5, 0xab, 0xa8, 0xd0, 0x6e, 0x4d, 0xc9, 0x21,
                                        0xd2, 0x98, 0x61, 0x85, 0x46, 0xd9, 0x93, 0xd0};

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  std::vector<fpromise::promise<>> promises;

  zx::vmo tx_frame;
  {
    Frame tx_frame_data{};
    memcpy(tx_frame_data.nonce, kTestNonce.data(), kTestNonce.size());
    tx_frame_data.request = htobe16(kRequestReadWriteCounter);

    zx::vmo tx_frame_rw;
    ASSERT_EQ(zx::vmo::create(sizeof(tx_frame_data), 0, &tx_frame_rw), ZX_OK);
    EXPECT_EQ(tx_frame_rw.write(&tx_frame_data, 0, sizeof(tx_frame_data)), ZX_OK);

    // Keep a read-only copy to pass to the RPMB driver.
    EXPECT_EQ(tx_frame_rw.duplicate(ZX_RIGHTS_BASIC | ZX_RIGHT_READ | ZX_RIGHTS_PROPERTY |
                                        ZX_RIGHT_MAP | ZX_RIGHT_SIGNAL,
                                    &tx_frame),
              ZX_OK);
  }

  auto verify_fidl_result = [&kTestNonce](
                                zx::vmo rx_frame,
                                std::unique_ptr<fidl::Client<fuchsia_hardware_rpmb::Rpmb>> client,
                                fpromise::completer<> completer) {
    return [&kTestNonce, rx_frame = std::move(rx_frame), client = std::move(client),
            completer = std::move(completer)](
               fidl::Result<fuchsia_hardware_rpmb::Rpmb::Request>& result) mutable {
      completer.complete_ok();

      ASSERT_TRUE(result.is_ok());

      Frame rx_frame_data{};
      EXPECT_EQ(rx_frame.read(&rx_frame_data, 0, sizeof(rx_frame_data)), ZX_OK);
      EXPECT_EQ(be16toh(rx_frame_data.result) & kResultMask, kResultOk);
      EXPECT_EQ(be16toh(rx_frame_data.request), kResponseReadWriteCounter);

      const std::vector<uint8_t> received_nonce(rx_frame_data.nonce,
                                                rx_frame_data.nonce + sizeof(rx_frame_data.nonce));
      EXPECT_EQ(received_nonce, kTestNonce);

      FX_LOGS(INFO) << "Dumping read counter value response frame:";
      LogFrame(rx_frame_data);
    };
  };

  fit::function<void(fidl::ClientEnd<fuchsia_hardware_rpmb::Rpmb>)> service_handler =
      [&promises, &loop, tx_frame = tx_frame.borrow(),
       &verify_fidl_result](fidl::ClientEnd<fuchsia_hardware_rpmb::Rpmb> client_end) {
        zx::vmo rx_frame;

        fuchsia_hardware_rpmb::Request request;
        {
          zx::vmo tx_frame_driver;
          ASSERT_EQ(tx_frame->duplicate(ZX_RIGHT_SAME_RIGHTS, &tx_frame_driver), ZX_OK);
          fuchsia_mem::Range tx_range(std::move(tx_frame_driver), 0,
                                      fuchsia_hardware_rpmb::kFrameSize);

          // Create a VMO to hold the RX frame, and keep a read-only copy for ourselves.
          zx::vmo rx_frame_driver;
          ASSERT_EQ(zx::vmo::create(fuchsia_hardware_rpmb::kFrameSize, 0, &rx_frame_driver), ZX_OK);
          ASSERT_EQ(rx_frame_driver.duplicate(ZX_RIGHT_READ, &rx_frame), ZX_OK);
          std::unique_ptr rx_range = std::make_unique<fuchsia_mem::Range>(
              std::move(rx_frame_driver), 0, fuchsia_hardware_rpmb::kFrameSize);

          request = fuchsia_hardware_rpmb::Request(std::move(tx_range), std::move(rx_range));
        }

        std::unique_ptr client = std::make_unique<fidl::Client<fuchsia_hardware_rpmb::Rpmb>>(
            std::move(client_end), loop.dispatcher());

        fpromise::bridge<> bridge;
        promises.push_back(bridge.consumer.promise());
        (*client)
            ->Request(std::move(request))
            .ThenExactlyOnce(verify_fidl_result(std::move(rx_frame), std::move(client),
                                                std::move(bridge.completer)));
      };

  async::Executor executor(loop.dispatcher());
  fit::callback<void()> idle_handler = [&promises, &loop, &executor]() mutable {
    EXPECT_GT(promises.size(), 0u) << "No RPMB devices found";

    auto task =
        fpromise::join_promise_vector(std::move(promises))
            .then([&](fpromise::result<std::vector<fpromise::result<>>>& results) { loop.Quit(); });
    executor.schedule_task(std::move(task));
  };

  component::ServiceMemberWatcher<fuchsia_hardware_rpmb::Service::Device> watcher;
  EXPECT_TRUE(watcher.Begin(loop.dispatcher(), std::move(service_handler), std::move(idle_handler))
                  .is_ok());
  loop.Run();
}

}  // namespace
