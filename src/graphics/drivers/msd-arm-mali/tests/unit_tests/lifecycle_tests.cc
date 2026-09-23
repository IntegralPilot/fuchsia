// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.kernel/cpp/wire_test_base.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/fake-resource/cpp/fake-resource.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/zx/result.h>

#include <gtest/gtest.h>

#include "src/graphics/drivers/msd-arm-mali/src/gpu_features.h"
#include "src/graphics/drivers/msd-arm-mali/src/registers.h"

enum InterruptIndex {
  kInterruptIndexJob = 0,
  kInterruptIndexMmu = 1,
  kInterruptIndexGpu = 2,
};
namespace {
std::unique_ptr<magma::RegisterIo::Hook> hook_s;
}  // namespace

// Overrides the implementation in msd_arm_device.cc
void InstallMaliRegisterIoHook(magma::RegisterIo* register_io) {
  if (hook_s) {
    register_io->InstallHook(std::move(hook_s));
  }
}

namespace {

class FakeInfoResource : public fidl::testing::WireTestBase<fuchsia_kernel::InfoResource> {
 public:
  FakeInfoResource() {}

  void Get(GetCompleter::Sync& completer) override {
    zx::resource resource;
    fake_resource_create(ZX_RSRC_KIND_SYSTEM, resource.reset_and_get_address());
    completer.Reply(std::move(resource));
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) final {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
};

class MsdArmTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    auto instance_handler = pdev_.GetInstanceHandler(dispatcher);
    zx::result<> result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        std::move(instance_handler), "pdev");
    if (result.is_error()) {
      return result.take_error();
    }

    result = to_driver_vfs.component().AddProtocol<fuchsia_kernel::InfoResource>(
        std::make_unique<FakeInfoResource>());
    return result;
  }

  fdf_fake::FakePDev& pdev() { return pdev_; }

 private:
  fdf_fake::FakePDev pdev_;
};

struct MsdArmTestConfig {
  using DriverType = fdf_testing::EmptyDriverType;
  using EnvironmentType = MsdArmTestEnvironment;
};

TEST(MsdArmDFv2, LoadDriver) {
  fdf_testing::ForegroundDriverTest<MsdArmTestConfig> driver_test;

  // Initialize MMIOs and IRQs needed by the device.
  zx::interrupt gpu_interrupt;
  zx::result<fdf::MmioBuffer> mmio_buffer;
  fdf_fake::FakePDev::Config config{.use_fake_bti = true, .use_fake_irq = true};
  {
    ASSERT_EQ(ZX_OK,
              zx::interrupt::create(zx::resource(0), 0, ZX_INTERRUPT_VIRTUAL, &gpu_interrupt));
    zx::interrupt dup_interrupt;
    ASSERT_EQ(ZX_OK, gpu_interrupt.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_interrupt));
    config.irqs[kInterruptIndexGpu] = std::move(dup_interrupt);

    constexpr uint64_t kMmioSize = 0x100000;
    zx::vmo vmo;
    ASSERT_EQ(ZX_OK, zx::vmo::create(kMmioSize, 0, &vmo));
    zx::vmo dup_vmo;
    ASSERT_EQ(ZX_OK, vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_vmo));
    mmio_buffer =
        fdf::MmioBuffer::Create(0, kMmioSize, std::move(dup_vmo), ZX_CACHE_POLICY_UNCACHED_DEVICE);
    ASSERT_EQ(ZX_OK, mmio_buffer.status_value());
    config.mmios[0] = fdf::PDev::MmioInfo{.size = kMmioSize, .vmo = std::move(vmo)};
  }

  driver_test.RunInEnvironmentTypeContext(
      [&config](MsdArmTestEnvironment& env) { env.pdev().SetConfig(std::move(config)); });

  class MaliHook : public magma::RegisterIo::Hook {
   public:
    MaliHook(fdf::MmioBuffer* mmio_buffer, zx::interrupt* gpu_interrupt)
        : mmio_buffer_(mmio_buffer), gpu_interrupt_(gpu_interrupt) {}
    void Write32(uint32_t val, uint32_t offset) override {
      if ((offset == registers::GpuCommand::kOffset) &&
          (val == registers::GpuCommand::kCmdSoftReset)) {
        // Mark that the reset has completed.
        auto irq_status = registers::GpuIrqFlags::GetStatus().FromValue(0);
        irq_status.set_reset_completed(1);
        irq_status.WriteTo(mmio_buffer_);
        gpu_interrupt_->trigger(0, zx::time_boot());
      }
    }

    virtual void Read32(uint32_t val, uint32_t offset) override {}

    virtual void Read64(uint64_t val, uint32_t offset) override {}

   private:
    fdf::MmioBuffer* mmio_buffer_;
    zx::interrupt* gpu_interrupt_;
  };
  hook_s = std::make_unique<MaliHook>(&*mmio_buffer, &gpu_interrupt);

  // Mark that shader cores are ready.
  {
    constexpr uint64_t kCoresEnabled = (1 << 2) - 1;
    constexpr uint32_t kShaderReadyOffset =
        static_cast<uint32_t>(registers::CoreReadyState::CoreType::kShader) +
        static_cast<uint32_t>(registers::CoreReadyState::StatusType::kReady);
    mmio_buffer->Write32(kCoresEnabled, kShaderReadyOffset);
    mmio_buffer->Write<uint32_t>(kCoresEnabled, GpuFeatures::kShaderPresentLowOffset);
  }

  zx::result<> start_result = driver_test.StartDriver();
  ASSERT_EQ(ZX_OK, start_result.status_value());

  // Hook ownership should have been taken by the driver.
  EXPECT_FALSE(hook_s);

  zx::result<> stop_result = driver_test.StopDriver();
  ASSERT_EQ(ZX_OK, stop_result.status_value());
}

}  // namespace
