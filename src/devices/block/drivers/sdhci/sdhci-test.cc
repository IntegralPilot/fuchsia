// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdhci.h"

#include <lib/async/cpp/wait.h>
#include <lib/async/default.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/fake-bti/cpp/fake-bti.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fit/function.h>
#include <lib/mmio-ptr/fake.h>
#include <lib/sync/completion.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <atomic>
#include <memory>
#include <optional>
#include <vector>

#include <gtest/gtest.h>

#include "src/devices/block/drivers/sdhci/sdhci_config.h"
#include "src/lib/testing/predicates/status.h"

// Stub out vmo_op_range to allow tests to use fake VMOs.
__EXPORT
zx_status_t zx_vmo_op_range(zx_handle_t handle, uint32_t op, uint64_t offset, uint64_t size,
                            void* buffer, size_t buffer_size) {
  return ZX_OK;
}

namespace {

zx_paddr_t PageMask() { return static_cast<uintptr_t>(zx_system_get_page_size()) - 1; }

}  // namespace

namespace sdhci {

class TestSdhci : public Sdhci {
 public:
  // Modify to configure the behaviour of this test driver.
  static fdf::MmioBuffer* mmio_;

  explicit TestSdhci()
      : Sdhci(),
        irq_ack_wait_(this, ZX_HANDLE_INVALID, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED,
                      ZX_WAIT_ASYNC_EDGE) {}

  void Request(RequestRequestView request, fdf::Arena& arena,
               RequestCompleter::Sync& completer) override {
    ASSERT_EQ(request->reqs.size(), 1u);
    size_t bytes = 0;
    for (fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer : request->reqs[0].buffers) {
      bytes += buffer.size;
    }
    blocks_remaining_ =
        request->reqs[0].blocksize ? static_cast<uint16_t>(bytes / request->reqs[0].blocksize) : 0;
    current_block_ = 0;
    return Sdhci::Request(request, arena, completer);
  }

  uint8_t reset_mask() {
    uint8_t ret = reset_mask_;
    reset_mask_ = 0;
    return ret;
  }

  void* iobuf_virt() const { return iobuf_->virt(); }

  void TriggerCardInterrupt() { card_interrupt_ = true; }
  void InjectTransferError() { inject_error_ = true; }

  zx_status_t BeginIrqAckWait(zx::unowned_interrupt irq) {
    irq_ack_wait_.set_object(irq->get());
    if (zx_status_t status = irq_ack_wait_.Begin(irq_dispatcher()); status != ZX_OK) {
      return status;
    }
    // Trigger the interrupt once to kick off the loop.
    return irq->trigger(0, zx::time{});
  }

 protected:
  zx_status_t WaitForReset(const SoftwareReset mask) override {
    reset_mask_ = mask.reg_value();
    return ZX_OK;
  }

  zx_status_t InitMmio() override {
    regs_mmio_buffer_ = mmio_->View(0);
    HostControllerVersion::Get()
        .FromValue(0)
        .set_specification_version(HostControllerVersion::kSpecificationVersion300)
        .WriteTo(&*regs_mmio_buffer_);
    ClockControl::Get().FromValue(0).set_internal_clock_stable(1).WriteTo(&*regs_mmio_buffer_);
    return ZX_OK;
  }

 private:
  void InterruptAck(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                    const zx_packet_signal_t* signal) {
    if (status != ZX_OK) {
      return;
    }

    // Start the wait and trigger the interrupt again to continue the loop.
    irq_ack_wait_.Begin(irq_dispatcher());
    zx_interrupt_trigger(irq_ack_wait_.object(), 0, {});

    auto interrupt_status = InterruptStatus::Get().FromValue(0).WriteTo(&*regs_mmio_buffer_);

    switch (GetRequestStatus()) {
      case RequestStatus::COMMAND:
        interrupt_status.set_command_complete(1).WriteTo(&*regs_mmio_buffer_);
        return;
      case RequestStatus::TRANSFER_DATA_DMA:
        interrupt_status.set_transfer_complete(1);
        if (inject_error_) {
          interrupt_status.set_error(1).set_data_crc_error(1);
        }
        interrupt_status.WriteTo(&*regs_mmio_buffer_);
        return;
      case RequestStatus::READ_DATA_PIO:
        if (++current_block_ == blocks_remaining_) {
          interrupt_status.set_buffer_read_ready(1).set_transfer_complete(1).WriteTo(
              &*regs_mmio_buffer_);
        } else {
          interrupt_status.set_buffer_read_ready(1).WriteTo(&*regs_mmio_buffer_);
        }
        return;
      case RequestStatus::WRITE_DATA_PIO:
        if (++current_block_ == blocks_remaining_) {
          interrupt_status.set_buffer_write_ready(1).set_transfer_complete(1).WriteTo(
              &*regs_mmio_buffer_);
        } else {
          interrupt_status.set_buffer_write_ready(1).WriteTo(&*regs_mmio_buffer_);
        }
        return;
      case RequestStatus::BUSY_RESPONSE:
        interrupt_status.set_transfer_complete(1).WriteTo(&*regs_mmio_buffer_);
        return;
      default:
        break;
    }

    if (card_interrupt_.exchange(false) &&
        InterruptStatusEnable::Get().ReadFrom(&*regs_mmio_buffer_).card_interrupt() == 1) {
      interrupt_status.set_card_interrupt(1).WriteTo(&*regs_mmio_buffer_);
    }
  }

  uint8_t reset_mask_ = 0;
  std::atomic<uint16_t> blocks_remaining_ = 0;
  std::atomic<uint16_t> current_block_ = 0;
  std::atomic<bool> card_interrupt_ = false;
  std::atomic<bool> inject_error_ = false;
  async::WaitMethod<TestSdhci, &TestSdhci::InterruptAck> irq_ack_wait_;
};

fdf::MmioBuffer* TestSdhci::mmio_;

class FakeSdhci : public fdf::WireServer<fuchsia_hardware_sdhci::Device> {
 public:
  zx::unowned_interrupt irq() const { return irq_.borrow(); }

  // fuchsia.hardware.sdhci/Device protocol implementation
  void GetInterrupt(fdf::Arena& arena, GetInterruptCompleter::Sync& completer) override {
    zx_status_t status =
        zx::interrupt::create(zx::resource(ZX_HANDLE_INVALID), 0, ZX_INTERRUPT_VIRTUAL, &irq_);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }

    zx::interrupt dup;
    ASSERT_OK(irq_.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    completer.buffer(arena).ReplySuccess(std::move(dup));
  }

  void GetSdhciMmio(fdf::Arena& arena, GetSdhciMmioCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void GetCqhciMmio(fdf::Arena& arena, GetCqhciMmioCompleter::Sync& completer) override {
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create(zx_system_get_page_size(), 0, &vmo);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
    completer.buffer(arena).ReplySuccess(std::move(vmo), 0);
  }

  void GetBti(GetBtiRequestView request, fdf::Arena& arena,
              GetBtiCompleter::Sync& completer) override {
    if (request->index != 0) {
      completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
      return;
    }
    zx::result bti = fake_bti::CreateFakeBtiWithPaddrs(dma_paddrs_);
    if (bti.is_error()) {
      completer.buffer(arena).ReplyError(bti.status_value());
      return;
    }
    unowned_bti_ = bti.value().borrow();

    completer.buffer(arena).ReplySuccess(std::move(bti.value()));
  }

  void GetBaseClock(fdf::Arena& arena, GetBaseClockCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(base_clock_);
  }

  void GetQuirks(fdf::Arena& arena, GetQuirksCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(quirks_, dma_boundary_alignment_);
  }

  void HwReset(fdf::Arena& arena, HwResetCompleter::Sync& completer) override {
    hw_reset_invoked_ = true;
    completer.buffer(arena).Reply();
  }

  void VendorConfigureBus(VendorConfigureBusRequestView request, fdf::Arena& arena,
                          VendorConfigureBusCompleter::Sync& completer) override {
    if (request->is_frequency_hz() && supports_set_bus_clock_) {
      completer.buffer(arena).ReplySuccess();
      return;
    }
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void VendorPerformTuning(VendorPerformTuningRequestView request, fdf::Arena& arena,
                           VendorPerformTuningCompleter::Sync& completer) override {
    if (!supports_perform_tuning_) {
      completer.buffer(arena).ReplyError(ZX_ERR_STOP);
      return;
    }
    completer.buffer(arena).ReplySuccess();
  }

  fuchsia_hardware_sdhci::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_sdhci::Service::InstanceHandler({
        .device = binding_group_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                               fidl::kIgnoreBindingClosure),
    });
  }

  void set_dma_paddrs(std::vector<zx_paddr_t> dma_paddrs) {
    if (unowned_bti_->is_valid()) {
      ZX_ASSERT(fake_bti::SetPaddrs(zx::unowned_bti(unowned_bti_), dma_paddrs).is_ok());
    } else {
      dma_paddrs_ = std::move(dma_paddrs);
    }
  }
  zx::unowned_bti& unowned_bti() { return unowned_bti_; }
  void set_base_clock(uint32_t base_clock) { base_clock_ = base_clock; }
  void set_quirks(fuchsia_hardware_sdhci::Quirk quirks) { quirks_ = quirks; }
  void set_dma_boundary_alignment(uint64_t dma_boundary_alignment) {
    dma_boundary_alignment_ = dma_boundary_alignment;
  }
  bool hw_reset_invoked() const { return hw_reset_invoked_; }
  void set_supports_set_bus_clock() { supports_set_bus_clock_ = true; }
  void set_supports_perform_tuning() { supports_perform_tuning_ = true; }

  std::vector<zx_paddr_t> dma_paddrs_;
  zx::unowned_bti unowned_bti_;
  uint32_t base_clock_ = 100'000'000;
  fuchsia_hardware_sdhci::Quirk quirks_;
  uint64_t dma_boundary_alignment_ = 0;
  bool hw_reset_invoked_ = false;
  bool supports_set_bus_clock_ = false;
  bool supports_perform_tuning_ = false;
  zx::interrupt irq_;

  fdf::ServerBindingGroup<fuchsia_hardware_sdhci::Device> binding_group_;
};

class Environment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    zx::result result =
        to_driver_vfs.AddService<fuchsia_hardware_sdhci::Service>(sdhci_.GetInstanceHandler());
    if (result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  fdf::MmioBuffer& mmio() { return mmio_; }
  FakeSdhci& sdhci() { return sdhci_; }

 private:
  fdf::MmioBuffer mmio_ = fdf_testing::CreateMmioBuffer(kRegisterSetSize);
  FakeSdhci sdhci_;
};

class TestConfig final {
 public:
  using DriverType = TestSdhci;
  using EnvironmentType = Environment;
};

class SdhciTest : public ::testing::Test {
 protected:
  zx::result<> StartDriver(std::vector<zx_paddr_t> dma_paddrs,
                           fuchsia_hardware_sdhci::Quirk quirks = {},
                           uint64_t dma_boundary_alignment = 0) {
    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      TestSdhci::mmio_ = &env.mmio();
      env.sdhci().set_dma_paddrs(std::move(dma_paddrs));
      env.sdhci().set_quirks(quirks);
      env.sdhci().set_dma_boundary_alignment(dma_boundary_alignment);
    });

    zx::result<> result =
        driver_test().StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
          sdhci_config::Config config{{.enable_suspend = true}};
          args.config(config.ToVmo());
        });
    if (result.is_error()) {
      return result;
    }

    zx::result client = driver_test().Connect<fuchsia_hardware_sdmmc::SdmmcService::Sdmmc>();
    if (client.is_error()) {
      return client.take_error();
    }
    client_.Bind(*std::move(client), fdf::Dispatcher::GetCurrent()->get());

    zx::unowned_interrupt irq = driver_test().RunInEnvironmentTypeContext(
        fit::callback<zx::unowned_interrupt(Environment&)>(
            [](Environment& env) { return env.sdhci().irq(); }));
    return zx::make_result(driver_test().driver()->BeginIrqAckWait(irq->borrow()));
  }

  zx::result<> StartDriver(fuchsia_hardware_sdhci::Quirk quirks = {},
                           uint64_t dma_boundary_alignment = 0) {
    return StartDriver({}, quirks, dma_boundary_alignment);
  }

  zx::result<> StopDriver() {
    if (zx::result<> result = driver_test().StopDriver(); result.is_error()) {
      return result;
    }
    driver_test().ShutdownAndDestroyDriver();
    return zx::ok();
  }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  void ExpectPmoCount(uint64_t count) {
    zx_info_bti_t bti_info;
    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      EXPECT_OK(env.sdhci().unowned_bti()->get_info(ZX_INFO_BTI, &bti_info, sizeof(bti_info),
                                                    nullptr, nullptr));
    });
    EXPECT_EQ(bti_info.pmo_count, count);
  }

  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
  fdf::WireClient<fuchsia_hardware_sdmmc::Sdmmc> client_;
};

TEST_F(SdhciTest, DriverLifecycle) {
  ASSERT_OK(StartDriver());
  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, BaseClockZero) {
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.sdhci().set_base_clock(0); });

  zx::result<> result = StartDriver();
  EXPECT_TRUE(result.is_error());
}

TEST_F(SdhciTest, BaseClockFromDriver) {
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.sdhci().set_base_clock(0xabcdef); });

  ASSERT_OK(StartDriver());

  EXPECT_EQ(driver_test().driver()->base_clock(), 0xabcdefu);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, BaseClockFromHardware) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get().FromValue(0).set_base_clock_frequency(104).WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  EXPECT_EQ(driver_test().driver()->base_clock(), 104'000'000u);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, HostInfo) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities1::Get()
        .FromValue(0)
        .set_sdr50_support(1)
        .set_sdr104_support(1)
        .set_use_tuning_for_sdr50(1)
        .WriteTo(&env.mmio());
    Capabilities0::Get()
        .FromValue(0)
        .set_base_clock_frequency(1)
        .set_bus_width_8_support(1)
        .set_voltage_3v3_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  fdf::Arena arena('TEST');
  client_.buffer(arena)->HostInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->info.caps,
              fuchsia_hardware_sdmmc::SdmmcHostCap::kBusWidth8 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kVoltage330 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kAutoCmd12 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kSdr50 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kSdr104 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kHs400EnhancedStrobe |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kCommandQueueing);
  });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, HostInfoNoDma) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities1::Get().FromValue(0).set_sdr50_support(1).set_ddr50_support(1).WriteTo(
        &env.mmio());
    Capabilities0::Get()
        .FromValue(0)
        .set_base_clock_frequency(1)
        .set_bus_width_8_support(1)
        .set_voltage_3v3_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver(fuchsia_hardware_sdhci::Quirk::kNoDma));

  fdf::Arena arena('TEST');
  client_.buffer(arena)->HostInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->info.caps,
              fuchsia_hardware_sdmmc::SdmmcHostCap::kBusWidth8 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kVoltage330 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kAutoCmd12 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kDdr50 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kSdr50 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kNoTuningSdr50 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kCommandQueueing |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kHs400EnhancedStrobe);
  });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, HostInfoNoTuning) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities1::Get().FromValue(0).WriteTo(&env.mmio());
    Capabilities0::Get().FromValue(0).set_base_clock_frequency(1).WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver(fuchsia_hardware_sdhci::Quirk::kNonStandardTuning));

  fdf::Arena arena('TEST');
  client_.buffer(arena)->HostInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->info.caps,
              fuchsia_hardware_sdmmc::SdmmcHostCap::kAutoCmd12 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kNoTuningSdr50 |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kCommandQueueing |
                  fuchsia_hardware_sdmmc::SdmmcHostCap::kHs400EnhancedStrobe);
  });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SetSignalVoltage) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get().FromValue(0).set_voltage_3v3_support(1).set_voltage_1v8_support(1).WriteTo(
        &env.mmio());
  });

  ASSERT_OK(StartDriver());

  PresentState::Get().FromValue(0).set_dat_3_0(0b0001).WriteTo(driver_test().driver()->mmio_);

  PowerControl::Get()
      .FromValue(0)
      .set_sd_bus_voltage_vdd1(PowerControl::kBusVoltage1V8)
      .set_sd_bus_power_vdd1(1)
      .WriteTo(driver_test().driver()->mmio_);

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->SetSignalVoltage(fuchsia_hardware_sdmmc::SdmmcVoltage::kV180)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(
      HostControl2::Get().ReadFrom(driver_test().driver()->mmio_).voltage_1v8_signalling_enable());

  PowerControl::Get()
      .FromValue(0)
      .set_sd_bus_voltage_vdd1(PowerControl::kBusVoltage3V3)
      .set_sd_bus_power_vdd1(1)
      .WriteTo(driver_test().driver()->mmio_);

  client_.buffer(arena)
      ->SetSignalVoltage(fuchsia_hardware_sdmmc::SdmmcVoltage::kV330)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_FALSE(
      HostControl2::Get().ReadFrom(driver_test().driver()->mmio_).voltage_1v8_signalling_enable());

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SetSignalVoltageUnsupported) {
  ASSERT_OK(StartDriver());

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->SetSignalVoltage(fuchsia_hardware_sdmmc::SdmmcVoltage::kV330)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_error());
      });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SetBusWidth) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get().FromValue(0).set_bus_width_8_support(1).WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  auto ctrl1 = HostControl1::Get().FromValue(0);

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->SetBusWidth(fuchsia_hardware_sdmmc::SdmmcBusWidth::kEight)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(ctrl1.ReadFrom(driver_test().driver()->mmio_).extended_data_transfer_width());
  EXPECT_FALSE(ctrl1.ReadFrom(driver_test().driver()->mmio_).data_transfer_width_4bit());

  client_.buffer(arena)
      ->SetBusWidth(fuchsia_hardware_sdmmc::SdmmcBusWidth::kOne)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_FALSE(ctrl1.ReadFrom(driver_test().driver()->mmio_).extended_data_transfer_width());
  EXPECT_FALSE(ctrl1.ReadFrom(driver_test().driver()->mmio_).data_transfer_width_4bit());

  client_.buffer(arena)
      ->SetBusWidth(fuchsia_hardware_sdmmc::SdmmcBusWidth::kFour)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_FALSE(ctrl1.ReadFrom(driver_test().driver()->mmio_).extended_data_transfer_width());
  EXPECT_TRUE(ctrl1.ReadFrom(driver_test().driver()->mmio_).data_transfer_width_4bit());

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SetBusWidthNotSupported) {
  ASSERT_OK(StartDriver());

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->SetBusWidth(fuchsia_hardware_sdmmc::SdmmcBusWidth::kEight)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_error());
      });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SetBusFreq) {
  ASSERT_OK(StartDriver());

  auto clock = ClockControl::Get().FromValue(0);

  fdf::Arena arena('TEST');
  client_.buffer(arena)->SetBusFreq(0).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(clock.ReadFrom(driver_test().driver()->mmio_).internal_clock_enable());

  client_.buffer(arena)->SetBusFreq(12'500'000).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(clock.ReadFrom(driver_test().driver()->mmio_).frequency_select(), 4);
  EXPECT_TRUE(clock.sd_clock_enable());
  EXPECT_TRUE(clock.internal_clock_enable());

  client_.buffer(arena)->SetBusFreq(65'190).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(clock.ReadFrom(driver_test().driver()->mmio_).frequency_select(), 767);
  EXPECT_TRUE(clock.sd_clock_enable());
  EXPECT_TRUE(clock.internal_clock_enable());

  client_.buffer(arena)->SetBusFreq(100'000'000).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(clock.ReadFrom(driver_test().driver()->mmio_).frequency_select(), 0);
  EXPECT_TRUE(clock.sd_clock_enable());
  EXPECT_TRUE(clock.internal_clock_enable());

  client_.buffer(arena)->SetBusFreq(26'000'000).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(clock.ReadFrom(driver_test().driver()->mmio_).frequency_select(), 2);
  EXPECT_TRUE(clock.sd_clock_enable());
  EXPECT_TRUE(clock.internal_clock_enable());

  client_.buffer(arena)->SetBusFreq(0).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_FALSE(clock.ReadFrom(driver_test().driver()->mmio_).sd_clock_enable());
  EXPECT_TRUE(clock.internal_clock_enable());

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SetBusFreqVendorSpecific) {
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.sdhci().set_supports_set_bus_clock(); });

  ASSERT_OK(StartDriver(fuchsia_hardware_sdhci::Quirk::kVendorSetBusFreq));

  auto clock_control = [&]() { return ClockControl::Get().ReadFrom(TestSdhci::mmio_).reg_value(); };

  const uint32_t initial_value = clock_control();

  fdf::Arena arena('TEST');
  client_.buffer(arena)->SetBusFreq(0).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(clock_control(), initial_value);
  });

  client_.buffer(arena)->SetBusFreq(12'500'000).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(clock_control(), initial_value);
  });

  client_.buffer(arena)->SetBusFreq(65'190).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(clock_control(), initial_value);
  });

  client_.buffer(arena)->SetBusFreq(100'000'000).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(clock_control(), initial_value);
  });

  client_.buffer(arena)->SetBusFreq(26'000'000).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(clock_control(), initial_value);
  });

  client_.buffer(arena)->SetBusFreq(0).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    driver_test().runtime().Quit();
  });
  driver_test().runtime().Run();

  EXPECT_EQ(clock_control(), initial_value);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, PerformTuningVendorSpecific) {
  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { env.sdhci().set_supports_perform_tuning(); });

  ASSERT_OK(StartDriver(fuchsia_hardware_sdhci::Quirk::kVendorPerformTuning));

  fdf::Arena arena('TEST');
  client_.buffer(arena)->PerformTuning(MMC_SEND_TUNING_BLOCK).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    driver_test().runtime().Quit();
  });
  driver_test().runtime().Run();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SetBusFreqTimeout) {
  ASSERT_OK(StartDriver());

  ClockControl::Get().FromValue(0).set_internal_clock_stable(1).WriteTo(
      driver_test().driver()->mmio_);

  fdf::Arena arena('TEST');
  client_.buffer(arena)->SetBusFreq(12'500'000).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  ClockControl::Get().FromValue(0).WriteTo(driver_test().driver()->mmio_);

  client_.buffer(arena)->SetBusFreq(12'500'000).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_error());
  });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SetBusFreqInternalClockEnable) {
  ASSERT_OK(StartDriver());

  ClockControl::Get()
      .FromValue(0)
      .set_internal_clock_stable(1)
      .set_internal_clock_enable(0)
      .WriteTo(driver_test().driver()->mmio_);

  fdf::Arena arena('TEST');
  client_.buffer(arena)->SetBusFreq(12'500'000).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(ClockControl::Get().ReadFrom(driver_test().driver()->mmio_).internal_clock_enable());

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SetTiming) {
  ASSERT_OK(StartDriver());

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->SetTiming(fuchsia_hardware_sdmmc::SdmmcTiming::kHs)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(HostControl1::Get().ReadFrom(driver_test().driver()->mmio_).high_speed_enable());
  EXPECT_EQ(HostControl2::Get().ReadFrom(driver_test().driver()->mmio_).uhs_mode_select(),
            HostControl2::kUhsModeSdr25);

  client_.buffer(arena)
      ->SetTiming(fuchsia_hardware_sdmmc::SdmmcTiming::kLegacy)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_FALSE(HostControl1::Get().ReadFrom(driver_test().driver()->mmio_).high_speed_enable());
  EXPECT_EQ(HostControl2::Get().ReadFrom(driver_test().driver()->mmio_).uhs_mode_select(),
            HostControl2::kUhsModeSdr12);

  client_.buffer(arena)
      ->SetTiming(fuchsia_hardware_sdmmc::SdmmcTiming::kHsddr)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(HostControl1::Get().ReadFrom(driver_test().driver()->mmio_).high_speed_enable());
  EXPECT_EQ(HostControl2::Get().ReadFrom(driver_test().driver()->mmio_).uhs_mode_select(),
            HostControl2::kUhsModeDdr50);

  client_.buffer(arena)
      ->SetTiming(fuchsia_hardware_sdmmc::SdmmcTiming::kSdr25)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(HostControl1::Get().ReadFrom(driver_test().driver()->mmio_).high_speed_enable());
  EXPECT_EQ(HostControl2::Get().ReadFrom(driver_test().driver()->mmio_).uhs_mode_select(),
            HostControl2::kUhsModeSdr25);

  client_.buffer(arena)
      ->SetTiming(fuchsia_hardware_sdmmc::SdmmcTiming::kSdr12)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(HostControl1::Get().ReadFrom(driver_test().driver()->mmio_).high_speed_enable());
  EXPECT_EQ(HostControl2::Get().ReadFrom(driver_test().driver()->mmio_).uhs_mode_select(),
            HostControl2::kUhsModeSdr12);

  client_.buffer(arena)
      ->SetTiming(fuchsia_hardware_sdmmc::SdmmcTiming::kHs400)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(HostControl1::Get().ReadFrom(driver_test().driver()->mmio_).high_speed_enable());
  EXPECT_EQ(HostControl2::Get().ReadFrom(driver_test().driver()->mmio_).uhs_mode_select(),
            HostControl2::kUhsModeHs400);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, HwReset) {
  ASSERT_OK(StartDriver());

  fdf::Arena arena('TEST');
  client_.buffer(arena)->HwReset().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    driver_test().runtime().Quit();
  });
  driver_test().runtime().Run();

  driver_test().RunInEnvironmentTypeContext(
      [&](Environment& env) { EXPECT_TRUE(env.sdhci().hw_reset_invoked()); });

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, RequestCommandOnly) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get().FromValue(0).set_adma2_support(1).WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_SEND_STATUS,
      .cmd_flags = SDMMC_SEND_STATUS_FLAGS,
      .arg = 0x7b7d9fbd,
      .buffers = {},
  };

  Response::Get(0).FromValue(0xf3bbf2c0).WriteTo(driver_test().driver()->mmio_);

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_EQ(result->value()->response[0], 0xf3bbf2c0u);
      });
  driver_test().runtime().RunUntilIdle();

  auto command = Command::Get().FromValue(0);

  EXPECT_EQ(Argument::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 0x7b7d9fbdu);
  EXPECT_EQ(command.ReadFrom(driver_test().driver()->mmio_).command_index(), SDMMC_SEND_STATUS);
  EXPECT_EQ(command.command_type(), Command::kCommandTypeNormal);
  EXPECT_FALSE(command.data_present());
  EXPECT_TRUE(command.command_index_check());
  EXPECT_TRUE(command.command_crc_check());
  EXPECT_EQ(command.response_type(), Command::kResponseType48Bits);

  request = {
      .cmd_idx = SDMMC_SEND_CSD,
      .cmd_flags = SDMMC_SEND_CSD_FLAGS,
      .arg = 0x9c1dc1ed,
      .buffers = {},
  };

  Response::Get(0).FromValue(0x9f93b17d).WriteTo(driver_test().driver()->mmio_);
  Response::Get(1).FromValue(0x89aaba9e).WriteTo(driver_test().driver()->mmio_);
  Response::Get(2).FromValue(0xc14b059e).WriteTo(driver_test().driver()->mmio_);
  Response::Get(3).FromValue(0x7329a9e3).WriteTo(driver_test().driver()->mmio_);

  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_EQ(result->value()->response[0], 0x9f93b17du);
        EXPECT_EQ(result->value()->response[1], 0x89aaba9eu);
        EXPECT_EQ(result->value()->response[2], 0xc14b059eu);
        EXPECT_EQ(result->value()->response[3], 0x7329a9e3u);
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(Argument::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 0x9c1dc1edu);
  EXPECT_EQ(command.ReadFrom(driver_test().driver()->mmio_).command_index(), SDMMC_SEND_CSD);
  EXPECT_EQ(command.command_type(), Command::kCommandTypeNormal);
  EXPECT_FALSE(command.data_present());
  EXPECT_TRUE(command.command_crc_check());
  EXPECT_EQ(command.response_type(), Command::kResponseType136Bits);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, RequestAbort) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get().FromValue(0).set_adma2_support(1).WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(1024, 0, &vmo));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
      .offset = 0,
      .size = 1024,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0,
      .blocksize = 4,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  driver_test().driver()->reset_mask();

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(driver_test().driver()->reset_mask(), 0);

  fuchsia_hardware_sdmmc::wire::SdmmcReq stop_request = {
      .cmd_idx = SDMMC_STOP_TRANSMISSION,
      .cmd_flags = SDMMC_STOP_TRANSMISSION_FLAGS,
      .blocksize = 0,
      .buffers = {},
  };

  client_.buffer(arena)
      ->Request(
          fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&stop_request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(driver_test().driver()->reset_mask(),
            SoftwareReset::Get().FromValue(0).set_reset_dat(1).set_reset_cmd(1).reg_value());

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, SdioInBandInterrupt) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get().FromValue(0).set_adma2_support(1).WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  struct InterruptHandler : fdf::WireServer<fuchsia_hardware_sdmmc::InBandInterrupt> {
    explicit InterruptHandler(fdf::ServerEnd<fuchsia_hardware_sdmmc::InBandInterrupt> server_end)
        : binding(fdf::Dispatcher::GetCurrent()->get(), std::move(server_end), this,
                  fidl::kIgnoreBindingClosure) {}

    void Callback(fdf::Arena& arena, CallbackCompleter::Sync& completer) override {
      interrupt_count++;
      if (callback) {
        callback();
      }
      completer.buffer(arena).Reply();
    }

    uint32_t interrupt_count = 0;
    fit::callback<void(void)> callback;
    fdf::ServerBinding<fuchsia_hardware_sdmmc::InBandInterrupt> binding;
  };

  auto [client, server] = fdf::Endpoints<fuchsia_hardware_sdmmc::InBandInterrupt>::Create();
  InterruptHandler handler(std::move(server));

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->RegisterInBandInterrupt(std::move(client))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  handler.callback = [&]() { driver_test().runtime().Quit(); };
  driver_test().driver()->TriggerCardInterrupt();
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();
  EXPECT_EQ(handler.interrupt_count, 1u);

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_SEND_CSD,
      .cmd_flags = SDMMC_SEND_CSD_FLAGS,
      .arg = 0x9c1dc1ed,
      .buffers = {},
  };
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_TRUE(client_.buffer(arena)->AckInBandInterrupt().ok());
  driver_test().runtime().RunUntilIdle();

  // Verify that the card interrupt remains enabled after other interrupts have been disabled, such
  // as after a commend.
  handler.callback = [&]() { driver_test().runtime().Quit(); };
  driver_test().driver()->TriggerCardInterrupt();
  driver_test().runtime().Run();
  // Request() may have triggered an interrupt by clearing the interrupt status register.
  EXPECT_GE(handler.interrupt_count, 2u);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, DmaRequest64Bit) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  fdf::Arena arena('TEST');
  for (int i = 0; i < 4; i++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(512 * 16, 0, &vmo));
    client_.buffer(arena)
        ->RegisterVmo(i, 3, std::move(vmo), 64 * i, 512 * 12,
                      fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead)
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          EXPECT_TRUE(result->is_ok());
        });
    driver_test().runtime().RunUntilIdle();
  }

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffers[4] = {
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(1),
          .offset = 16,
          .size = 512,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
          .offset = 32,
          .size = 512 * 3,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(3),
          .offset = 48,
          .size = 512 * 10,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(2),
          .offset = 80,
          .size = 512 * 7,
      },
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 3,
      .buffers =
          fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(buffers),
  };
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(AdmaSystemAddress::Get(0).ReadFrom(driver_test().driver()->mmio_).reg_value(),
            FAKE_BTI_PHYS_ADDR);
  EXPECT_EQ(AdmaSystemAddress::Get(1).ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);

  const auto* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor96*>(driver_test().driver()->iobuf_virt());

  uint64_t address;
  memcpy(&address, &descriptors[0].address, sizeof(address));
  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR + 80);
  EXPECT_EQ(descriptors[0].length, 512u);

  memcpy(&address, &descriptors[1].address, sizeof(address));
  EXPECT_EQ(descriptors[1].attr, 0b100'001u);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR + 32);
  EXPECT_EQ(descriptors[1].length, 512 * 3);

  // Buffer is greater than one page and gets split across two descriptors.
  memcpy(&address, &descriptors[2].address, sizeof(address));
  EXPECT_EQ(descriptors[2].attr, 0b100'001u);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR + 240);
  EXPECT_EQ(descriptors[2].length, zx_system_get_page_size() - 240);

  memcpy(&address, &descriptors[3].address, sizeof(address));
  EXPECT_EQ(descriptors[3].attr, 0b100'001u);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR);
  EXPECT_EQ(descriptors[3].length, (512 * 10) - zx_system_get_page_size() + 240);

  memcpy(&address, &descriptors[4].address, sizeof(address));
  EXPECT_EQ(descriptors[4].attr, 0b100'011);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR + 208);
  EXPECT_EQ(descriptors[4].length, 512 * 7);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, DmaRequest32Bit) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(0)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  fdf::Arena arena('TEST');
  for (int i = 0; i < 4; i++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(512 * 16, 0, &vmo));
    client_.buffer(arena)
        ->RegisterVmo(i, 3, std::move(vmo), 64 * i, 512 * 12,
                      fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite)
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          EXPECT_TRUE(result->is_ok());
        });
    driver_test().runtime().RunUntilIdle();
  }

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffers[4] = {
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(1),
          .offset = 16,
          .size = 512,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
          .offset = 32,
          .size = 512 * 3,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(3),
          .offset = 48,
          .size = 512 * 10,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(2),
          .offset = 80,
          .size = 512 * 7,
      },
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 3,
      .buffers =
          fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(buffers),
  };
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(AdmaSystemAddress::Get(0).ReadFrom(driver_test().driver()->mmio_).reg_value(),
            FAKE_BTI_PHYS_ADDR);
  EXPECT_EQ(AdmaSystemAddress::Get(1).ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);

  const auto* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor64*>(driver_test().driver()->iobuf_virt());

  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(descriptors[0].address, FAKE_BTI_PHYS_ADDR + 80);
  EXPECT_EQ(descriptors[0].length, 512u);

  EXPECT_EQ(descriptors[1].attr, 0b100'001u);
  EXPECT_EQ(descriptors[1].address, FAKE_BTI_PHYS_ADDR + 32);
  EXPECT_EQ(descriptors[1].length, 512 * 3);

  // Buffer is greater than one page and gets split across two descriptors.
  EXPECT_EQ(descriptors[2].attr, 0b100'001u);
  EXPECT_EQ(descriptors[2].address, FAKE_BTI_PHYS_ADDR + 240);
  EXPECT_EQ(descriptors[2].length, zx_system_get_page_size() - 240);

  EXPECT_EQ(descriptors[3].attr, 0b100'001u);
  EXPECT_EQ(descriptors[3].address, FAKE_BTI_PHYS_ADDR);
  EXPECT_EQ(descriptors[3].length, (512 * 10) - zx_system_get_page_size() + 240);

  EXPECT_EQ(descriptors[4].attr, 0b100'011);
  EXPECT_EQ(descriptors[4].address, FAKE_BTI_PHYS_ADDR + 208);
  EXPECT_EQ(descriptors[4].length, 512 * 7);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, DmaSplitOneBoundary) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(0)
        .WriteTo(&env.mmio());
  });

  constexpr zx_paddr_t kDescriptorAddress = 0xc000'0000;
  const zx_paddr_t kStartAddress = 0xa7ff'ffff & ~PageMask();

  ASSERT_OK(StartDriver(
      {
          kDescriptorAddress,
          kStartAddress,
          kStartAddress + zx_system_get_page_size(),
          kStartAddress + (zx_system_get_page_size() * 2),
          0xb000'0000,
      },
      fuchsia_hardware_sdhci::Quirk::kUseDmaBoundaryAlignment, 0x0800'0000));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 4, 0, &vmo));
  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->RegisterVmo(0, 0, std::move(vmo), 0, zx_system_get_page_size() * 4,
                    fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
      // The first buffer should be split across the 128M boundary.
      .offset = zx_system_get_page_size() - 4,
      // Two pages plus 256 bytes.
      .size = (zx_system_get_page_size() * 2) + 256,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 16,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(AdmaSystemAddress::Get(0).ReadFrom(driver_test().driver()->mmio_).reg_value(),
            kDescriptorAddress);
  EXPECT_EQ(AdmaSystemAddress::Get(1).ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);

  const Sdhci::AdmaDescriptor64* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor64*>(driver_test().driver()->iobuf_virt());

  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(descriptors[0].address, 0xa7ff'fffc);
  EXPECT_EQ(descriptors[0].length, 4);

  EXPECT_EQ(descriptors[1].attr, 0b100'001u);
  EXPECT_EQ(descriptors[1].address, 0xa800'0000);
  EXPECT_EQ(descriptors[1].length, zx_system_get_page_size() * 2);

  EXPECT_EQ(descriptors[2].attr, 0b100'011);
  EXPECT_EQ(descriptors[2].address, 0xb000'0000);
  EXPECT_EQ(descriptors[2].length, 256 - 4);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, DmaSplitManyBoundaries) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(0)
        .WriteTo(&env.mmio());
  });

  constexpr zx_paddr_t kDescriptorAddress = 0xc000'0000;
  ASSERT_OK(StartDriver(
      {
          kDescriptorAddress,
          0xabcd'0000,
      },
      fuchsia_hardware_sdhci::Quirk::kUseDmaBoundaryAlignment, 0x100));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->RegisterVmo(0, 0, std::move(vmo), 0, zx_system_get_page_size(),
                    fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
      .offset = 128,
      .size = 16 * 64,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 16,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(AdmaSystemAddress::Get(0).ReadFrom(driver_test().driver()->mmio_).reg_value(),
            kDescriptorAddress);
  EXPECT_EQ(AdmaSystemAddress::Get(1).ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);

  const Sdhci::AdmaDescriptor64* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor64*>(driver_test().driver()->iobuf_virt());

  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(descriptors[0].address, 0xabcd'0080);
  EXPECT_EQ(descriptors[0].length, 128);

  EXPECT_EQ(descriptors[1].attr, 0b100'001u);
  EXPECT_EQ(descriptors[1].address, 0xabcd'0100);
  EXPECT_EQ(descriptors[1].length, 256);

  EXPECT_EQ(descriptors[2].attr, 0b100'001u);
  EXPECT_EQ(descriptors[2].address, 0xabcd'0200);
  EXPECT_EQ(descriptors[2].length, 256);

  EXPECT_EQ(descriptors[3].attr, 0b100'001u);
  EXPECT_EQ(descriptors[3].address, 0xabcd'0300);
  EXPECT_EQ(descriptors[3].length, 256);

  EXPECT_EQ(descriptors[4].attr, 0b100'011);
  EXPECT_EQ(descriptors[4].address, 0xabcd'0400);
  EXPECT_EQ(descriptors[4].length, 128);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, DmaNoBoundaries) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(0)
        .WriteTo(&env.mmio());
  });

  constexpr zx_paddr_t kDescriptorAddress = 0xc000'0000;
  const zx_paddr_t kStartAddress = 0xa7ff'ffff & ~PageMask();

  ASSERT_OK(StartDriver({
      kDescriptorAddress,
      kStartAddress,
      kStartAddress + zx_system_get_page_size(),
      kStartAddress + (zx_system_get_page_size() * 2),
      0xb000'0000,
  }));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 4, 0, &vmo));
  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->RegisterVmo(0, 0, std::move(vmo), 0, zx_system_get_page_size() * 4,
                    fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
      .offset = zx_system_get_page_size() - 4,
      .size = (zx_system_get_page_size() * 2) + 256,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 16,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(AdmaSystemAddress::Get(0).ReadFrom(driver_test().driver()->mmio_).reg_value(),
            kDescriptorAddress);
  EXPECT_EQ(AdmaSystemAddress::Get(1).ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);

  const Sdhci::AdmaDescriptor64* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor64*>(driver_test().driver()->iobuf_virt());

  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(descriptors[0].address, 0xa7ff'fffc);
  EXPECT_EQ(descriptors[0].length, (zx_system_get_page_size() * 2) + 4);

  EXPECT_EQ(descriptors[1].attr, 0b100'011);
  EXPECT_EQ(descriptors[1].address, 0xb000'0000);
  EXPECT_EQ(descriptors[1].length, 256 - 4);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, CommandSettingsMultiBlock) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver(fuchsia_hardware_sdhci::Quirk::kStripResponseCrcPreserveOrder));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->RegisterVmo(0, 0, std::move(vmo), 0, zx_system_get_page_size(),
                    fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
      .offset = 0,
      .size = 1024,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234'abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  Response::Get(0).FromValue(0).set_reg_value(0xabcd'1234).WriteTo(driver_test().driver()->mmio_);
  Response::Get(1).FromValue(0).set_reg_value(0xa5a5'a5a5).WriteTo(driver_test().driver()->mmio_);
  Response::Get(2).FromValue(0).set_reg_value(0x1122'3344).WriteTo(driver_test().driver()->mmio_);
  Response::Get(3).FromValue(0).set_reg_value(0xaabb'ccdd).WriteTo(driver_test().driver()->mmio_);

  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_EQ(result->value()->response[0], 0xabcd'1234u);
        EXPECT_EQ(result->value()->response[1], 0u);
        EXPECT_EQ(result->value()->response[2], 0u);
        EXPECT_EQ(result->value()->response[3], 0u);
      });
  driver_test().runtime().RunUntilIdle();

  const Command command = Command::Get().ReadFrom(driver_test().driver()->mmio_);
  EXPECT_EQ(command.response_type(), Command::kResponseType48Bits);
  EXPECT_TRUE(command.command_crc_check());
  EXPECT_TRUE(command.command_index_check());
  EXPECT_TRUE(command.data_present());
  EXPECT_EQ(command.command_type(), Command::kCommandTypeNormal);
  EXPECT_EQ(command.command_index(), SDMMC_WRITE_MULTIPLE_BLOCK);

  const TransferMode transfer_mode = TransferMode::Get().ReadFrom(driver_test().driver()->mmio_);
  EXPECT_TRUE(transfer_mode.dma_enable());
  EXPECT_TRUE(transfer_mode.block_count_enable());
  EXPECT_EQ(transfer_mode.auto_cmd_enable(), TransferMode::kAutoCmdDisable);
  EXPECT_FALSE(transfer_mode.read());
  EXPECT_TRUE(transfer_mode.multi_block());

  EXPECT_EQ(BlockSize::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 512u);
  EXPECT_EQ(BlockCount::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 2u);
  EXPECT_EQ(Argument::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 0x1234'abcdu);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, CommandSettingsSingleBlock) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver(fuchsia_hardware_sdhci::Quirk::kStripResponseCrcPreserveOrder));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->RegisterVmo(0, 0, std::move(vmo), 0, zx_system_get_page_size(),
                    fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
      .offset = 0,
      .size = 128,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_READ_BLOCK,
      .cmd_flags = SDMMC_READ_BLOCK_FLAGS,
      .arg = 0x1234'abcd,
      .blocksize = 128,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  Response::Get(0).FromValue(0).set_reg_value(0xabcd'1234).WriteTo(driver_test().driver()->mmio_);
  Response::Get(1).FromValue(0).set_reg_value(0xa5a5'a5a5).WriteTo(driver_test().driver()->mmio_);
  Response::Get(2).FromValue(0).set_reg_value(0x1122'3344).WriteTo(driver_test().driver()->mmio_);
  Response::Get(3).FromValue(0).set_reg_value(0xaabb'ccdd).WriteTo(driver_test().driver()->mmio_);

  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_EQ(result->value()->response[0], 0xabcd'1234u);
        EXPECT_EQ(result->value()->response[1], 0u);
        EXPECT_EQ(result->value()->response[2], 0u);
        EXPECT_EQ(result->value()->response[3], 0u);
      });
  driver_test().runtime().RunUntilIdle();

  const Command command = Command::Get().ReadFrom(driver_test().driver()->mmio_);
  EXPECT_EQ(command.response_type(), Command::kResponseType48Bits);
  EXPECT_TRUE(command.command_crc_check());
  EXPECT_TRUE(command.command_index_check());
  EXPECT_TRUE(command.data_present());
  EXPECT_EQ(command.command_type(), Command::kCommandTypeNormal);
  EXPECT_EQ(command.command_index(), SDMMC_READ_BLOCK);

  const TransferMode transfer_mode = TransferMode::Get().ReadFrom(driver_test().driver()->mmio_);
  EXPECT_TRUE(transfer_mode.dma_enable());
  EXPECT_EQ(transfer_mode.auto_cmd_enable(), TransferMode::kAutoCmdDisable);
  EXPECT_TRUE(transfer_mode.read());
  EXPECT_FALSE(transfer_mode.multi_block());
  // The controller ignores block count enable if multi block is cleared.

  EXPECT_EQ(BlockSize::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 128u);
  EXPECT_EQ(BlockCount::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 1u);
  EXPECT_EQ(Argument::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 0x1234'abcdu);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, CommandSettingsBusyResponse) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver(fuchsia_hardware_sdhci::Quirk::kStripResponseCrcPreserveOrder));

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = 55,
      .cmd_flags = SDMMC_RESP_LEN_48B | SDMMC_CMD_TYPE_NORMAL | SDMMC_RESP_CRC_CHECK |
                   SDMMC_RESP_CMD_IDX_CHECK,
      .arg = 0x1234'abcd,
      .blocksize = 0,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = {},
  };

  Response::Get(0).FromValue(0).set_reg_value(0xabcd'1234).WriteTo(driver_test().driver()->mmio_);
  Response::Get(1).FromValue(0).set_reg_value(0xa5a5'a5a5).WriteTo(driver_test().driver()->mmio_);
  Response::Get(2).FromValue(0).set_reg_value(0x1122'3344).WriteTo(driver_test().driver()->mmio_);
  Response::Get(3).FromValue(0).set_reg_value(0xaabb'ccdd).WriteTo(driver_test().driver()->mmio_);

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        EXPECT_EQ(result->value()->response[0], 0xabcd'1234u);
        EXPECT_EQ(result->value()->response[1], 0u);
        EXPECT_EQ(result->value()->response[2], 0u);
        EXPECT_EQ(result->value()->response[3], 0u);
      });
  driver_test().runtime().RunUntilIdle();

  const Command command = Command::Get().ReadFrom(driver_test().driver()->mmio_);
  EXPECT_EQ(command.response_type(), Command::kResponseType48BitsWithBusy);
  EXPECT_TRUE(command.command_crc_check());
  EXPECT_TRUE(command.command_index_check());
  EXPECT_FALSE(command.data_present());
  EXPECT_EQ(command.command_type(), Command::kCommandTypeNormal);
  EXPECT_EQ(command.command_index(), 55);

  const TransferMode transfer_mode = TransferMode::Get().ReadFrom(driver_test().driver()->mmio_);
  EXPECT_FALSE(transfer_mode.dma_enable());
  EXPECT_FALSE(transfer_mode.block_count_enable());
  EXPECT_EQ(transfer_mode.auto_cmd_enable(), TransferMode::kAutoCmdDisable);
  EXPECT_FALSE(transfer_mode.read());
  EXPECT_FALSE(transfer_mode.multi_block());

  EXPECT_EQ(BlockSize::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);
  EXPECT_EQ(BlockCount::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);
  EXPECT_EQ(Argument::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), 0x1234'abcdu);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, ZeroBlockSize) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  fdf::Arena arena('TEST');
  for (int i = 0; i < 4; i++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(512 * 16, 0, &vmo));
    client_.buffer(arena)
        ->RegisterVmo(i, 3, std::move(vmo), 64 * i, 512 * 12,
                      fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead)
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          EXPECT_TRUE(result->is_ok());
        });
    driver_test().runtime().RunUntilIdle();
  }

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffers[4] = {
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(1),
          .offset = 16,
          .size = 512,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
          .offset = 32,
          .size = 512 * 3,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(3),
          .offset = 48,
          .size = 512 * 10,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(2),
          .offset = 80,
          .size = 512 * 7,
      },
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 0,
      .suppress_error_messages = false,
      .client_id = 3,
      .buffers =
          fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(buffers),
  };
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_error());
      });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, NoBuffers) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(512 * 16, 0, &vmo));
  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->RegisterVmo(1, 3, std::move(vmo), 0, 1024,
                    fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead |
                        fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(1),
      .offset = 0,
      .size = 512,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 0,
      .suppress_error_messages = false,
      .client_id = 3,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_error());
      });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, OwnedAndUnownedBuffers) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  zx::vmo vmos[4];
  fdf::Arena arena('TEST');
  for (int i = 0; i < 4; i++) {
    ASSERT_OK(zx::vmo::create(512 * 16, 0, &vmos[i]));
    if (i % 2 == 0) {
      client_.buffer(arena)
          ->RegisterVmo(i, 3, std::move(vmos[i]), 64 * i, 512 * 12,
                        fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead)
          .ThenExactlyOnce([](auto& result) {
            ASSERT_TRUE(result.ok());
            EXPECT_TRUE(result->is_ok());
          });
      driver_test().runtime().RunUntilIdle();
    }
  }

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffers[4] = {
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmos[1])),
          .offset = 16,
          .size = 512,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
          .offset = 32,
          .size = 512 * 3,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmos[3])),
          .offset = 48,
          .size = 512 * 10,
      },
      {
          .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(2),
          .offset = 80,
          .size = 512 * 7,
      },
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 3,
      .buffers =
          fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(buffers),
  };

  ExpectPmoCount(3);

  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  // Unowned buffers should have been unpinned.
  ExpectPmoCount(3);

  EXPECT_EQ(AdmaSystemAddress::Get(0).ReadFrom(driver_test().driver()->mmio_).reg_value(),
            FAKE_BTI_PHYS_ADDR);
  EXPECT_EQ(AdmaSystemAddress::Get(1).ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);

  const auto* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor96*>(driver_test().driver()->iobuf_virt());

  uint64_t address;
  memcpy(&address, &descriptors[0].address, sizeof(address));
  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR + 16);
  EXPECT_EQ(descriptors[0].length, 512u);

  memcpy(&address, &descriptors[1].address, sizeof(address));
  EXPECT_EQ(descriptors[1].attr, 0b100'001u);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR + 32);
  EXPECT_EQ(descriptors[1].length, 512 * 3);

  // Buffer is greater than one page and gets split across two descriptors.
  memcpy(&address, &descriptors[2].address, sizeof(address));
  EXPECT_EQ(descriptors[2].attr, 0b100'001u);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR + 48);
  EXPECT_EQ(descriptors[2].length, zx_system_get_page_size() - 48);

  memcpy(&address, &descriptors[3].address, sizeof(address));
  EXPECT_EQ(descriptors[3].attr, 0b100'001u);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR);
  EXPECT_EQ(descriptors[3].length, (512 * 10) - zx_system_get_page_size() + 48);

  memcpy(&address, &descriptors[4].address, sizeof(address));
  EXPECT_EQ(descriptors[4].attr, 0b100'011);
  EXPECT_EQ(address, FAKE_BTI_PHYS_ADDR + 208);
  EXPECT_EQ(descriptors[4].length, 512 * 7);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, CombineContiguousRegions) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(0)
        .WriteTo(&env.mmio());
  });

  constexpr zx_paddr_t kDescriptorAddress = 0xc000'0000;
  const zx_paddr_t kStartAddress = 0xa7ff'ffff & ~PageMask();

  ASSERT_OK(StartDriver({
      kDescriptorAddress,
      kStartAddress,
      kStartAddress + zx_system_get_page_size(),
      kStartAddress + (zx_system_get_page_size() * 2),
      kStartAddress + (zx_system_get_page_size() * 3),
      0xb000'0000,
  }));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create((zx_system_get_page_size() * 4) + 512, 0, &vmo));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
      .offset = 512,
      .size = zx_system_get_page_size() * 4,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  ExpectPmoCount(1);

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  ExpectPmoCount(1);

  EXPECT_EQ(AdmaSystemAddress::Get(0).ReadFrom(driver_test().driver()->mmio_).reg_value(),
            kDescriptorAddress);
  EXPECT_EQ(AdmaSystemAddress::Get(1).ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);

  const Sdhci::AdmaDescriptor64* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor64*>(driver_test().driver()->iobuf_virt());

  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(descriptors[0].address, kStartAddress + 512);
  EXPECT_EQ(descriptors[0].length, (zx_system_get_page_size() * 4) - 512);

  EXPECT_EQ(descriptors[1].attr, 0b100'011);
  EXPECT_EQ(descriptors[1].address, 0xb000'0000);
  EXPECT_EQ(descriptors[1].length, 512u);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, DiscontiguousRegions) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  constexpr zx_paddr_t kDescriptorAddress = 0xc000'0000;
  constexpr zx_paddr_t kDiscontiguousPageOffset = 0x1'0000'0000;
  const zx_paddr_t kStartAddress = 0xa7ff'ffff & ~PageMask();

  ASSERT_OK(StartDriver({
      kDescriptorAddress,
      kStartAddress,
      kDiscontiguousPageOffset + kStartAddress,
      (2 * kDiscontiguousPageOffset) + kStartAddress,
      (3 * kDiscontiguousPageOffset) + kStartAddress,
      (4 * kDiscontiguousPageOffset) + kStartAddress,
      (4 * kDiscontiguousPageOffset) + kStartAddress + zx_system_get_page_size(),
      (4 * kDiscontiguousPageOffset) + kStartAddress + (2 * zx_system_get_page_size()),
      (5 * kDiscontiguousPageOffset) + kStartAddress,
      (6 * kDiscontiguousPageOffset) + kStartAddress,
      (7 * kDiscontiguousPageOffset) + kStartAddress,
      (7 * kDiscontiguousPageOffset) + kStartAddress + zx_system_get_page_size(),
      (8 * kDiscontiguousPageOffset) + kStartAddress,
  }));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 12, 0, &vmo));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
      .offset = 512,
      .size = (zx_system_get_page_size() * 12) - 512 - 1024,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  ExpectPmoCount(1);

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  ExpectPmoCount(1);

  EXPECT_EQ(AdmaSystemAddress::Get(0).ReadFrom(driver_test().driver()->mmio_).reg_value(),
            kDescriptorAddress);
  EXPECT_EQ(AdmaSystemAddress::Get(1).ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);

  const auto* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor96*>(driver_test().driver()->iobuf_virt());

  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(descriptors[0].get_address(), kStartAddress + 512);
  EXPECT_EQ(descriptors[0].length, zx_system_get_page_size() - 512);

  EXPECT_EQ(descriptors[1].attr, 0b100'001u);
  EXPECT_EQ(descriptors[1].get_address(), kDiscontiguousPageOffset + kStartAddress);
  EXPECT_EQ(descriptors[1].length, zx_system_get_page_size());

  EXPECT_EQ(descriptors[2].attr, 0b100'001u);
  EXPECT_EQ(descriptors[2].get_address(), (2 * kDiscontiguousPageOffset) + kStartAddress);
  EXPECT_EQ(descriptors[2].length, zx_system_get_page_size());

  EXPECT_EQ(descriptors[3].attr, 0b100'001u);
  EXPECT_EQ(descriptors[3].get_address(), (3 * kDiscontiguousPageOffset) + kStartAddress);
  EXPECT_EQ(descriptors[3].length, zx_system_get_page_size());

  EXPECT_EQ(descriptors[4].attr, 0b100'001u);
  EXPECT_EQ(descriptors[4].get_address(), (4 * kDiscontiguousPageOffset) + kStartAddress);
  EXPECT_EQ(descriptors[4].length, zx_system_get_page_size() * 3);

  EXPECT_EQ(descriptors[5].attr, 0b100'001u);
  EXPECT_EQ(descriptors[5].get_address(), (5 * kDiscontiguousPageOffset) + kStartAddress);
  EXPECT_EQ(descriptors[5].length, zx_system_get_page_size());

  EXPECT_EQ(descriptors[6].attr, 0b100'001u);
  EXPECT_EQ(descriptors[6].get_address(), (6 * kDiscontiguousPageOffset) + kStartAddress);
  EXPECT_EQ(descriptors[6].length, zx_system_get_page_size());

  EXPECT_EQ(descriptors[7].attr, 0b100'001u);
  EXPECT_EQ(descriptors[7].get_address(), (7 * kDiscontiguousPageOffset) + kStartAddress);
  EXPECT_EQ(descriptors[7].length, zx_system_get_page_size() * 2);

  EXPECT_EQ(descriptors[8].attr, 0b100'011);
  EXPECT_EQ(descriptors[8].get_address(), (8 * kDiscontiguousPageOffset) + kStartAddress);
  EXPECT_EQ(descriptors[8].length, zx_system_get_page_size() - 1024);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, RegionStartAndEndOffsets) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(0)
        .WriteTo(&env.mmio());
  });

  constexpr zx_paddr_t kDescriptorAddress = 0xc000'0000;
  const zx_paddr_t kStartAddress = 0xa7ff'ffff & ~PageMask();

  ASSERT_OK(StartDriver({
      kDescriptorAddress,
      kStartAddress,
      kStartAddress + zx_system_get_page_size(),
      kStartAddress + (zx_system_get_page_size() * 2),
      kStartAddress + (zx_system_get_page_size() * 3),
  }));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create((zx_system_get_page_size() * 4), 0, &vmo));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
      .offset = 0,
      .size = zx_system_get_page_size(),
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  const Sdhci::AdmaDescriptor64* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor64*>(driver_test().driver()->iobuf_virt());

  EXPECT_EQ(descriptors[0].attr, 0b100'011);
  EXPECT_EQ(descriptors[0].address, kStartAddress);
  EXPECT_EQ(descriptors[0].length, zx_system_get_page_size());

  ASSERT_OK(zx::vmo::create((zx_system_get_page_size() * 4), 0, &vmo));
  buffer.buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo));
  buffer.offset = 512;
  buffer.size = zx_system_get_page_size() - 512;

  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(descriptors[0].attr, 0b100'011);
  EXPECT_EQ(descriptors[0].address, kStartAddress + zx_system_get_page_size() + 512);
  EXPECT_EQ(descriptors[0].length, zx_system_get_page_size() - 512);

  ASSERT_OK(zx::vmo::create((zx_system_get_page_size() * 4), 0, &vmo));
  buffer.buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo));
  buffer.offset = 0;
  buffer.size = zx_system_get_page_size() - 512;

  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(descriptors[0].attr, 0b100'011);
  EXPECT_EQ(descriptors[0].address, kStartAddress + (zx_system_get_page_size() * 2));
  EXPECT_EQ(descriptors[0].length, zx_system_get_page_size() - 512);

  ASSERT_OK(zx::vmo::create((zx_system_get_page_size() * 4), 0, &vmo));
  buffer.buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo));
  buffer.offset = 512;
  buffer.size = zx_system_get_page_size() - 1024;

  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(descriptors[0].attr, 0b100'011);
  EXPECT_EQ(descriptors[0].address, kStartAddress + (zx_system_get_page_size() * 3) + 512);
  EXPECT_EQ(descriptors[0].length, zx_system_get_page_size() - 1024);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, BufferZeroSize) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(0)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  fdf::Arena arena('TEST');
  {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 4, 0, &vmo));
    client_.buffer(arena)
        ->RegisterVmo(1, 0, std::move(vmo), 0, zx_system_get_page_size() * 4,
                      fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead)
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          EXPECT_TRUE(result->is_ok());
        });
    driver_test().runtime().RunUntilIdle();
  }

  {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 4, 0, &vmo));

    fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffers[3] = {
        {
            .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(1),
            .offset = 0,
            .size = 512,
        },
        {
            .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
            .offset = 0,
            .size = 0,
        },
        {
            .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(1),
            .offset = 512,
            .size = 512,
        },
    };

    fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
        .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
        .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
        .arg = 0x1234abcd,
        .blocksize = 512,
        .suppress_error_messages = false,
        .client_id = 0,
        .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
            buffers),
    };

    fdf::Arena arena('TEST');
    client_.buffer(arena)
        ->Request(
            fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          EXPECT_TRUE(result->is_error());
        });
    driver_test().runtime().RunUntilIdle();
  }

  {
    zx::vmo vmo1, vmo2;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 4, 0, &vmo1));
    ASSERT_OK(vmo1.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo2));

    fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffers[3] = {
        {
            .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo1)),
            .offset = 0,
            .size = 512,
        },
        {
            .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(1),
            .offset = 0,
            .size = 0,
        },
        {
            .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo2)),
            .offset = 512,
            .size = 512,
        },
    };

    fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
        .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
        .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
        .arg = 0x1234abcd,
        .blocksize = 512,
        .suppress_error_messages = false,
        .client_id = 0,
        .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
            buffers),
    };

    fdf::Arena arena('TEST');
    client_.buffer(arena)
        ->Request(
            fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          EXPECT_TRUE(result->is_error());
        });
    driver_test().runtime().RunUntilIdle();
  }

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, TransferError) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(512, 0, &vmo));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
      .offset = 0,
      .size = 512,
  };
  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  driver_test().driver()->InjectTransferError();

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_error());
      });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, MaxTransferSize) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  std::vector<zx_paddr_t> bti_paddrs;
  bti_paddrs.push_back(0x1000'0000'0000'0000);

  for (size_t i = 0; i < 512; i++) {
    // 512 pages, fully discontiguous.
    bti_paddrs.push_back(zx_system_get_page_size() * (i + 1) * 2);
  }

  ASSERT_OK(StartDriver(std::move(bti_paddrs)));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(512, 0, &vmo));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
      .offset = 0,
      .size = 512 * zx_system_get_page_size(),
  };
  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  const Sdhci::AdmaDescriptor96* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor96*>(driver_test().driver()->iobuf_virt());

  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(descriptors[0].get_address(), zx_system_get_page_size() * 2);
  EXPECT_EQ(descriptors[0].length, zx_system_get_page_size());

  EXPECT_EQ(descriptors[511].attr, 0b100'011);
  EXPECT_EQ(descriptors[511].get_address(), zx_system_get_page_size() * 2 * 512);
  EXPECT_EQ(descriptors[511].length, zx_system_get_page_size());

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, TransferSizeExceeded) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  std::vector<zx_paddr_t> bti_paddrs;
  bti_paddrs.push_back(0x1000'0000'0000'0000);

  for (size_t i = 0; i < 513; i++) {
    bti_paddrs.push_back(zx_system_get_page_size() * (i + 1) * 2);
  }

  ASSERT_OK(StartDriver(std::move(bti_paddrs)));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(512, 0, &vmo));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
      .offset = 0,
      .size = 513 * zx_system_get_page_size(),
  };
  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_error());
      });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, DmaSplitSizeAndAligntmentBoundaries) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  constexpr zx_paddr_t kDescriptorAddress = 0xc000'0000;
  std::vector<zx_paddr_t> paddrs;
  // Generate a single contiguous physical region.
  paddrs.push_back(kDescriptorAddress);
  for (zx_paddr_t p = 0x1'0001'8000; p < 0x1'0010'0000; p += zx_system_get_page_size()) {
    paddrs.push_back(p);
  }

  ASSERT_OK(StartDriver(std::move(paddrs), fuchsia_hardware_sdhci::Quirk::kUseDmaBoundaryAlignment,
                        0x2'0000));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(1024, 0, &vmo));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
      .offset = 0x1'8000,
      .size = 0x4'0000,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(AdmaSystemAddress::Get(0).ReadFrom(driver_test().driver()->mmio_).reg_value(),
            kDescriptorAddress);
  EXPECT_EQ(AdmaSystemAddress::Get(1).ReadFrom(driver_test().driver()->mmio_).reg_value(), 0u);

  const Sdhci::AdmaDescriptor96* const descriptors =
      reinterpret_cast<Sdhci::AdmaDescriptor96*>(driver_test().driver()->iobuf_virt());

  // Region split due to alignment.
  EXPECT_EQ(descriptors[0].attr, 0b100'001u);
  EXPECT_EQ(descriptors[0].get_address(), 0x1'0001'8000u);
  EXPECT_EQ(descriptors[0].length, 0x8000);

  // Region split due to both alignment and descriptor max size.
  EXPECT_EQ(descriptors[1].attr, 0b100'001u);
  EXPECT_EQ(descriptors[1].get_address(), 0x1'0002'0000u);
  EXPECT_EQ(descriptors[1].length, 0);  // Zero length -> 0x1'0000 bytes
  EXPECT_EQ(descriptors[2].attr, 0b100'001u);

  // Region split due to descriptor max size.
  EXPECT_EQ(descriptors[2].get_address(), 0x1'0003'0000u);
  EXPECT_EQ(descriptors[2].length, 0);

  EXPECT_EQ(descriptors[3].attr, 0b100'001u);
  EXPECT_EQ(descriptors[3].get_address(), 0x1'0004'0000u);
  EXPECT_EQ(descriptors[3].length, 0);

  EXPECT_EQ(descriptors[4].attr, 0b100'011);
  EXPECT_EQ(descriptors[4].get_address(), 0x1'0005'0000u);
  EXPECT_EQ(descriptors[4].length, 0x8000);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, BufferedRead) {
  ASSERT_OK(StartDriver(fuchsia_hardware_sdhci::Quirk::kNoDma));

  constexpr uint32_t kTestWord = 0x1234'5678;
  BufferData::Get().FromValue(kTestWord).WriteTo(driver_test().driver()->mmio_);

  zx::vmo vmo, vmo_dup;
  ASSERT_OK(zx::vmo::create(512 * 8, 0, &vmo));
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo_dup)),
      .offset = 512,
      .size = 512 * 6,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());

  uint32_t actual;

  // Make sure the test word was written to the beginning and end of the buffer, but not outside the
  // range we wanted.

  EXPECT_OK(vmo.read(&actual, 512 - sizeof(actual), sizeof(actual)));
  EXPECT_NE(actual, kTestWord);

  EXPECT_OK(vmo.read(&actual, 512, sizeof(actual)));
  EXPECT_EQ(actual, kTestWord);

  EXPECT_OK(vmo.read(&actual, (512 * 7) - sizeof(actual), sizeof(actual)));
  EXPECT_EQ(actual, kTestWord);

  EXPECT_OK(vmo.read(&actual, (512 * 7), sizeof(actual)));
  EXPECT_NE(actual, kTestWord);
}

TEST_F(SdhciTest, BufferedWrite) {
  ASSERT_OK(StartDriver(fuchsia_hardware_sdhci::Quirk::kNoDma));

  constexpr uint32_t kTestWord = 0x1234'5678;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(512 * 8, 0, &vmo));
  EXPECT_OK(vmo.write(&kTestWord, (512 * 7) - sizeof(kTestWord), sizeof(kTestWord)));

  fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
      .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmo(std::move(vmo)),
      .offset = 512,
      .size = 512 * 6,
  };

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_WRITE_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_WRITE_MULTIPLE_BLOCK_FLAGS,
      .arg = 0,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
          &buffer, 1),
  };

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  // The data port should hold the last word from the buffer.
  EXPECT_EQ(BufferData::Get().ReadFrom(driver_test().driver()->mmio_).reg_value(), kTestWord);

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, PrepareStop) {
  ASSERT_OK(StartDriver());

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_SET_BLOCK_COUNT,
      .cmd_flags = SDMMC_SET_BLOCK_COUNT_FLAGS,
      .arg = 1,
      .blocksize = 0,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers = {},
  };

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  // Call PrepareStop() and make sure the next request is canceled.
  EXPECT_TRUE(driver_test().StopDriver().is_ok());

  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_error());
        EXPECT_EQ(result->error_value(), ZX_ERR_CANCELED);
      });
  driver_test().runtime().RunUntilIdle();

  driver_test().ShutdownAndDestroyDriver();
}

TEST_F(SdhciTest, InhibitCmdChecked) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    PresentState::Get().FromValue(0).set_command_inhibit_cmd(1).WriteTo(&env.mmio());
  });

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_SEND_STATUS,
      .cmd_flags = SDMMC_SEND_STATUS_FLAGS,
      .arg = 0,
      .suppress_error_messages = false,
      .client_id = 0,
  };

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        // The request should fail due to inhibit cmd being set.
        ASSERT_TRUE(result->is_error());
        EXPECT_EQ(result->error_value(), ZX_ERR_TIMED_OUT);
      });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, InhibitDatChecked) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->RegisterVmo(0, 0, std::move(vmo), 0, zx_system_get_page_size(),
                    fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    PresentState::Get().FromValue(0).set_command_inhibit_dat(1).WriteTo(&env.mmio());
  });

  {
    fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion buffer = {
        .buffer = fuchsia_hardware_sdmmc::wire::SdmmcBuffer::WithVmoId(0),
        .offset = 0,
        .size = 512,
    };

    fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
        .cmd_idx = SDMMC_READ_BLOCK,
        .cmd_flags = SDMMC_READ_BLOCK_FLAGS,
        .arg = 1,
        .blocksize = 512,
        .suppress_error_messages = false,
        .client_id = 0,
        .buffers = fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion>::FromExternal(
            &buffer, 1),
    };

    client_.buffer(arena)
        ->Request(
            fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          ASSERT_TRUE(result.ok());
          // The request should fail due to inhibit dat being set.
          ASSERT_TRUE(result->is_error());
          EXPECT_EQ(result->error_value(), ZX_ERR_TIMED_OUT);
        });
  }

  {
    fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
        .cmd_idx = SDMMC_ERASE,  // Choose a command with R1b but no data.
        .cmd_flags = SDMMC_ERASE_FLAGS,
        .arg = 1,
        .suppress_error_messages = false,
        .client_id = 0,
    };

    client_.buffer(arena)
        ->Request(
            fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          ASSERT_TRUE(result.ok());
          // The request should fail due to inhibit dat being set.
          ASSERT_TRUE(result->is_error());
          EXPECT_EQ(result->error_value(), ZX_ERR_TIMED_OUT);
        });
  }

  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

TEST_F(SdhciTest, InhibitDatIgnoreForAbort) {
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    PresentState::Get().FromValue(0).set_command_inhibit_dat(1).WriteTo(&env.mmio());
  });

  fuchsia_hardware_sdmmc::wire::SdmmcReq request = {
      .cmd_idx = SDMMC_STOP_TRANSMISSION,
      .cmd_flags = SDMMC_STOP_TRANSMISSION_FLAGS,
      .arg = 0,
      .suppress_error_messages = false,
      .client_id = 0,
  };

  fdf::Arena arena('TEST');
  client_.buffer(arena)
      ->Request(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq>::FromExternal(&request, 1))
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        // The request should succeed despite inhibit cmd being set.
        ASSERT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(StopDriver());
}

class SdhciBackgroundTest : public ::testing::Test {
 protected:
  zx::result<> StartDriver(std::vector<zx_paddr_t> dma_paddrs,
                           fuchsia_hardware_sdhci::Quirk quirks = {},
                           uint64_t dma_boundary_alignment = 0) {
    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      TestSdhci::mmio_ = &env.mmio();
      env.sdhci().set_dma_paddrs(std::move(dma_paddrs));
      env.sdhci().set_quirks(quirks);
      env.sdhci().set_dma_boundary_alignment(dma_boundary_alignment);
    });

    zx::result<> result =
        driver_test().StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
          sdhci_config::Config config{{.enable_suspend = true}};
          args.config(config.ToVmo());
        });
    if (result.is_error()) {
      return result;
    }

    zx::result client = driver_test().Connect<fuchsia_hardware_sdmmc::SdmmcService::Sdmmc>();
    if (client.is_error()) {
      return client.take_error();
    }
    client_.Bind(*std::move(client));

    return zx::ok();
  }

  zx::result<> StartDriver(fuchsia_hardware_sdhci::Quirk quirks = {},
                           uint64_t dma_boundary_alignment = 0) {
    return StartDriver({}, quirks, dma_boundary_alignment);
  }

  zx::result<> StopDriver() {
    if (zx::result<> result = driver_test().StopDriver(); result.is_error()) {
      return result;
    }
    driver_test().ShutdownAndDestroyDriver();
    return zx::ok();
  }

  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  fdf::WireSyncClient<fuchsia_hardware_sdmmc::Sdmmc> client_;
};

TEST_F(SdhciBackgroundTest, InitializeCommandQueueing) {
  driver_test().RunInEnvironmentTypeContext([](Environment& env) {
    Capabilities0::Get()
        .FromValue(0)
        .set_adma2_support(1)
        .set_v3_64_bit_system_address_support(1)
        .WriteTo(&env.mmio());
  });

  ASSERT_OK(StartDriver());

  class InterruptHandler : public fdf::WireServer<fuchsia_hardware_sdmmc::InBandInterrupt> {
   public:
    explicit InterruptHandler(fdf::ServerEnd<fuchsia_hardware_sdmmc::InBandInterrupt> server_end)
        : binding_(fdf::Dispatcher::GetCurrent()->get(), std::move(server_end), this,
                   fidl::kIgnoreBindingClosure) {}

    void Callback(fdf::Arena& arena, CallbackCompleter::Sync& completer) override {
      std::lock_guard lock(lock_);
      interrupt_count_++;
      completer.buffer(arena).Reply();
    }

    size_t interrupt_count() {
      std::lock_guard lock(lock_);
      return interrupt_count_;
    }

   private:
    std::mutex lock_;
    size_t interrupt_count_ TA_GUARDED(lock_) = 0;
    fdf::ServerBinding<fuchsia_hardware_sdmmc::InBandInterrupt> binding_;
  };

  auto [client, server] = fdf::Endpoints<fuchsia_hardware_sdmmc::InBandInterrupt>::Create();
  InterruptHandler handler(std::move(server));

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult result = client_.buffer(arena)->RegisterInBandInterrupt(std::move(client));
  EXPECT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());

  EXPECT_TRUE(client_.buffer(arena)->AckInBandInterrupt().ok());

  zx::interrupt virtual_irq, virtual_irq_device;
  ASSERT_OK(zx::interrupt::create(zx::resource(ZX_HANDLE_INVALID), 0, ZX_INTERRUPT_VIRTUAL,
                                  &virtual_irq));
  ASSERT_OK(virtual_irq.duplicate(ZX_RIGHT_SAME_RIGHTS, &virtual_irq_device));
  zx::eventpair virtual_irq_lifeline_host, virtual_irq_lifeline_device;
  ASSERT_OK(zx::eventpair::create(0, &virtual_irq_lifeline_host, &virtual_irq_lifeline_device));

  zx::interrupt physical_irq;
  zx::vmo cqhci_mmio;
  zx::vmo sdhci_mmio;
  zx::bti bti;

  fdf::WireUnownedResult result2 = client_.buffer(arena)->InitializeCommandQueueing(
      std::move(virtual_irq_device), std::move(virtual_irq_lifeline_device));
  EXPECT_TRUE(result2.ok());
  EXPECT_TRUE(result2->is_ok());
  physical_irq = std::move(result2->value()->interrupt);
  cqhci_mmio = std::move(result2->value()->cqhci_mmio);
  sdhci_mmio = std::move(result2->value()->sdhci_mmio);
  bti = std::move(result2->value()->bti);
  EXPECT_TRUE(physical_irq.is_valid());
  EXPECT_TRUE(cqhci_mmio.is_valid());
  EXPECT_TRUE(sdhci_mmio.is_valid());
  EXPECT_TRUE(bti.is_valid());

  // Verify that triggering the physical interrupt results in no interrupt delivery, because the
  // driver is watching the virtual IRQ instead.
  EXPECT_OK(physical_irq.trigger(0, zx::time()));
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    sdhci::InterruptStatus::Get().FromValue(0).set_card_interrupt(1).WriteTo(&env.mmio());
  });
  zx::nanosleep(zx::deadline_after(zx::msec(10)));
  driver_test().runtime().RunUntilIdle();
  ASSERT_EQ(handler.interrupt_count(), 0u);
  // Wait to consume the pending physical IRQ.  This also verifies that the driver's port is no
  // longer bound to the physical IRQ (since zx_interrupt_wait requires that).
  EXPECT_OK(physical_irq.wait(nullptr));

  // Verify that triggering the virtual IRQ eventually results in the IRQ being delivered and acked
  // by the driver.
  EXPECT_OK(virtual_irq.trigger(0, zx::time()));
  EXPECT_OK(virtual_irq.wait_one(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, zx::time::infinite(), nullptr));
  // Although the IRQ has been acked at this point, the driver may not have yet called Callback,
  // because that happens on a different dispatcher from the IRQ dispatcher.  From the IRQ
  // dispatcher's perspective, Callback is fire-and-forget.
  // The callback will run on this test's dispatcher, so make sure we yield to allow it to run.
  driver_test().runtime().RunUntil([&]() { return handler.interrupt_count() >= 1u; });
  size_t num_irqs = handler.interrupt_count();

  // Verify that after dropping lifeline, the driver will handle physical IRQs again.
  // Since we're using a virtual IRQ, it's edge-triggered, so we need to do this in a loop.  Real
  // interrupts are level-triggered.
  virtual_irq_lifeline_host.reset();
  while (handler.interrupt_count() < num_irqs + 1) {
    // TODO(https://fxbug.dev/492600836): This ack shouldn't be necessary, but it is due to a kernel
    // bug where the interrupt may end up in a state where it's unacked but will also never enqueue
    // a port packet to the driver.
    physical_irq.ack();
    EXPECT_OK(physical_irq.trigger(0, zx::time()));
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
    driver_test().runtime().RunUntilIdle();
  }
  // Make sure the driver acked the physical interrupt.
  zx_signals_t signals;
  physical_irq.wait_one(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, zx::time::infinite_past(), &signals);
  EXPECT_TRUE(signals & ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);

  ASSERT_OK(StopDriver());
}

}  // namespace sdhci

FUCHSIA_DRIVER_EXPORT2(sdhci::TestSdhci);
