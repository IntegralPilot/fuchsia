// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-gpio.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/wire_test_base.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>
#include <mock-mmio-reg/mock-mmio-reg.h>
#include <soc/aml-s905d2/s905d2-gpio.h>
#include <soc/aml-s905d2/s905d2-hw.h>

#include "src/lib/testing/predicates/status.h"

namespace {

constexpr size_t kGpioRegSize = 0x100;
constexpr size_t kInterruptRegSize = 0x30;
constexpr size_t kInterruptRegOffset = 0x3c00;

std::atomic<uint32_t> g_options = ZX_INTERRUPT_VIRTUAL;

}  // namespace

extern "C" {
zx_status_t zx_object_get_info(zx_handle_t handle, uint32_t topic, void* buffer, size_t buffer_size,
                               size_t* actual_count, size_t* avail_count) {
  if (topic == ZX_INFO_INTERRUPT) {
    zx_info_interrupt_t info = {
        .options = g_options.load(),
    };
    ZX_ASSERT(buffer_size >= sizeof(info));
    memcpy(buffer, &info, sizeof(info));
    return ZX_OK;
  }
  return _zx_object_get_info(handle, topic, buffer, buffer_size, actual_count, avail_count);
}
}

namespace gpio {

template <uint32_t kPid>
class FakePlatformDevice final
    : public fidl::testing::WireTestBase<fuchsia_hardware_platform_device::Device> {
 public:
  FakePlatformDevice(fdf::MmioBuffer mmio, fdf::MmioBuffer ao_mmio,
                     fdf::MmioBuffer interrupt_mmio) {
    mmios_.insert({0, std::move(mmio)});
    mmios_.insert({1, std::move(ao_mmio)});
    mmios_.insert({2, std::move(interrupt_mmio)});
  }

  fuchsia_hardware_platform_device::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_platform_device::Service::InstanceHandler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                          fidl::kIgnoreBindingClosure),
    });
  }

  void SetExpectedInterruptFlags(uint32_t flags) { expected_interrupt_flags_ = flags; }

  void GetMmioById(GetMmioByIdRequestView request, GetMmioByIdCompleter::Sync& completer) override {
    auto mmio = mmios_.find(request->index);
    ASSERT_NE(mmio, mmios_.end());
    auto& mmio_buffer = mmio->second;
    fidl::Arena arena;
    auto buffer = fuchsia_hardware_platform_device::wire::Mmio::Builder(arena)
                      .offset(reinterpret_cast<size_t>(&mmio_buffer))
                      .Build();
    completer.ReplySuccess(buffer);
  }

  void GetInterruptById(GetInterruptByIdRequestView request,
                        GetInterruptByIdCompleter::Sync& completer) override {
    EXPECT_EQ(request->flags, expected_interrupt_flags_);
    zx::interrupt irq;
    if (zx_status_t status = zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &irq);
        status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    // Trigger the interrupt so that the test can wait on it.
    irq.trigger(0, zx::time_boot());
    completer.ReplySuccess(std::move(irq));
  }

  void GetNodeDeviceInfo(GetNodeDeviceInfoCompleter::Sync& completer) override {
    fidl::Arena arena;
    auto info = fuchsia_hardware_platform_device::wire::NodeDeviceInfo::Builder(arena);
    // Report kIrqCount IRQs even though we don't check on GetInterruptX calls.
    static constexpr size_t kIrqCount = 3;
    completer.ReplySuccess(info.vid(PDEV_VID_AMLOGIC).pid(kPid).irq_count(kIrqCount).Build());
  }

  void GetMetadata(fuchsia_hardware_platform_device::wire::DeviceGetMetadataRequest* request,
                   GetMetadataCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_platform_device::Device> bindings_;
  uint32_t expected_interrupt_flags_ = ZX_INTERRUPT_MODE_EDGE_HIGH;
  std::unordered_map<uint32_t, fdf::MmioBuffer> mmios_;
};

class TestAmlGpioDriver : public AmlGpioDriver {
 public:
  explicit TestAmlGpioDriver() : AmlGpioDriver() {}

  static DriverRegistration GetDriverRegistration() {
    // Use a custom DriverRegistration to create the DUT. Without this, the non-test implementation
    // will be used by default.
    return FUCHSIA_DRIVER_REGISTRATION_V1(
        fdf_internal::DriverServer2<TestAmlGpioDriver>::initialize,
        fdf_internal::DriverServer2<TestAmlGpioDriver>::destroy);
  }

 protected:
  fpromise::promise<fdf::MmioBuffer, zx_status_t> MapMmio(
      fidl::WireClient<fuchsia_hardware_platform_device::Device>& pdev, uint32_t mmio_id) override {
    fpromise::bridge<fdf::MmioBuffer, zx_status_t> bridge;

    pdev->GetMmioById(mmio_id).Then(
        [completer = std::move(bridge.completer)](
            fidl::WireUnownedResult<fuchsia_hardware_platform_device::Device::GetMmioById>&
                result) mutable {
          EXPECT_TRUE(result.ok());
          EXPECT_FALSE(result->is_error());

          auto& mmio = *result->value();
          EXPECT_TRUE(mmio.has_offset());
          auto* mmio_buffer = reinterpret_cast<fdf::MmioBuffer*>(mmio.offset());
          completer.complete_ok(std::move(*mmio_buffer));
        });

    return bridge.consumer.promise_or(fpromise::error(ZX_ERR_BAD_STATE));
  }
};

template <uint32_t kPid>
class FakeAmlGpioTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.GetInstanceHandler());
      if (result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  ddk_mock::MockMmioRegRegion& mmio() { return mmio_gpio_; }
  ddk_mock::MockMmioRegRegion& ao_mmio() { return mmio_gpio_ao_; }
  ddk_mock::MockMmioRegRegion& interrupt_mmio() { return mmio_interrupt_; }
  FakePlatformDevice<kPid>& pdev() { return pdev_; }

 private:
  ddk_mock::MockMmioRegRegion mmio_gpio_{sizeof(uint32_t), kGpioRegSize};
  ddk_mock::MockMmioRegRegion mmio_gpio_ao_{sizeof(uint32_t), kGpioRegSize};
  ddk_mock::MockMmioRegRegion mmio_interrupt_{sizeof(uint32_t), kInterruptRegSize,
                                              kInterruptRegOffset};
  FakePlatformDevice<kPid> pdev_{mmio_gpio_.GetMmioBuffer(), mmio_gpio_ao_.GetMmioBuffer(),
                                 mmio_interrupt_.GetMmioBuffer()};
};

template <uint32_t kPid>
class FixtureConfig final {
 public:
  using DriverType = TestAmlGpioDriver;
  using EnvironmentType = FakeAmlGpioTestEnvironment<kPid>;
};

template <uint32_t kPid>
class AmlGpioTest : public testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(driver_test_.StartDriver());

    // Wait for initialization to complete in background.
    driver_test_.runtime().RunUntil([&]() {
      return driver_test_.RunInNodeContext(fit::callback<bool(fdf_testing::TestNode&)>(
          [&](fdf_testing::TestNode& node) { return node.children().contains("aml-gpio"); }));
    });

    zx::result client = driver_test_.template Connect<fuchsia_hardware_pinimpl::Service::Device>();
    ASSERT_OK(client);
    client_.Bind(std::move(client.value()));
  }

  void TearDown() override {
    ASSERT_OK(driver_test_.StopDriver());

    driver_test_.RunInEnvironmentTypeContext([](auto& env) {
      env.mmio().VerifyAll();
      env.ao_mmio().VerifyAll();
      env.interrupt_mmio().VerifyAll();
    });
  }

 protected:
  void CheckA113GetInterrupt(uint32_t expected_kernel_interrupt_flags,
                             uint32_t expected_register_polarity_values) {
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) {
      env.pdev().SetExpectedInterruptFlags(expected_kernel_interrupt_flags);
      auto& interrupt_mmio = env.interrupt_mmio();
      interrupt_mmio[0x3c20 * sizeof(uint32_t)].ExpectRead(expected_register_polarity_values);

      // Modify IRQ index for IRQ pin.
      interrupt_mmio[0x3c21 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000048);
      // Interrupt select filter.
      interrupt_mmio[0x3c23 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000007);
    });

    fdf::Arena arena('GPIO');

    fdf::WireUnownedResult result = client().buffer(arena)->GetInterrupt(0x0B, {});
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  void WithMmio(fit::callback<void(ddk_mock::MockMmioRegRegion&)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [callback = std::move(callback)](auto& env) mutable { callback(env.mmio()); });
  }

  void WithAoMmio(fit::callback<void(ddk_mock::MockMmioRegRegion&)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [callback = std::move(callback)](auto& env) mutable { callback(env.ao_mmio()); });
  }

  void WithInterruptMmio(fit::callback<void(ddk_mock::MockMmioRegRegion&)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [callback = std::move(callback)](auto& env) mutable { callback(env.interrupt_mmio()); });
  }

  void WithPDev(fit::callback<void(FakePlatformDevice<kPid>&)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [callback = std::move(callback)](auto& env) mutable { callback(env.pdev()); });
  }

  fdf::WireSyncClient<fuchsia_hardware_pinimpl::PinImpl>& client() { return client_; }

 private:
  static void ExpectReadThenWrite(ddk_mock::MockMmioReg& mmio_reg, uint64_t read_value,
                                  uint64_t expected_write_value) {
    mmio_reg.ExpectRead(read_value).ExpectWrite(expected_write_value);
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig<kPid>> driver_test_;
  fdf::WireSyncClient<fuchsia_hardware_pinimpl::PinImpl> client_;
};

using A113AmlGpioTest = AmlGpioTest<PDEV_PID_AMLOGIC_A113>;
using S905d2AmlGpioTest = AmlGpioTest<PDEV_PID_AMLOGIC_S905D2>;

// PinImplSetAltFunction Tests
TEST_F(A113AmlGpioTest, A113AltMode1) {
  WithMmio([](auto& mmio_region) {
    mmio_region[0x24 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000001);
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(1).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x00, config);
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());
}

TEST_F(A113AmlGpioTest, A113AltMode2) {
  WithMmio([](auto& mmio_region) {
    mmio_region[0x26 * sizeof(uint32_t)].ExpectRead(0x00000009 << 8).ExpectWrite(0x00000005 << 8);
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(5).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x12, config);
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());
}

TEST_F(A113AmlGpioTest, A113AltMode3) {
  WithAoMmio([](auto& ao_mmio) {
    ao_mmio[0x05 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000005 << 16);
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(5).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x56, config);
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());
}

TEST_F(S905d2AmlGpioTest, S905d2AltMode) {
  WithMmio([](auto& mmio) {
    mmio[0xb6 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000001);
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(1).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x00, config);
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());
}

TEST_F(S905d2AmlGpioTest, S905d2SdioClkAltMode) {
  WithMmio([](auto& mmio) {
    mmio[S905D2_PREG_PAD_GPIO5_O * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00020000);
    mmio[S905D2_PERIPHS_PIN_MUX_3 * sizeof(uint32_t)]
        .ExpectRead(0x00000000)
        .ExpectWrite(0x00000000);
    // Verification read for GetPull
    mmio[S905D2_PAD_PULL_UP_EN_REG2 * sizeof(uint32_t)].ExpectRead(0x00000000);
    // Verification read for GetFunction
    mmio[S905D2_PREG_PAD_GPIO5_O * sizeof(uint32_t)].ExpectRead(0x00020000);
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(1).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(S905D2_WIFI_SDIO_CLK, config);
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());
  EXPECT_EQ(result.value()->new_config.function(), 1u);
}

TEST_F(S905d2AmlGpioTest, S905d2SdioDataCmdAltMode) {
  WithMmio([](auto& mmio) {
    mmio[S905D2_PERIPHS_PIN_MUX_2 * sizeof(uint32_t)]
        .ExpectRead(0x00000000)
        .ExpectWrite(0x01000000);
    mmio[S905D2_PERIPHS_PIN_MUX_3 * sizeof(uint32_t)]
        .ExpectRead(0x00000000)
        .ExpectWrite(0x00000000);
    // Verification read for GetPull
    mmio[S905D2_PAD_PULL_UP_EN_REG2 * sizeof(uint32_t)].ExpectRead(0x00000000);
    // Verification read for GetFunction
    mmio[S905D2_PERIPHS_PIN_MUX_2 * sizeof(uint32_t)].ExpectRead(0x01000000);
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(1).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(S905D2_WIFI_SDIO_D0, config);
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());
  EXPECT_EQ(result.value()->new_config.function(), 1u);
}

TEST_F(A113AmlGpioTest, AltModeFail1) {
  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(16).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x00, config);
  ASSERT_TRUE(result.ok());
  EXPECT_FALSE(result->is_ok());
}

TEST_F(A113AmlGpioTest, AltModeFail2) {
  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(1).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0xFFFF, config);
  ASSERT_TRUE(result.ok());
  EXPECT_FALSE(result->is_ok());
}

// PinImplConfigIn Tests
TEST_F(A113AmlGpioTest, A113NoPull0) {
  WithMmio([](auto& mmio) {
    mmio[0x12 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // oen
    mmio[0x3c * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // pull
    mmio[0x4a * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFE);  // pull_en
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kNone)
                    .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Configure(0, config);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  {
    fdf::WireUnownedResult result =
        client().buffer(arena)->SetBufferMode(0, fuchsia_hardware_gpio::BufferMode::kInput);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }
}

TEST_F(A113AmlGpioTest, A113NoPullMid) {
  WithMmio([](auto& mmio) {
    mmio[0x12 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // oen
    mmio[0x3c * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // pull
    mmio[0x4a * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFBFFFF);  // pull_en
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kNone)
                    .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x12, config);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  {
    fdf::WireUnownedResult result =
        client().buffer(arena)->SetBufferMode(0x12, fuchsia_hardware_gpio::BufferMode::kInput);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }
}

TEST_F(A113AmlGpioTest, A113NoPullHigh) {
  WithAoMmio([](auto& ao_mmio) {
    ao_mmio[0x08 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // oen
    ao_mmio[0x0b * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // pull
    ao_mmio[0x0b * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFEFFFF);  // pull_en
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kNone)
                    .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x56, config);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  {
    fdf::WireUnownedResult result =
        client().buffer(arena)->SetBufferMode(0x56, fuchsia_hardware_gpio::BufferMode::kInput);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }
}

TEST_F(S905d2AmlGpioTest, S905d2NoPull0) {
  WithMmio([](auto& mmio) {
    mmio[0x1c * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // oen
    mmio[0x3e * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // pull
    mmio[0x4c * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFE);  // pull_en
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kNone)
                    .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x0, config);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  {
    fdf::WireUnownedResult result =
        client().buffer(arena)->SetBufferMode(0x0, fuchsia_hardware_gpio::BufferMode::kInput);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }
}

TEST_F(S905d2AmlGpioTest, S905d2PullUp) {
  WithMmio([](auto& mmio) {
    mmio[0x10 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // oen
    mmio[0x3a * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // pull
    mmio[0x48 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // pull_en
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kUp)
                    .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x21, config);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  {
    fdf::WireUnownedResult result =
        client().buffer(arena)->SetBufferMode(0x21, fuchsia_hardware_gpio::BufferMode::kInput);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }
}

TEST_F(S905d2AmlGpioTest, S905d2PullDown) {
  WithMmio([](auto& mmio) {
    mmio[0x10 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // oen
    mmio[0x3a * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFE);  // pull
    mmio[0x48 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // pull_en
  });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kDown)
                    .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x20, config);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  {
    fdf::WireUnownedResult result =
        client().buffer(arena)->SetBufferMode(0x20, fuchsia_hardware_gpio::BufferMode::kInput);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }
}

TEST_F(A113AmlGpioTest, A113NoPullFail) {
  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kNone)
                    .Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0xFFFF, config);
  ASSERT_TRUE(result.ok());
  EXPECT_FALSE(result->is_ok());
}

// PinImplConfigOut Tests
TEST_F(A113AmlGpioTest, A113Out) {
  WithMmio([](auto& mmio) {
    mmio[0x0d * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // output
    mmio[0x0c * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFB);  // oen
  });

  fdf::Arena arena('GPIO');
  fdf::WireUnownedResult result =
      client().buffer(arena)->SetBufferMode(0x19, fuchsia_hardware_gpio::BufferMode::kOutputHigh);

  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());
}

// PinImplRead Tests
TEST_F(A113AmlGpioTest, A113Read) {
  WithMmio([](auto& mmio) {
    mmio[0x12 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // oen
    mmio[0x3c * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFF);  // pull
    mmio[0x4a * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFDF);  // pull_en

    mmio[0x14 * sizeof(uint32_t)].ExpectRead(0x00000020);  // read 0x01.
    mmio[0x14 * sizeof(uint32_t)].ExpectRead(0x00000000);  // read 0x00.
    mmio[0x14 * sizeof(uint32_t)].ExpectRead(0x00000020);  // read 0x01.
  });

  fdf::Arena arena('GPIO');

  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                    .pull(fuchsia_hardware_pin::Pull::kNone)
                    .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Configure(5, config);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  {
    fdf::WireUnownedResult result =
        client().buffer(arena)->SetBufferMode(5, fuchsia_hardware_gpio::BufferMode::kInput);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Read(5);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0x01);
  }

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Read(5);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0x00);
  }

  {
    fdf::WireUnownedResult result = client().buffer(arena)->Read(5);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0x01);
  }
}

// PinImplGetInterrupt Tests
TEST_F(A113AmlGpioTest, A113GetInterruptFail) {
  fdf::Arena arena('GPIO');
  fdf::WireUnownedResult result = client().buffer(arena)->GetInterrupt(0xFFFF, {});
  ASSERT_TRUE(result.ok());
  EXPECT_FALSE(result->is_ok());
}

TEST_F(A113AmlGpioTest, A113GetInterruptEdgeLow) {
  // Request edge low polarity. Expect the kernel polarity to be converted to edge high.
  CheckA113GetInterrupt(ZX_INTERRUPT_MODE_EDGE_HIGH, 0x0001'0001);
}

TEST_F(A113AmlGpioTest, A113GetInterruptEdgeHigh) {
  // Request edge high polarity. Expect the kernel polarity to stay as edge high.
  CheckA113GetInterrupt(ZX_INTERRUPT_MODE_EDGE_HIGH, 0x0000'0001);
}

TEST_F(A113AmlGpioTest, A113GetInterruptLevelLow) {
  // Request level low polarity. Expect the kernel polarity to be converted to level high.
  CheckA113GetInterrupt(ZX_INTERRUPT_MODE_LEVEL_HIGH, 0x0001'0000);
}

TEST_F(A113AmlGpioTest, A113GetInterruptLevelHigh) {
  // Request level high polarity. Expect the kernel polarity to stay as level high.
  CheckA113GetInterrupt(ZX_INTERRUPT_MODE_LEVEL_HIGH, 0x0000'0000);
}

// PinImplReleaseInterrupt Tests
TEST_F(A113AmlGpioTest, A113ReleaseInterruptFail) {
  fdf::Arena arena('GPIO');
  fdf::WireUnownedResult result = client().buffer(arena)->ReleaseInterrupt(0x66);
  ASSERT_TRUE(result.ok());
  EXPECT_FALSE(result->is_ok());
}

TEST_F(A113AmlGpioTest, A113ReleaseInterrupt) {
  WithInterruptMmio([](auto& interrupt_mmio) {
    interrupt_mmio[0x3c21 * sizeof(uint32_t)]
        .ExpectRead(0x00000000)
        .ExpectWrite(0x00000048);  // modify
    interrupt_mmio[0x3c20 * sizeof(uint32_t)].ExpectRead(0x00010001);
    interrupt_mmio[0x3c23 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000007);
  });

  fdf::Arena arena('GPIO');

  {
    fdf::WireUnownedResult result = client().buffer(arena)->GetInterrupt(0x0B, {});
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  {
    fdf::WireUnownedResult result = client().buffer(arena)->ReleaseInterrupt(0x0B);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }
}

// PinImplConfigureInterrupt Tests
TEST_F(A113AmlGpioTest, A113ConfigureInterrupt) {
  WithInterruptMmio([](auto& interrupt_mmio) {
    interrupt_mmio[0x3c21 * sizeof(uint32_t)]
        .ExpectRead(0x00000000)
        .ExpectWrite(0x00000048);  // modify
    interrupt_mmio[0x3c20 * sizeof(uint32_t)].ExpectRead(0x00010001);
    interrupt_mmio[0x3c23 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000007);
    interrupt_mmio[0x3c20 * sizeof(uint32_t)]
        .ExpectRead(0x00010001)
        .ExpectWrite(0x00000001)  // polarity + for any edge.
        .ExpectRead(0x00000001)
        .ExpectWrite(0x00000000)
        .ExpectRead(0x00000000)
        .ExpectWrite(0x00010000)
        .ExpectRead(0x00010000)
        .ExpectWrite(0x00010001);
  });

  fdf::Arena arena('GPIO');

  {
    fdf::WireUnownedResult result = client().buffer(arena)->GetInterrupt(0x0B, {});
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  auto edge_high = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                       .mode(fuchsia_hardware_gpio::InterruptMode::kEdgeHigh)
                       .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->ConfigureInterrupt(0x0B, edge_high);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  auto level_high = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                        .mode(fuchsia_hardware_gpio::InterruptMode::kLevelHigh)
                        .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->ConfigureInterrupt(0x0B, level_high);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  auto level_low = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                       .mode(fuchsia_hardware_gpio::InterruptMode::kLevelLow)
                       .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->ConfigureInterrupt(0x0B, level_low);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  auto edge_low = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                      .mode(fuchsia_hardware_gpio::InterruptMode::kEdgeLow)
                      .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->ConfigureInterrupt(0x0B, edge_low);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }
}

TEST_F(A113AmlGpioTest, A113ConfigureThenGetInterrupt) {
  fdf::Arena arena('GPIO');

  // Configuring the interrupt first should not result in any MMIO accesses.

  auto edge_high = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                       .mode(fuchsia_hardware_gpio::InterruptMode::kEdgeHigh)
                       .Build();

  {
    fdf::WireUnownedResult result = client().buffer(arena)->ConfigureInterrupt(0x0B, edge_high);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  WithInterruptMmio([](auto& interrupt_mmio) {
    interrupt_mmio[0x3c20 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000001);
    interrupt_mmio[0x3c21 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000048);
    interrupt_mmio[0x3c23 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000007);
  });

  {
    fdf::WireUnownedResult result = client().buffer(arena)->GetInterrupt(0x0B, {});
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }
}

// PinImplSetDriveStrength Tests
TEST_F(A113AmlGpioTest, A113SetDriveStrength) {
  fdf::Arena arena('GPIO');
  auto config =
      fuchsia_hardware_pin::wire::Configuration::Builder(arena).drive_strength_ua(2).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x87, config);
  ASSERT_TRUE(result.ok());
  EXPECT_FALSE(result->is_ok());
}

TEST_F(S905d2AmlGpioTest, S905d2SetDriveStrength) {
  WithAoMmio([](auto& ao_mmio) {
    ao_mmio[0x08 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFFFFFB);
  });

  fdf::Arena arena('GPIO');
  auto config =
      fuchsia_hardware_pin::wire::Configuration::Builder(arena).drive_strength_ua(3000).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x62, config);
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());
  ASSERT_TRUE(result->value()->new_config.has_drive_strength_ua());
  EXPECT_EQ(result->value()->new_config.drive_strength_ua(), 3000);
}

TEST_F(S905d2AmlGpioTest, S905d2GetDriveStrength) {
  WithAoMmio(
      [](auto& mmio_region) { mmio_region[0x08 * sizeof(uint32_t)].ExpectRead(0xFFFFFFFB); });

  fdf::Arena arena('GPIO');
  auto config = fuchsia_hardware_pin::wire::Configuration::Builder(arena).Build();
  fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x62, config);
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());
  ASSERT_TRUE(result->value()->new_config.has_drive_strength_ua());
  EXPECT_EQ(result->value()->new_config.drive_strength_ua(), 3000);
}

TEST_F(S905d2AmlGpioTest, TimestampMonoInterruptOption) {
  WithPDev([](auto& pdev) {
    pdev.SetExpectedInterruptFlags(ZX_INTERRUPT_MODE_EDGE_HIGH | ZX_INTERRUPT_TIMESTAMP_MONO);
  });
  WithInterruptMmio([](auto& interrupt_mmio) {
    interrupt_mmio[0x3c20 * sizeof(uint32_t)].ExpectRead(0x0001'0001);

    // Modify IRQ index for IRQ pin.
    interrupt_mmio[0x3c21 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000017);
    // Interrupt select filter.
    interrupt_mmio[0x3c23 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000007);
  });

  constexpr auto kOptions = fuchsia_hardware_gpio::InterruptOptions::kTimestampMono;

  fdf::Arena arena('GPIO');
  fdf::WireUnownedResult result = client().buffer(arena)->GetInterrupt(0x0B, kOptions);
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());
}

TEST_F(S905d2AmlGpioTest, WakeableInterruptOption) {
  WithPDev([](auto& pdev) { pdev.SetExpectedInterruptFlags(0); });
  WithInterruptMmio([](auto& interrupt_mmio) {
    interrupt_mmio[0x3c20 * sizeof(uint32_t)].ExpectRead(0x0001'0001);

    // Modify IRQ index for IRQ pin.
    interrupt_mmio[0x3c21 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000017);
    // Interrupt select filter.
    interrupt_mmio[0x3c23 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000007);
  });
  g_options = ZX_INTERRUPT_VIRTUAL & 0x20;

  fdf::Arena arena('GPIO');
  {
    auto config =
        fuchsia_hardware_pin::wire::Configuration::Builder(arena).wake_vector(true).Build();
    fdf::WireUnownedResult result = client().buffer(arena)->Configure(0x0B, config);
  }
  WithPDev([](auto& pdev) { pdev.SetExpectedInterruptFlags(ZX_INTERRUPT_MODE_EDGE_HIGH); });
  fdf::WireUnownedResult result = client().buffer(arena)->GetInterrupt(0x0B, {});
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());
}

TEST_F(A113AmlGpioTest, DestroyInterruptOnRelease) {
  WithInterruptMmio([](auto& interrupt_mmio) {
    interrupt_mmio[0x3c21 * sizeof(uint32_t)]
        .ExpectRead(0x00000000)
        .ExpectWrite(0x00000048);  // modify
    interrupt_mmio[0x3c20 * sizeof(uint32_t)].ExpectRead(0x00010001);
    interrupt_mmio[0x3c23 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x00000007);
  });

  fdf::Arena arena('GPIO');

  zx::interrupt interrupt{};
  {
    fdf::WireUnownedResult result = client().buffer(arena)->GetInterrupt(0x0B, {});
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    interrupt = std::move(result->value()->interrupt);
  }

  // FakePlatformDevice triggered the interrupt -- wait on it to make sure we have a working handle.
  EXPECT_EQ(interrupt.wait(nullptr), ZX_OK);

  {
    fdf::WireUnownedResult result = client().buffer(arena)->ReleaseInterrupt(0x0B);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  // AmlGpio should have called zx_interrupt_destroy in the interrupt for this pin.
  EXPECT_EQ(interrupt.wait(nullptr), ZX_ERR_CANCELED);
}

}  // namespace gpio
