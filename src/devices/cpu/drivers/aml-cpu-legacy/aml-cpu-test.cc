// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-cpu.h"

#include <fidl/fuchsia.hardware.thermal/cpp/wire.h>
#include <fuchsia/hardware/thermal/cpp/banjo.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>

#include <algorithm>
#include <memory>
#include <vector>

#include <ddktl/device.h>
#include <ddktl/fidl.h>
#include <fbl/array.h>
#include <sdk/lib/inspect/testing/cpp/zxtest/inspect.h>
#include <soc/aml-common/aml-cpu-metadata.h>
#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"

namespace amlogic_cpu {

// Fake MMIO  that exposes CPU version.
class FakeMmio {
 public:
  FakeMmio() : mmio_(sizeof(uint32_t), kRegCount) {
    mmio_[kCpuVersionOffset].SetReadCallback([]() { return kCpuVersion; });
  }

  fdf::MmioBuffer mmio() { return mmio_.GetMmioBuffer(); }

 private:
  static constexpr size_t kCpuVersionOffset = 0x220;
  static constexpr size_t kRegCount = kCpuVersionOffset / sizeof(uint32_t) + 1;

  // Note: FakeMmioReg's read callback returns a uint64_t, which is then cast to uint32_t when
  // AmlCpu calls FakeMmioRegRegion::Read32.
  constexpr static uint64_t kCpuVersion = 43;

  fake_mmio::FakeMmioRegRegion mmio_;
};

using CpuCtrlSyncClient = fidl::WireSyncClient<fuchsia_cpuctrl::Device>;
using ThermalSyncClient = fidl::WireSyncClient<fuchsia_thermal::Device>;

constexpr size_t kBigClusterIdx =
    static_cast<size_t>(fuchsia_thermal::wire::PowerDomain::kBigClusterPowerDomain);
constexpr size_t kLittleClusterIdx =
    static_cast<size_t>(fuchsia_thermal::wire::PowerDomain::kLittleClusterPowerDomain);

constexpr uint32_t kBigClusterCoreCount = 4;
constexpr uint32_t kLittleClusterCoreCount = 2;

constexpr uint8_t kBigClusterRelativePerformance = 255;
constexpr uint8_t kLittleClusterRelativePerformance = 112;

constexpr legacy_cluster_info_t kClusterSizeMetadata[] = {
    {
        .pd_id = kBigClusterIdx,
        .core_count = kBigClusterCoreCount,
        .relative_performance = kBigClusterRelativePerformance,
    },
    {
        .pd_id = kLittleClusterIdx,
        .core_count = kLittleClusterCoreCount,
        .relative_performance = kLittleClusterRelativePerformance,
    },
};

constexpr size_t PowerDomainToIndex(fuchsia_thermal::wire::PowerDomain pd) {
  switch (pd) {
    case fuchsia_thermal::wire::PowerDomain::kLittleClusterPowerDomain:
      return kLittleClusterIdx;
    case fuchsia_thermal::wire::PowerDomain::kBigClusterPowerDomain:
      return kBigClusterIdx;
  }
  __UNREACHABLE;
}

const fuchsia_thermal::wire::OperatingPoint kFakeOperatingPoints = []() {
  fuchsia_thermal::wire::OperatingPoint result;

  result.count = 3;
  result.latency = 0;
  result.opp[0].volt_uv = 1;
  result.opp[0].freq_hz = 100;
  result.opp[1].volt_uv = 2;
  result.opp[1].freq_hz = 200;
  result.opp[2].volt_uv = 3;
  result.opp[2].freq_hz = 300;

  return result;
}();

const fuchsia_thermal::wire::ThermalDeviceInfo kDefaultDeviceInfo = []() {
  fuchsia_thermal::wire::ThermalDeviceInfo result;

  result.active_cooling = false;
  result.passive_cooling = false;
  result.gpu_throttling = false;
  result.num_trip_points = 0;
  result.big_little = false;
  result.critical_temp_celsius = 0;

  result.opps[kLittleClusterIdx].count = 0;
  result.opps[kBigClusterIdx] = kFakeOperatingPoints;

  return result;
}();

class FakeAmlThermal : public fidl::WireServer<fuchsia_thermal::Device> {
 public:
  FakeAmlThermal() : active_operating_point_(0), device_info_(kDefaultDeviceInfo) {}
  ~FakeAmlThermal() {}

  // Manage the Fake FIDL Message Loop
  zx_status_t Init(fidl::ServerEnd<fuchsia_thermal::Device> remote);

  // Accessor
  uint16_t ActiveOperatingPoint() const { return active_operating_point_; }

  void DdkRelease() {}

  void set_device_info(const fuchsia_thermal::wire::ThermalDeviceInfo& device_info) {
    device_info_ = device_info;
  }

 private:
  // Implement Thermal FIDL Protocol.
  void GetInfo(GetInfoCompleter::Sync& completer) override;
  void GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) override;
  void GetDvfsInfo(GetDvfsInfoRequestView request, GetDvfsInfoCompleter::Sync& completer) override;
  void GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) override;
  void GetSensorName(GetSensorNameCompleter::Sync& completer) override {}
  void GetStateChangeEvent(GetStateChangeEventCompleter::Sync& completer) override;
  void GetStateChangePort(GetStateChangePortCompleter::Sync& completer) override;
  void SetTripCelsius(SetTripCelsiusRequestView request,
                      SetTripCelsiusCompleter::Sync& completer) override;
  void GetDvfsOperatingPoint(GetDvfsOperatingPointRequestView request,
                             GetDvfsOperatingPointCompleter::Sync& completer) override;
  void SetDvfsOperatingPoint(SetDvfsOperatingPointRequestView request,
                             SetDvfsOperatingPointCompleter::Sync& completer) override;
  void GetFanLevel(GetFanLevelCompleter::Sync& completer) override;
  void SetFanLevel(SetFanLevelRequestView request, SetFanLevelCompleter::Sync& completer) override;

  uint16_t active_operating_point_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  fuchsia_thermal::wire::ThermalDeviceInfo device_info_;
};

zx_status_t FakeAmlThermal::Init(fidl::ServerEnd<fuchsia_thermal::Device> request) {
  loop_.StartThread("fake-aml-thermal");
  fidl::BindServer(loop_.dispatcher(), std::move(request), this, nullptr);
  return ZX_OK;
}

void FakeAmlThermal::GetInfo(GetInfoCompleter::Sync& completer) {
  fuchsia_thermal::wire::ThermalInfo result;

  result.state = 0;
  result.passive_temp_celsius = 0;
  result.critical_temp_celsius = 0;
  result.max_trip_count = 0;

  completer.Reply(ZX_OK,
                  fidl::ObjectView<fuchsia_thermal::wire::ThermalInfo>::FromExternal(&result));
}

void FakeAmlThermal::GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) {
  fuchsia_thermal::wire::ThermalDeviceInfo result = device_info_;
  completer.Reply(
      ZX_OK, fidl::ObjectView<fuchsia_thermal::wire::ThermalDeviceInfo>::FromExternal(&result));
}

void FakeAmlThermal::GetDvfsInfo(GetDvfsInfoRequestView request,
                                 GetDvfsInfoCompleter::Sync& completer) {
  fuchsia_thermal::wire::ThermalDeviceInfo device_info = device_info_;
  fuchsia_thermal::wire::OperatingPoint result =
      device_info.opps[PowerDomainToIndex(request->power_domain)];
  completer.Reply(ZX_OK,
                  fidl::ObjectView<fuchsia_thermal::wire::OperatingPoint>::FromExternal(&result));
}

void FakeAmlThermal::GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) {
  completer.Reply(ZX_OK, 0.0);
}

void FakeAmlThermal::GetStateChangeEvent(GetStateChangeEventCompleter::Sync& completer) {
  zx::event invalid;
  completer.Reply(ZX_ERR_NOT_SUPPORTED, std::move(invalid));
}

void FakeAmlThermal::GetStateChangePort(GetStateChangePortCompleter::Sync& completer) {
  zx::port invalid;
  completer.Reply(ZX_ERR_NOT_SUPPORTED, std::move(invalid));
}

void FakeAmlThermal::SetTripCelsius(SetTripCelsiusRequestView request,
                                    SetTripCelsiusCompleter::Sync& completer) {
  completer.Reply(ZX_ERR_NOT_SUPPORTED);
}

void FakeAmlThermal::GetDvfsOperatingPoint(GetDvfsOperatingPointRequestView request,
                                           GetDvfsOperatingPointCompleter::Sync& completer) {
  if (request->power_domain == fuchsia_thermal::wire::PowerDomain::kLittleClusterPowerDomain) {
    completer.Reply(ZX_ERR_NOT_SUPPORTED, 0);
    return;
  }

  completer.Reply(ZX_OK, active_operating_point_);
}

void FakeAmlThermal::SetDvfsOperatingPoint(SetDvfsOperatingPointRequestView request,
                                           SetDvfsOperatingPointCompleter::Sync& completer) {
  if (request->power_domain == fuchsia_thermal::wire::PowerDomain::kLittleClusterPowerDomain) {
    completer.Reply(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  active_operating_point_ = request->op_idx;
  completer.Reply(ZX_OK);
}

void FakeAmlThermal::GetFanLevel(GetFanLevelCompleter::Sync& completer) {
  completer.Reply(ZX_ERR_NOT_SUPPORTED, 0);
}

void FakeAmlThermal::SetFanLevel(SetFanLevelRequestView request,
                                 SetFanLevelCompleter::Sync& completer) {
  completer.Reply(ZX_ERR_OUT_OF_RANGE);
}

// Fake device that exposes the thermal banjo protocol. Upon calling Connect, a new instance of
// FakeAmlThermal is created to serve a client, at which point any previous FakeThermalAml
// instance is destroyed.
class FakeThermalDevice : public ddk::ThermalProtocol<FakeThermalDevice, ddk::base_protocol> {
 public:
  FakeThermalDevice()
      : proto_({&thermal_protocol_ops_, this}), device_info_(kDefaultDeviceInfo), fidl_service_() {}

  zx_status_t ThermalConnect(zx::channel chan) {
    fidl_service_ = std::make_unique<FakeAmlThermal>();
    fidl_service_->set_device_info(device_info_);
    return fidl_service_->Init(fidl::ServerEnd<fuchsia_thermal::Device>(std::move(chan)));
  }

  const thermal_protocol_t* proto() const { return &proto_; }

  void set_device_info(const fuchsia_thermal::wire::ThermalDeviceInfo& device_info) {
    device_info_ = device_info;
  }

 private:
  thermal_protocol_t proto_;
  fuchsia_thermal::wire::ThermalDeviceInfo device_info_;
  std::unique_ptr<FakeAmlThermal> fidl_service_;
};

struct IncomingNamespace {
  fdf_fake::FakePDev pdev_server;
  component::OutgoingDirectory outgoing{async_get_default_dispatcher()};
};

// Fixture that supports tests of AmlCpu::Create.
class AmlCpuBindingTest : public zxtest::Test {
 public:
  AmlCpuBindingTest() {
    root_ = MockDevice::FakeRootParent();

    ASSERT_OK(incoming_loop_.StartThread("incoming-ns-thread"));

    fdf_fake::FakePDev::Config config;
    config.mmios[0] = mmio_.mmio();
    auto outgoing_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    incoming_.SyncCall([config = std::move(config), server = std::move(outgoing_endpoints.server)](
                           IncomingNamespace* infra) mutable {
      infra->pdev_server.SetConfig(std::move(config));
      ASSERT_OK(infra->outgoing.AddService<fuchsia_hardware_platform_device::Service>(
          infra->pdev_server.GetInstanceHandler()));
      ASSERT_OK(infra->outgoing.Serve(std::move(server)));
    });
    ASSERT_NO_FATAL_FAILURE();
    root_->AddFidlService(fuchsia_hardware_platform_device::Service::Name,
                          std::move(outgoing_endpoints.client), "pdev");

    root_->AddProtocol(ZX_PROTOCOL_THERMAL, thermal_device_.proto()->ops,
                       thermal_device_.proto()->ctx, "thermal");

    root_->SetMetadata(DEVICE_METADATA_CLUSTER_SIZE_LEGACY, &kClusterSizeMetadata,
                       sizeof(kClusterSizeMetadata));
  }

 protected:
  std::shared_ptr<MockDevice> root_;
  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_{incoming_loop_.dispatcher(),
                                                                   std::in_place};
  FakeMmio mmio_;
  FakeThermalDevice thermal_device_;
};

TEST_F(AmlCpuBindingTest, OneDomain) {
  ASSERT_OK(AmlCpu::Create(nullptr, root_.get()));
  ASSERT_EQ(root_->child_count(), 1);
}

TEST_F(AmlCpuBindingTest, TwoDomains) {
  // Set up device info that defines two power domains.
  thermal_device_.set_device_info([]() {
    fuchsia_thermal::wire::ThermalDeviceInfo result;

    result.active_cooling = false;
    result.passive_cooling = false;
    result.gpu_throttling = false;
    result.num_trip_points = 0;
    result.big_little = true;
    result.critical_temp_celsius = 0;

    result.opps[kLittleClusterIdx] = kFakeOperatingPoints;
    result.opps[kBigClusterIdx] = kFakeOperatingPoints;

    return result;
  }());

  ASSERT_OK(AmlCpu::Create(nullptr, root_.get()));
  ASSERT_EQ(root_->child_count(), 2);

  const auto& devices = root_->children();
  for (const auto& zxdev : devices) {
    AmlCpu* device = zxdev->GetDeviceContext<AmlCpu>();
    const size_t idx = device->PowerDomainIndex();

    // Find the cluster metadata that corresponds to this cluster index.
    const auto& cluster_size_meta_itr = std::find_if(
        std::begin(kClusterSizeMetadata), std::end(kClusterSizeMetadata),
        [idx](const legacy_cluster_info_t& elem) -> bool { return idx == elem.pd_id; });

    ASSERT_NE(cluster_size_meta_itr, std::end(kClusterSizeMetadata));
    ASSERT_EQ(cluster_size_meta_itr->core_count, device->ClusterCoreCount());
  }
}

class AmlCpuTest : public AmlCpu {
 public:
  AmlCpuTest(ThermalSyncClient thermal)
      : AmlCpu(nullptr, std::move(thermal), kBigClusterIdx, kBigClusterCoreCount,
               kBigClusterRelativePerformance) {}

  zx::vmo inspect_vmo() { return inspector_.DuplicateVmo(); }
};

using inspect::InspectTestHelper;

class AmlCpuTestFixture : public InspectTestHelper, public zxtest::Test {
 public:
  explicit AmlCpuTestFixture() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {}
  void SetUp() override;
  void TearDown() override { loop_.Shutdown(); }

 protected:
  FakeAmlThermal thermal_;

  async::Loop loop_;
  std::shared_ptr<MockDevice> parent_;
  std::unique_ptr<AmlCpuTest> dut_;
  CpuCtrlSyncClient cpu_client_;
};

void AmlCpuTestFixture::SetUp() {
  parent_ = MockDevice::FakeRootParent();

  auto thermal_eps = fidl::Endpoints<fuchsia_thermal::Device>::Create();

  ASSERT_OK(thermal_.Init(std::move(thermal_eps.server)));
  ThermalSyncClient thermal_client = fidl::WireSyncClient(std::move(thermal_eps.client));

  dut_ = std::make_unique<AmlCpuTest>(std::move(thermal_client));

  auto cpu_eps = fidl::Endpoints<fuchsia_cpuctrl::Device>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(cpu_eps.server), dut_.get());
  loop_.StartThread("aml-cpu-legacy-test-thread");

  cpu_client_ = CpuCtrlSyncClient(std::move(cpu_eps.client));
}

TEST_F(AmlCpuTestFixture, TestGetOperatingPointInfo) {
  // Make sure that we can get information about all the supported opps.
  for (uint32_t i = 0; i < kFakeOperatingPoints.count; i++) {
    auto oppInfo = cpu_client_->GetOperatingPointInfo(i);

    // First, make sure there were no transport errors.
    ASSERT_OK(oppInfo.status());

    // Then make sure that the driver accepted the call.
    ASSERT_FALSE(oppInfo->is_error());

    // Then make sure that we're getting the accepted frequency and voltage values.
    EXPECT_EQ(oppInfo->value()->info.frequency_hz,
              kFakeOperatingPoints.opp[kFakeOperatingPoints.count - i - 1].freq_hz);
    EXPECT_EQ(oppInfo->value()->info.voltage_uv,
              kFakeOperatingPoints.opp[kFakeOperatingPoints.count - i - 1].volt_uv);
  }

  // Make sure that we can't get any information about opps that don't exist.
  for (uint32_t i = kFakeOperatingPoints.count; i < kFakeOperatingPoints.count + 10; i++) {
    auto oppInfo = cpu_client_->GetOperatingPointInfo(i);

    // Even if it's an unsupported opp, we still expect the transport to
    // deliver the message successfully.
    ASSERT_OK(oppInfo.status());

    // Make sure that the driver returns an error, however.
    EXPECT_TRUE(oppInfo->is_error());
  }
}

TEST_F(AmlCpuTestFixture, TestSetCurrentOperatingPoint) {
  // Make sure that we can drive the CPU to all of the supported operating
  // points.
  for (uint32_t i = 0; i < kFakeOperatingPoints.count; i++) {
    uint32_t out_state = UINT32_MAX;
    zx_status_t st = dut_->SetCurrentOperatingPointInternal(i, &out_state);

    // Make sure the call succeeded.
    EXPECT_OK(st);

    // Make sure we could actually drive the device into the state that we
    // expected.
    EXPECT_EQ(out_state, i);

    // Make sure that the call was forwarded to the thermal driver.
    const uint16_t kExpectedOperatingPoint =
        static_cast<uint16_t>(kFakeOperatingPoints.count - i - 1);
    EXPECT_EQ(kExpectedOperatingPoint, thermal_.ActiveOperatingPoint());
  }

  // Next make sure that we can't drive the CPU into any unsupported
  // operating points.
  for (uint32_t i = kFakeOperatingPoints.count; i < kFakeOperatingPoints.count + 10; i++) {
    const uint16_t kInitialOperatingPoint = thermal_.ActiveOperatingPoint();
    uint32_t out_state = UINT32_MAX;
    zx_status_t st = dut_->SetCurrentOperatingPointInternal(i, &out_state);

    // This is not a supported operating point.
    EXPECT_NOT_OK(st);

    // Make sure we haven't meddled with `out_state`
    EXPECT_EQ(out_state, UINT32_MAX);

    // Make sure we haven't meddled with the thermal driver's active
    // operating point.
    EXPECT_EQ(kInitialOperatingPoint, thermal_.ActiveOperatingPoint());
  }
}

TEST_F(AmlCpuTestFixture, TestSetCpuInfo) {
  uint32_t test_cpu_version = 0x28200b02;
  dut_->SetCpuInfo(test_cpu_version);
  ASSERT_NO_FATAL_FAILURE(ReadInspect(dut_->inspect_vmo()));
  auto* cpu_info = hierarchy().GetByPath({"cpu_info_service"});
  ASSERT_TRUE(cpu_info);

  // cpu_major_revision : 40
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(cpu_info->node(), "cpu_major_revision", inspect::UintPropertyValue(40)));
  // cpu_minor_revision : 11
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(cpu_info->node(), "cpu_minor_revision", inspect::UintPropertyValue(11)));
  // cpu_package_id : 2
  ASSERT_NO_FATAL_FAILURE(
      CheckProperty(cpu_info->node(), "cpu_package_id", inspect::UintPropertyValue(2)));
}

TEST_F(AmlCpuTestFixture, TestGetOperatingPointCount) {
  auto resp = cpu_client_->GetOperatingPointCount();

  ASSERT_OK(resp.status());

  EXPECT_EQ(resp.value()->count, kFakeOperatingPoints.count);
}

TEST_F(AmlCpuTestFixture, TestGetNumLogicalCores) {
  auto resp = cpu_client_->GetNumLogicalCores();

  ASSERT_OK(resp.status());

  EXPECT_EQ(resp.value().count, kBigClusterCoreCount);
}

// Tests that GetRelativePerformance and GetRelativePerformance2 return the
// expected relative performance values configured for the domain.
TEST_F(AmlCpuTestFixture, TestGetRelativePerformance) {
  {
    auto resp = cpu_client_->GetRelativePerformance();
    ASSERT_OK(resp.status());
    ASSERT_FALSE(resp->is_error());
    EXPECT_EQ(resp->value()->relative_performance, kBigClusterRelativePerformance);
  }

  {
    auto resp = cpu_client_->GetRelativePerformance2();
    ASSERT_OK(resp.status());
    ASSERT_FALSE(resp->is_error());
    EXPECT_EQ(resp->value()->relative_performance, kBigClusterRelativePerformance);
  }
}

}  // namespace amlogic_cpu
