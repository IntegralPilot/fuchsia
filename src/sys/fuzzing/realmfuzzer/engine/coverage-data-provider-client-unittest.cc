// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/realmfuzzer/engine/coverage-data-provider-client.h"

#include <fuchsia/fuzzer/cpp/fidl.h>
#include <lib/fidl/cpp/binding.h>
#include <zircon/status.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/sys/fuzzing/common/async-deque.h"
#include "src/sys/fuzzing/common/async-eventpair.h"
#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/options.h"
#include "src/sys/fuzzing/common/testing/async-test.h"
#include "src/sys/fuzzing/realmfuzzer/testing/coverage.h"
#include "src/sys/fuzzing/realmfuzzer/testing/module.h"

namespace fuzzing {

using fuchsia::fuzzer::Data;
using fuchsia::fuzzer::InstrumentedProcess;

// Test fixtures.

class CoverageDataProviderClientTest : public AsyncTest {
 protected:
  void SetUp() override {
    AsyncTest::SetUp();
    coverage_ = std::make_unique<FakeCoverage>(executor());
  }

  std::unique_ptr<CoverageDataProviderClient> GetProviderClient() {
    auto client = std::make_unique<CoverageDataProviderClient>(executor());
    client->Bind(coverage_->GetProviderHandler());
    return client;
  }

  OptionsPtr GetOptions() const { return coverage_->options(); }

  void Pend(CoverageData coverage_data) { coverage_->Send(std::move(coverage_data)); }

 private:
  std::unique_ptr<FakeCoverage> coverage_;
};

// Unit tests.

TEST_F(CoverageDataProviderClientTest, SetOptions) {
  auto provider_client = GetProviderClient();

  auto options = MakeOptions();
  options->set_runs(3);
  provider_client->Configure(options);
  RunOnce();

  EXPECT_EQ(GetOptions()->runs(), 3U);
}

TEST_F(CoverageDataProviderClientTest, GetProcess) {
  auto provider_client = GetProviderClient();
  CoverageData coverage;
  FUZZING_EXPECT_OK(provider_client->GetCoverageData(), &coverage);

  auto self = zx::process::self();
  zx_info_handle_basic_t info;
  EXPECT_EQ(self->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr), ZX_OK);
  auto koid = info.koid;

  zx::process process;
  EXPECT_EQ(self->duplicate(ZX_RIGHT_SAME_RIGHTS, &process), ZX_OK);
  AsyncEventPair eventpair(executor());
  Pend(CoverageData{
      .target_id = zx_koid_t(1),
      .data = Data::WithInstrumented(InstrumentedProcess{
          .eventpair = eventpair.Create(),
          .process = std::move(process),
      }),
  });
  RunUntilIdle();

  ASSERT_TRUE(coverage.data.is_instrumented());
  auto& received = coverage.data.instrumented();
  EXPECT_EQ(received.process.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr),
            ZX_OK);
  EXPECT_EQ(koid, info.koid);
  FUZZING_EXPECT_OK(eventpair.WaitFor(kSync));
  EXPECT_EQ(received.eventpair.signal_peer(0, kSync), ZX_OK);
  RunUntilIdle();
}

TEST_F(CoverageDataProviderClientTest, GetModule) {
  auto provider_client = GetProviderClient();
  CoverageData coverage;

  zx::vmo counters;
  char name[ZX_MAX_NAME_LEN];

  // Send multiple, and verify they arrive in order.
  FakeRealmFuzzerModule module1(1);
  EXPECT_EQ(module1.Share(&counters), ZX_OK);
  Pend(CoverageData{
      .target_id = zx_koid_t(1),
      .data = Data::WithInline8bitCounters(std::move(counters)),
  });

  FakeRealmFuzzerModule module2(1);
  EXPECT_EQ(module2.Share(&counters), ZX_OK);
  Pend(CoverageData{
      .target_id = zx_koid_t(2),
      .data = Data::WithInline8bitCounters(std::move(counters)),
  });

  FUZZING_EXPECT_OK(provider_client->GetCoverageData(), &coverage);
  RunUntilIdle();
  EXPECT_EQ(coverage.target_id, zx_koid_t(1));
  ASSERT_TRUE(coverage.data.is_inline_8bit_counters());
  auto& inline_8bit_counters1 = coverage.data.inline_8bit_counters();
  EXPECT_EQ(inline_8bit_counters1.get_property(ZX_PROP_NAME, name, sizeof(name)), ZX_OK);
  EXPECT_EQ(name, module1.id());

  FUZZING_EXPECT_OK(provider_client->GetCoverageData(), &coverage);
  RunUntilIdle();
  EXPECT_EQ(coverage.target_id, zx_koid_t(2));
  ASSERT_TRUE(coverage.data.is_inline_8bit_counters());
  auto& inline_8bit_counters2 = coverage.data.inline_8bit_counters();
  EXPECT_EQ(inline_8bit_counters2.get_property(ZX_PROP_NAME, name, sizeof(name)), ZX_OK);
  EXPECT_EQ(name, module2.id());

  // Intentionally drop a |GetCoverageData| future and ensure no data is lost.
  FakeRealmFuzzerModule module3(3);
  {
    auto dropped = provider_client->GetCoverageData();
    RunOnce();
    EXPECT_EQ(module3.Share(&counters), ZX_OK);
    Pend(CoverageData{
        .target_id = zx_koid_t(3),
        .data = Data::WithInline8bitCounters(std::move(counters)),
    });
  }

  FUZZING_EXPECT_OK(provider_client->GetCoverageData(), &coverage);
  RunUntilIdle();
  EXPECT_EQ(coverage.target_id, zx_koid_t(3));
  ASSERT_TRUE(coverage.data.is_inline_8bit_counters());
  auto& inline_8bit_counters3 = coverage.data.inline_8bit_counters();
  EXPECT_EQ(inline_8bit_counters3.get_property(ZX_PROP_NAME, name, sizeof(name)), ZX_OK);
  EXPECT_EQ(name, module3.id());
}

}  // namespace fuzzing
