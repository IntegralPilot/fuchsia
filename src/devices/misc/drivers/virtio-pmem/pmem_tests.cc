// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fake-bti/bti.h>
#include <lib/fake-resource/resource.h>
#include <lib/virtio/backends/fake.h>

#include <gtest/gtest.h>

#include "src/devices/misc/drivers/virtio-pmem/pmem.h"
#include "src/devices/misc/drivers/virtio-pmem/virtio/pmem.h"
#include "src/lib/testing/predicates/status.h"

namespace {

// Fake virtio 'backend' for a virtio-pmem device.
class FakeBackendForPmem final : public virtio::FakeBackend {
 public:
  static constexpr uint64_t kFakeStart = 0xaf000000;
  static constexpr uint64_t kFakeSize = 0x20000000;

  explicit FakeBackendForPmem(zx::unowned_bti fake_bti)
      : virtio::FakeBackend({{0, 1}}), fake_bti_(std::move(fake_bti)) {}

  ~FakeBackendForPmem() final = default;

  uint64_t ReadFeatures() final {
    uint64_t features = FakeBackend::ReadFeatures();

    features |= VIRTIO_F_VERSION_1;
    features |= VIRTIO_PMEM_F_SHMEM_REGION;

    return features;
  }

  void SetFeatures(uint64_t bitmap) final {
    FakeBackend::SetFeatures(bitmap);

    // Don't declare support for VIRTIO_PMEM_F_SHMEM_REGION.
    ZX_ASSERT((bitmap & VIRTIO_PMEM_F_SHMEM_REGION) == 0);
  }

  // FakeBackend does not implement 64 bit config reads.
  void ReadDeviceConfig(uint16_t offset, uint64_t* value) final {
    if (offset == offsetof(virtio_pmem_config, start)) {
      *value = kFakeStart;
    } else if (offset == offsetof(virtio_pmem_config, size)) {
      *value = kFakeSize;
    } else {
      ZX_ASSERT_MSG(false, "Invalid offset");
    }
  }

 private:
  zx::unowned_bti fake_bti_;
};

zx::result<zx::resource> CreateFakeMmioResource() {
  zx::resource fake_mmio_root;
  zx_status_t status =
      fake_resource_create(ZX_RSRC_KIND_MMIO, fake_mmio_root.reset_and_get_address());
  if (status != ZX_OK) {
    return zx::error(status);
  }
  zx::resource fake_mmio_resource;
  status = zx::resource::create(fake_mmio_root, ZX_RSRC_KIND_MMIO, FakeBackendForPmem::kFakeStart,
                                FakeBackendForPmem::kFakeSize, "", 0, &fake_mmio_resource);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(fake_mmio_resource));
}

TEST(PmemDevice, Init) {
  fdf_testing::ScopedGlobalLogger logger;

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  zx::result fake_mmio_resource = CreateFakeMmioResource();
  ASSERT_OK(fake_mmio_resource.status_value());

  auto backend = std::make_unique<FakeBackendForPmem>(fake_bti.borrow());
  auto* backend_ptr = backend.get();
  virtio::PmemDevice device(std::move(fake_bti), std::move(backend),
                            std::move(*fake_mmio_resource));

  ASSERT_OK(device.Init());
  EXPECT_EQ(backend_ptr->DeviceState(), virtio::FakeBackend::State::DRIVER_OK);
}

class TestPmemDriver : public virtio::PmemDriver {
 public:
  TestPmemDriver() = default;

 private:
  zx::result<std::unique_ptr<virtio::PmemDevice>> CreatePmemDevice(
      const std::shared_ptr<fdf::Namespace>& incoming) final {
    zx::bti fake_bti;
    zx_status_t status = fake_bti_create(fake_bti.reset_and_get_address());
    if (status != ZX_OK) {
      return zx::error(status);
    }
    auto backend = std::make_unique<FakeBackendForPmem>(fake_bti.borrow());
    zx::result fake_mmio_resource = CreateFakeMmioResource();
    if (fake_mmio_resource.is_error()) {
      return fake_mmio_resource.take_error();
    }
    return zx::ok(std::make_unique<virtio::PmemDevice>(std::move(fake_bti), std::move(backend),
                                                       std::move(*fake_mmio_resource)));
  }
};

struct TestConfig final {
  using DriverType = TestPmemDriver;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

class PmemTestDriver : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
};

TEST_F(PmemTestDriver, Lifecycle) {}

TEST_F(PmemTestDriver, ServiceTest) {
  zx::result service = driver_test().Connect<fuchsia_hardware_virtio_pmem::Service::Device>();
  ASSERT_OK(service.status_value());
  zx::result run_result = driver_test().RunOnBackgroundDispatcherSync([&service]() {
    fidl::Result vmo = fidl::Call(*service)->Get();
    uint64_t vmo_size{};
    ASSERT_OK(vmo->vmo().get_size(&vmo_size));
    // Size should match value reported by the fake backend.
    EXPECT_EQ(vmo_size, FakeBackendForPmem::kFakeSize);
  });
  ASSERT_OK(run_result.status_value());
}

TEST_F(PmemTestDriver, CachePolicy) {
  zx::result service = driver_test().Connect<fuchsia_hardware_virtio_pmem::Service::Device>();
  ASSERT_OK(service.status_value());
  zx::result run_result = driver_test().RunOnBackgroundDispatcherSync([&service]() {
    fidl::Result vmo = fidl::Call(*service)->Get();
    zx_info_vmo_t vmo_info{};
    ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &vmo_info, sizeof(vmo_info), nullptr, nullptr));
    // VMO should have a cache policy of "cached".
    EXPECT_EQ(vmo_info.cache_policy, ZX_CACHE_POLICY_CACHED);
  });
  ASSERT_OK(run_result.status_value());
}

FUCHSIA_DRIVER_EXPORT2(TestPmemDriver);

}  // namespace
