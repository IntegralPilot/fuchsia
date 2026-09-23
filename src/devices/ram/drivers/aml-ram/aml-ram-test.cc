// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-ram.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>

#include <atomic>
#include <memory>

#include <ddktl/device.h>
#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"

namespace amlogic_ram {

constexpr size_t kRegCount = 0x01000 / sizeof(uint32_t);

class FakeMmio {
 public:
  FakeMmio() : mmio_(sizeof(uint32_t), kRegCount) {}

  fdf::MmioBuffer mmio() { return fdf::MmioBuffer(mmio_.GetMmioBuffer()); }

  fake_mmio::FakeMmioReg& reg(size_t ix) {
    // AML registers are in virtual address units.
    return mmio_[ix];
  }

 private:
  fake_mmio::FakeMmioRegRegion mmio_;
};

struct IncomingNamespace {
  fdf_fake::FakePDev pdev_server;
  component::OutgoingDirectory outgoing{async_get_default_dispatcher()};
};

class AmlRamDeviceTest : public zxtest::Test {
 public:
  void SetUp() override {
    fdf_fake::FakePDev::Config config;
    config.irqs[0] = {};
    ASSERT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &config.irqs[0]));
    irq_signaller_ = config.irqs[0].borrow();
    config.mmios[0] = mmio_.mmio();

    config.device_info = fdf::PDev::DeviceInfo{
        .pid = PDEV_PID_AMLOGIC_T931,
    };

    dmc_offsets_ = g12_dmc_regs;

    auto outgoing_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_OK(incoming_loop_.StartThread("incoming-ns-thread"));
    incoming_.SyncCall([config = std::move(config), server = std::move(outgoing_endpoints.server)](
                           IncomingNamespace* infra) mutable {
      infra->pdev_server.SetConfig(std::move(config));
      ASSERT_OK(infra->outgoing.AddService<fuchsia_hardware_platform_device::Service>(
          infra->pdev_server.GetInstanceHandler()));
      ASSERT_OK(infra->outgoing.Serve(std::move(server)));
    });
    ASSERT_NO_FATAL_FAILURE();
    fake_parent_->AddFidlService(fuchsia_hardware_platform_device::Service::Name,
                                 std::move(outgoing_endpoints.client), "pdev");

    EXPECT_OK(amlogic_ram::AmlRam::Create(nullptr, fake_parent_.get()));
  }

  void TearDown() override { loop_.Shutdown(); }

  fidl::ClientEnd<ram_metrics::Device> ConnectToFidl() {
    loop_.StartThread("aml-ram-test-thread");

    EXPECT_EQ(1, fake_parent_->child_count());
    auto* child = fake_parent_->GetLatestChild();
    EXPECT_NOT_NULL(child);

    auto endpoints = fidl::Endpoints<ram_metrics::Device>::Create();
    binding_ = fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server),
                                child->GetDeviceContext<AmlRam>());
    return std::move(endpoints.client);
  }

  void InjectInterrupt() { irq_signaller_->trigger(0, zx::time_boot()); }

 protected:
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::shared_ptr<MockDevice> fake_parent_ = MockDevice::FakeRootParent();
  FakeMmio mmio_;
  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_{incoming_loop_.dispatcher(),
                                                                   std::in_place};
  zx::unowned_interrupt irq_signaller_;
  std::optional<fidl::ServerBindingRef<ram_metrics::Device>> binding_;

  // DMC Register Offset
  dmc_reg_ctl_t dmc_offsets_;
};

void WriteDisallowed(uint64_t value) { EXPECT_TRUE(false, "got register write of 0x%lx", value); }

TEST_F(AmlRamDeviceTest, InitDoesNothing) {
  // By itself the driver does not write to registers.
  // The fixture's TearDown() test also the lifecycle bits.
  mmio_.reg(dmc_offsets_.port_ctrl_offset).SetWriteCallback(&WriteDisallowed);
  mmio_.reg(dmc_offsets_.timer_offset).SetWriteCallback(&WriteDisallowed);
}

TEST_F(AmlRamDeviceTest, MalformedRequests) {
  // An invalid request does not write to registers.
  mmio_.reg(dmc_offsets_.port_ctrl_offset).SetWriteCallback(&WriteDisallowed);
  mmio_.reg(dmc_offsets_.timer_offset).SetWriteCallback(&WriteDisallowed);

  auto client = fidl::WireSyncClient<ram_metrics::Device>(ConnectToFidl());

  // Invalid cycles (too low).
  {
    ram_metrics::wire::BandwidthMeasurementConfig config = {(200), {1, 0, 0, 0, 0, 0}};
    auto info = client->MeasureBandwidth(config);
    ASSERT_TRUE(info.ok());
    ASSERT_TRUE(info->is_error());
    EXPECT_EQ(info->error_value(), ZX_ERR_INVALID_ARGS);
  }

  // Invalid cycles (too high).
  {
    ram_metrics::wire::BandwidthMeasurementConfig config = {(0x100000000ull), {1, 0, 0, 0, 0, 0}};
    auto info = client->MeasureBandwidth(config);
    ASSERT_TRUE(info.ok());
    ASSERT_TRUE(info->is_error());
    EXPECT_EQ(info->error_value(), ZX_ERR_INVALID_ARGS);
  }

  // Invalid channel (above channel 3).
  {
    ram_metrics::wire::BandwidthMeasurementConfig config = {(1024 * 1024 * 10), {0, 0, 0, 0, 1}};
    auto info = client->MeasureBandwidth(config);
    ASSERT_TRUE(info.ok());
    ASSERT_TRUE(info->is_error());
    EXPECT_EQ(info->error_value(), ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(AmlRamDeviceTest, ValidRequest) {
  // Perform a request for 3 channels. The harness provides the data that should be
  // read via mmio and verifies that the control registers are accessed in the
  // right sequence.
  constexpr uint32_t kCyclesToMeasure = (1024 * 1024 * 10u);
  constexpr uint32_t kControlStart = DMC_QOS_ENABLE_CTRL | 0b0111;
  constexpr uint32_t kControlStop = DMC_QOS_CLEAR_CTRL | 0b1111;
  constexpr uint32_t kFreq = 0x4 | (0x1 << 10);  // F=24000000 (M=4, N=1, OD=0, OD1=0)

  // Note that the cycles are to be interpreted as shifted 4 bits.
  constexpr uint32_t kReadCycles[] = {0x125001, 0x124002, 0x123003, 0x0};

  ram_metrics::wire::BandwidthMeasurementConfig config = {kCyclesToMeasure, {4, 2, 1, 0, 0, 0}};

  // |step| helps track of the expected sequence of reads and writes.
  std::atomic<int> step = 0;

  mmio_.reg(dmc_offsets_.timer_offset)
      .SetWriteCallback([expect = kCyclesToMeasure, &step](size_t value) {
        EXPECT_EQ(step, 0, "unexpected: 0x%lx", value);
        EXPECT_EQ(value, expect, "0: got write of 0x%lx", value);
        ++step;
      });

  mmio_.reg(dmc_offsets_.port_ctrl_offset).SetReadCallback([]() { return DMC_QOS_CLEAR_CTRL; });

  mmio_.reg(dmc_offsets_.port_ctrl_offset)
      .SetWriteCallback([this, start = kControlStart, stop = kControlStop, &step](size_t value) {
        if (step == 1) {
          EXPECT_EQ(value, start, "1: got write of 0x%lx", value);
          InjectInterrupt();
          ++step;
        } else if (step == 4) {
          EXPECT_EQ(value, stop, "4: got write of 0x%lx", value);
          ++step;
        } else {
          EXPECT_TRUE(false, "unexpected: 0x%lx", value);
        }
      });

  mmio_.reg(dmc_offsets_.pll_ctrl0_offset).SetReadCallback([value = kFreq]() { return value; });

  // Note that reading from `dmc_offsets_.port_ctrl_offset` by default returns 0
  // and that makes the operation succeed.

  mmio_.reg(dmc_offsets_.bw_offset[0]).SetReadCallback([&step, value = kReadCycles[0]]() {
    EXPECT_EQ(step, 2);
    // Value of channel 0 cycles granted.
    return value;
  });

  mmio_.reg(dmc_offsets_.bw_offset[1]).SetReadCallback([&step, value = kReadCycles[1]]() {
    EXPECT_EQ(step, 2);
    // Value of channel 1 cycles granted.
    return value;
  });

  mmio_.reg(dmc_offsets_.bw_offset[2]).SetReadCallback([&step, value = kReadCycles[2]]() {
    EXPECT_EQ(step, 2);
    // Value of channel 2 cycles granted.
    return value;
  });

  mmio_.reg(dmc_offsets_.bw_offset[3]).SetReadCallback([&step, value = kReadCycles[3]]() {
    EXPECT_EQ(step, 2);
    ++step;
    // Value of channel 3 cycles granted.
    return value;
  });

  mmio_.reg(dmc_offsets_.all_bw_offset)
      .SetReadCallback(
          [&step, value = kReadCycles[0] + kReadCycles[1] + kReadCycles[2] + kReadCycles[3]]() {
            EXPECT_EQ(step, 3);
            ++step;
            // Value of all cycles granted.
            return value;
          });

  auto client = fidl::WireSyncClient<ram_metrics::Device>(ConnectToFidl());
  auto info = client->MeasureBandwidth(config);
  ASSERT_TRUE(info.ok());
  ASSERT_FALSE(info->is_error());

  // Check All mmio reads and writes happened.
  EXPECT_EQ(step, 5);

  EXPECT_GT(info->value()->info.timestamp, 0u);
  EXPECT_EQ(info->value()->info.frequency, 24000000u);
  EXPECT_EQ(info->value()->info.bytes_per_cycle, 16u);

  // Check FIDL result makes sense. AML hw does not support read or write only counters.
  int ix = 0;
  for (auto& c : info->value()->info.channels) {
    if (ix < 4) {
      EXPECT_EQ(c.readwrite_cycles, kReadCycles[ix]);
    } else {
      EXPECT_EQ(c.readwrite_cycles, 0u);
    }
    EXPECT_EQ(c.write_cycles, 0u);
    EXPECT_EQ(c.read_cycles, 0u);
    ++ix;
  }
  EXPECT_EQ(info->value()->info.total.readwrite_cycles,
            kReadCycles[0] + kReadCycles[1] + kReadCycles[2] + kReadCycles[3]);
}

TEST_F(AmlRamDeviceTest, ValidRequestWithRetries) {
  constexpr uint32_t kCyclesToMeasure = (1024 * 1024 * 5u);
  constexpr uint32_t kControlStart = DMC_QOS_ENABLE_CTRL | 0b1001;
  constexpr uint32_t kControlStop = DMC_QOS_CLEAR_CTRL | 0b1111;
  constexpr uint32_t kFreq = 0x4 | (0x1 << 10);  // F=24000000 (M=4, N=1, OD=0, OD1=0)

  // Note that the cycles are to be interpreted as shifted 4 bits.
  constexpr uint32_t kReadCycles[] = {0x125001, 0x0, 0x0, 0x123003};

  // This is the number of MMIO accesses we track as part of each bandwidth measurement. Multiple
  // measurement attempts are done in this test case, so this constant is used to determine which
  // attempt and which step within each attempt we're on.
  constexpr int kStepsPerMeasurement = 6;

  ram_metrics::wire::BandwidthMeasurementConfig config = {kCyclesToMeasure, {3, 0, 0, 4}};

  // |step| helps track of the expected sequence of reads and writes.
  std::atomic<int> step = 0;

  mmio_.reg(dmc_offsets_.timer_offset)
      .SetWriteCallback([expect = kCyclesToMeasure, &step](size_t value) {
        EXPECT_EQ(step % kStepsPerMeasurement, 0, "unexpected: 0x%lx", value);
        EXPECT_EQ(value, expect, "0: got write of 0x%lx", value);
        ++step;
      });

  mmio_.reg(dmc_offsets_.port_ctrl_offset).SetReadCallback([&step]() -> uint32_t {
    uint32_t value = 0;
    if (step % kStepsPerMeasurement == 2) {
      // Only return a valid register value on the third try.
      if (step / kStepsPerMeasurement >= 2) {
        value = DMC_QOS_CLEAR_CTRL;
      }

      ++step;
    } else {
      EXPECT_TRUE(false, "unexpected read of port_ctrl_offset");
    }

    return value;
  });

  mmio_.reg(dmc_offsets_.port_ctrl_offset)
      .SetWriteCallback([this, start = kControlStart, stop = kControlStop, &step](size_t value) {
        if (step % kStepsPerMeasurement == 1) {
          EXPECT_EQ(value, start, "1: got write of 0x%lx", value);
          InjectInterrupt();
          ++step;
        } else if (step % kStepsPerMeasurement == 5) {
          EXPECT_EQ(value, stop, "5: got write of 0x%lx", value);
          ++step;
        } else {
          EXPECT_TRUE(false, "unexpected: 0x%lx", value);
        }
      });

  mmio_.reg(dmc_offsets_.pll_ctrl0_offset).SetReadCallback([value = kFreq]() { return value; });

  // Note that reading from `dmc_offsets_.port_ctrl_offset` by default returns 0
  // and that makes the operation succeed.

  mmio_.reg(dmc_offsets_.bw_offset[0]).SetReadCallback([&step, value = kReadCycles[0]]() {
    EXPECT_EQ(step % kStepsPerMeasurement, 3);
    // Value of channel 0 cycles granted.
    return value;
  });

  mmio_.reg(dmc_offsets_.bw_offset[1]).SetReadCallback([&step, value = kReadCycles[1]]() {
    EXPECT_EQ(step % kStepsPerMeasurement, 3);
    // Value of channel 1 cycles granted.
    return value;
  });

  mmio_.reg(dmc_offsets_.bw_offset[2]).SetReadCallback([&step, value = kReadCycles[2]]() {
    EXPECT_EQ(step % kStepsPerMeasurement, 3);
    // Value of channel 2 cycles granted.
    return value;
  });

  mmio_.reg(dmc_offsets_.bw_offset[3]).SetReadCallback([&step, value = kReadCycles[3]]() {
    EXPECT_EQ(step % kStepsPerMeasurement, 3);
    ++step;
    // Value of channel 3 cycles granted.
    return value;
  });

  mmio_.reg(dmc_offsets_.all_bw_offset)
      .SetReadCallback(
          [&step, value = kReadCycles[0] + kReadCycles[1] + kReadCycles[2] + kReadCycles[3]]() {
            EXPECT_EQ(step % kStepsPerMeasurement, 4);
            ++step;
            // Value of all cycles granted.
            return value;
          });

  auto client = fidl::WireSyncClient<ram_metrics::Device>(ConnectToFidl());
  auto info = client->MeasureBandwidth(config);
  ASSERT_TRUE(info.ok());
  ASSERT_FALSE(info->is_error());

  // Check All mmio reads and writes happened.
  EXPECT_EQ(step, 18);

  EXPECT_GT(info->value()->info.timestamp, 0u);
  EXPECT_EQ(info->value()->info.frequency, 24000000u);
  EXPECT_EQ(info->value()->info.bytes_per_cycle, 16u);

  // Check FIDL result makes sense. AML hw does not support read or write only counters.
  int ix = 0;
  for (auto& c : info->value()->info.channels) {
    if (ix < 4) {
      EXPECT_EQ(c.readwrite_cycles, kReadCycles[ix]);
    } else {
      EXPECT_EQ(c.readwrite_cycles, 0u);
    }
    EXPECT_EQ(c.write_cycles, 0u);
    EXPECT_EQ(c.read_cycles, 0u);
    ++ix;
  }
  EXPECT_EQ(info->value()->info.total.readwrite_cycles,
            kReadCycles[0] + kReadCycles[1] + kReadCycles[2] + kReadCycles[3]);
}

}  // namespace amlogic_ram
