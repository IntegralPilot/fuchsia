// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.cpu.ctrl/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/wire/channel.h>

#include <zxtest/zxtest.h>

#include "performance-domain.h"

namespace {

constexpr cpuctrl::wire::CpuOperatingPointInfo kTestOpps[] = {
    {.frequency_hz = 1000, .voltage_uv = 100}, {.frequency_hz = 800, .voltage_uv = 90},
    {.frequency_hz = 600, .voltage_uv = 80},   {.frequency_hz = 400, .voltage_uv = 70},
    {.frequency_hz = 200, .voltage_uv = 60},
};

constexpr uint32_t kInitialOperatingPoint = 0;

constexpr uint32_t kNumLogicalCores = 4;

constexpr uint8_t kDefaultRelativePerformance = 255;
constexpr uint64_t kDefaultRelativePerformance2 = 500;

constexpr uint32_t kDefaultDomainId = 1;

constexpr uint64_t kLogicalCoreIds[kNumLogicalCores] = {1, 2, 3, 4};

class FakeCpuDevice : public fidl::testing::WireTestBase<cpuctrl::Device> {
 public:
  unsigned int OppSetCount() const { return opp_set_count_; }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ADD_FAILURE("unexpected call to %s", name.c_str());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  void SetCurrentOperatingPoint(SetCurrentOperatingPointRequestView request,
                                SetCurrentOperatingPointCompleter::Sync& completer) override;
  void GetCurrentOperatingPoint(GetCurrentOperatingPointCompleter::Sync& completer) override;
  void GetOperatingPointInfo(GetOperatingPointInfoRequestView request,
                             GetOperatingPointInfoCompleter::Sync& completer) override;
  void GetOperatingPointCount(GetOperatingPointCountCompleter::Sync& completer) override;
  void GetNumLogicalCores(GetNumLogicalCoresCompleter::Sync& completer) override;
  void GetLogicalCoreId(GetLogicalCoreIdRequestView request,
                        GetLogicalCoreIdCompleter::Sync& completer) override;
  void GetDomainId(GetDomainIdCompleter::Sync& completer) override;
  void GetRelativePerformance(GetRelativePerformanceCompleter::Sync& completer) override;
  void GetRelativePerformance2(GetRelativePerformance2Completer::Sync& completer) override;

  uint32_t current_opp_ = kInitialOperatingPoint;
  unsigned int opp_set_count_ = 0;
};

void FakeCpuDevice::GetOperatingPointInfo(GetOperatingPointInfoRequestView request,
                                          GetOperatingPointInfoCompleter::Sync& completer) {
  if (request->opp >= std::size(kTestOpps)) {
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
  } else {
    completer.ReplySuccess(kTestOpps[request->opp]);
  }
}

void FakeCpuDevice::GetOperatingPointCount(GetOperatingPointCountCompleter::Sync& completer) {
  completer.ReplySuccess(std::size(kTestOpps));
}

void FakeCpuDevice::GetNumLogicalCores(GetNumLogicalCoresCompleter::Sync& completer) {
  completer.Reply(kNumLogicalCores);
}

void FakeCpuDevice::GetLogicalCoreId(GetLogicalCoreIdRequestView request,
                                     GetLogicalCoreIdCompleter::Sync& completer) {
  if (request->index >= std::size(kLogicalCoreIds)) {
    completer.Reply(UINT64_MAX);
  }
  completer.Reply(kLogicalCoreIds[request->index]);
}

void FakeCpuDevice::SetCurrentOperatingPoint(SetCurrentOperatingPointRequestView request,
                                             SetCurrentOperatingPointCompleter::Sync& completer) {
  if (request->requested_opp > std::size(kTestOpps)) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  opp_set_count_++;
  current_opp_ = request->requested_opp;
  completer.ReplySuccess(request->requested_opp);
}

void FakeCpuDevice::GetCurrentOperatingPoint(GetCurrentOperatingPointCompleter::Sync& completer) {
  completer.Reply(current_opp_);
}

void FakeCpuDevice::GetDomainId(GetDomainIdCompleter::Sync& completer) {
  completer.Reply(kDefaultDomainId);
}

void FakeCpuDevice::GetRelativePerformance(GetRelativePerformanceCompleter::Sync& completer) {
  completer.ReplySuccess(kDefaultRelativePerformance);
}

void FakeCpuDevice::GetRelativePerformance2(GetRelativePerformance2Completer::Sync& completer) {
  completer.ReplySuccess(kDefaultRelativePerformance2);
}

class TestCpuPerformanceDomain : public CpuPerformanceDomain {
 public:
  // Permit Explicit Construction
  TestCpuPerformanceDomain(fidl::ClientEnd<cpuctrl::Device> cpu_client)
      : CpuPerformanceDomain(std::move(cpu_client)) {}
};

class PerformanceDomainTest : public zxtest::Test {
 public:
  void SetUp() override;

  FakeCpuDevice& cpu() { return cpu_; }
  TestCpuPerformanceDomain& pd() { return pd_.value(); }

 private:
  FakeCpuDevice cpu_;
  std::optional<TestCpuPerformanceDomain> pd_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

void PerformanceDomainTest::SetUp() {
  auto cpu_endpoints = fidl::Endpoints<fuchsia_hardware_cpu_ctrl::Device>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(cpu_endpoints.server),
                   static_cast<fidl::WireServer<cpuctrl::Device>*>(&cpu_));

  pd_.emplace(std::move(cpu_endpoints.client));
  ASSERT_OK(loop_.StartThread("performance-domain-test-fidl-thread"));
}

// Trivial Tests.
TEST_F(PerformanceDomainTest, TestNumLogicalCores) {
  const auto [core_count_status, core_count] = pd().GetNumLogicalCores();

  EXPECT_OK(core_count_status);
  EXPECT_EQ(core_count, kNumLogicalCores);
}

TEST_F(PerformanceDomainTest, TestGetCurrentOperatingPoint) {
  const auto [st, opp, opp_info] = pd().GetCurrentOperatingPoint();
  EXPECT_OK(st);
  EXPECT_EQ(opp, kInitialOperatingPoint);
  EXPECT_EQ(opp_info.frequency_hz, kTestOpps[kInitialOperatingPoint].frequency_hz);
  EXPECT_EQ(opp_info.voltage_uv, kTestOpps[kInitialOperatingPoint].voltage_uv);
}

TEST_F(PerformanceDomainTest, TestGetOperatingPoints) {
  const auto [st, opps] = pd().GetOperatingPoints();

  EXPECT_OK(st);

  ASSERT_EQ(opps.size(), std::size(kTestOpps));

  for (size_t i = 0; i < opps.size(); i++) {
    EXPECT_EQ(opps[i].voltage_uv, kTestOpps[i].voltage_uv);
    EXPECT_EQ(opps[i].frequency_hz, kTestOpps[i].frequency_hz);
  }
}

TEST_F(PerformanceDomainTest, TestSetCurrentOperatingPoint) {
  // Just move to the next sequential opp with wraparound.
  const uint32_t test_opp = (kInitialOperatingPoint + 1) % std::size(kTestOpps);
  const uint32_t invalid_opp = std::size(kTestOpps) + 1;
  zx_status_t st = pd().SetCurrentOperatingPoint(test_opp);

  EXPECT_OK(st);

  {
    const auto [res, new_opp, info] = pd().GetCurrentOperatingPoint();
    EXPECT_OK(res);
    EXPECT_EQ(new_opp, test_opp);
  }

  st = pd().SetCurrentOperatingPoint(invalid_opp);
  EXPECT_NOT_OK(st);

  {
    // Make sure the opp hasn't changed.
    const auto [res, new_opp, info] = pd().GetCurrentOperatingPoint();
    EXPECT_OK(res);
    EXPECT_EQ(new_opp, test_opp);
  }

  // Make sure there was exactly one successful call to SetCurrentOperatingPoint.
  EXPECT_EQ(cpu().OppSetCount(), 1);
}

TEST_F(PerformanceDomainTest, TestGetRelativePerformance) {
  const auto [relative_perf_status, relative_perf] = pd().GetRelativePerformance();

  EXPECT_OK(relative_perf_status);
  EXPECT_EQ(relative_perf, kDefaultRelativePerformance);
}

TEST_F(PerformanceDomainTest, TestGetRelativePerformance2) {
  const auto [relative_perf_status, relative_perf] = pd().GetRelativePerformance2();

  EXPECT_OK(relative_perf_status);
  EXPECT_EQ(relative_perf, kDefaultRelativePerformance2);
}

TEST_F(PerformanceDomainTest, TestGetDomainId) {
  const auto [domain_id_status, domain_id] = pd().GetDomainId();

  EXPECT_OK(domain_id_status);
  EXPECT_EQ(domain_id, kDefaultDomainId);
}

}  // namespace
