// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include <vector>

#include <gmock/gmock.h>

#include "src/performance/trace_manager/tests/trace_manager_test.h"

namespace tracing {
namespace test {

TEST_F(TraceManagerTest, InitToFini) {
  ConnectToProvisionerService();

  FakeProvider* provider;
  ASSERT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name, &provider));
  EXPECT_EQ(fake_provider_bindings().size(), 1u);

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());
  VerifyCounts(1, 0);

  ASSERT_TRUE(StopSession());
  VerifyCounts(1, 1);

  ASSERT_TRUE(StartSession());
  VerifyCounts(2, 1);

  ASSERT_TRUE(StopSession());
  VerifyCounts(2, 2);

  ASSERT_TRUE(TerminateSession());
  VerifyCounts(2, 2);
}

TEST_F(TraceManagerTest, InitToFiniWithNoProviders) {
  ConnectToProvisionerService();

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());
  VerifyCounts(1, 0);

  ASSERT_TRUE(StopSession());
  VerifyCounts(1, 1);

  ASSERT_TRUE(StartSession());
  VerifyCounts(2, 1);

  ASSERT_TRUE(StopSession());
  VerifyCounts(2, 2);

  ASSERT_TRUE(TerminateSession());
  VerifyCounts(2, 2);
}

TEST_F(TraceManagerTest, InitToFiniWithProviderAddedAfterSessionStarts) {
  ConnectToProvisionerService();

  FakeProvider* provider1;
  ASSERT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name, &provider1));
  EXPECT_EQ(fake_provider_bindings().size(), 1u);

  const std::string kDuplicatedCategory = "this_should_be_deduped";

  fuchsia_tracing_controller::ProviderSpec provider1_spec;
  provider1_spec.name(kProvider1Name);
  provider1_spec.categories({{"union", "sammamish", "washington", kDuplicatedCategory}});
  std::vector<fuchsia_tracing_controller::ProviderSpec> provider_specs;
  provider_specs.push_back(std::move(provider1_spec));

  fuchsia_tracing_controller::ProviderSpec provider2_spec;
  provider2_spec.name(kProvider2Name);
  provider2_spec.categories({{"rainier", "baker", "stuart", kDuplicatedCategory}});
  provider_specs.push_back(std::move(provider2_spec));

  auto trace_config = GetDefaultTraceConfig();
  trace_config.categories()->push_back(kDuplicatedCategory);
  trace_config.provider_specs(std::move(provider_specs));
  ASSERT_TRUE(InitializeSession(std::move(trace_config)));

  EXPECT_THAT(provider1->GetEnabledCategories(),
              ::testing::UnorderedElementsAreArray(std::vector<std::string>{
                  "union", "sammamish", "washington", kDuplicatedCategory, kTestUmbrellaCategory}));

  ASSERT_TRUE(StartSession());
  VerifyCounts(1, 0);

  FakeProvider* provider2;
  ASSERT_TRUE(AddFakeProvider(kProvider2Pid, kProvider2Name, &provider2));
  EXPECT_EQ(fake_provider_bindings().size(), 2u);

  // Given the session a chance to start the new provider before we stop it.
  RunLoopUntilIdle();
  EXPECT_THAT(provider2->GetEnabledCategories(),
              ::testing::UnorderedElementsAreArray(std::vector<std::string>{
                  "rainier", "baker", "stuart", kDuplicatedCategory, kTestUmbrellaCategory}));
  ASSERT_EQ(provider2->state(), FakeProvider::State::kStarting);
  provider2->MarkStarted();
  // Give TraceSession a chance to process the STARTED fifo packet.
  RunLoopUntilIdle();

  ASSERT_TRUE(StopSession());
  VerifyCounts(1, 1);

  ASSERT_TRUE(StartSession());
  VerifyCounts(2, 1);

  ASSERT_TRUE(StopSession());
  VerifyCounts(2, 2);

  ASSERT_TRUE(TerminateSession());
  VerifyCounts(2, 2);
}

TEST_F(TraceManagerTest, InitToFiniWithNoStop) {
  ConnectToProvisionerService();

  FakeProvider* provider;
  ASSERT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name, &provider));
  EXPECT_EQ(fake_provider_bindings().size(), 1u);

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());
  VerifyCounts(1, 0);

  ASSERT_TRUE(TerminateSession());
  VerifyCounts(1, 0);
}

static constexpr char kAlertName[] = "alert-name";
// min length
static constexpr char kAlertNameMin[] = "a";
// max length
static constexpr char kAlertNameMax[] = "alert-name-max";

// Tests alerts with names of various lengths.
TEST_F(TraceManagerTest, Alerted) {
  ConnectToProvisionerService();

  FakeProvider* provider;
  ASSERT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name, &provider));
  EXPECT_EQ(fake_provider_bindings().size(), 1u);

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());
  VerifyCounts(1, 0);

  // Intermediate-length alert name (10 characters).
  provider->SendAlert(kAlertName);
  std::string received_alert_name;
  session_client()->WatchAlert().ThenExactlyOnce([&received_alert_name](auto& result) {
    ASSERT_TRUE(result.is_ok());
    received_alert_name = result.value().alert_name();
  });
  RunLoopUntilIdle();
  ASSERT_EQ(kAlertName, received_alert_name);

  // Minimum-length alert name (1 character).
  provider->SendAlert(kAlertNameMin);
  received_alert_name.clear();
  session_client()->WatchAlert().ThenExactlyOnce([&received_alert_name](auto& result) {
    ASSERT_TRUE(result.is_ok());
    received_alert_name = result.value().alert_name();
  });
  RunLoopUntilIdle();
  ASSERT_EQ(kAlertNameMin, received_alert_name);

  // Maximum-length alert name (14 characters).
  provider->SendAlert(kAlertNameMax);
  received_alert_name.clear();
  session_client()->WatchAlert().ThenExactlyOnce([&received_alert_name](auto& result) {
    ASSERT_TRUE(result.is_ok());
    received_alert_name = result.value().alert_name();
  });
  RunLoopUntilIdle();
  ASSERT_EQ(kAlertNameMax, received_alert_name);

  ASSERT_TRUE(StopSession());
  VerifyCounts(1, 1);

  ASSERT_TRUE(TerminateSession());
  VerifyCounts(1, 1);
}

static constexpr size_t kMaxAlertQueueDepth = 16;

// Tests alerts with a variety of sequences WRT |WatchAlert|.
TEST_F(TraceManagerTest, AlertSequence) {
  ConnectToProvisionerService();

  FakeProvider* provider;
  ASSERT_TRUE(AddFakeProvider(kProvider1Pid, kProvider1Name, &provider));
  EXPECT_EQ(fake_provider_bindings().size(), 1u);

  ASSERT_TRUE(InitializeSession());

  ASSERT_TRUE(StartSession());
  VerifyCounts(1, 0);

  // Calling |WatchAlert| before sending alert.
  std::string received_alert_name;
  session_client()->WatchAlert().ThenExactlyOnce([&received_alert_name](auto& result) {
    ASSERT_TRUE(result.is_ok());
    received_alert_name = result.value().alert_name();
  });
  RunLoopUntilIdle();
  ASSERT_EQ("", received_alert_name);
  provider->SendAlert(kAlertName);
  RunLoopUntilIdle();
  ASSERT_EQ(kAlertName, received_alert_name);

  // Sending multiple alerts before watching alerts.
  for (uint8_t i = 0; i < 4; ++i) {
    std::string alert_name = kAlertName;
    alert_name.append(1, 'A' + i);
    provider->SendAlert(alert_name.c_str());
  }

  for (uint8_t i = 0; i < 4; ++i) {
    received_alert_name.clear();
    session_client()->WatchAlert().ThenExactlyOnce([&received_alert_name](auto& result) {
      ASSERT_TRUE(result.is_ok());
      received_alert_name = result.value().alert_name();
    });
    RunLoopUntilIdle();
    std::string alert_name = kAlertName;
    alert_name.append(1, 'A' + i);
    ASSERT_EQ(alert_name, received_alert_name);
  }

  // Sending more than 16 alerts before watching alerts. The oldest alerts are discarded.
  for (uint8_t i = 0; i < kMaxAlertQueueDepth + 2; ++i) {
    std::string alert_name = kAlertName;
    alert_name.append(1, 'A' + i);
    provider->SendAlert(alert_name.c_str());
    RunLoopUntilIdle();
  }

  for (uint8_t i = 2; i < kMaxAlertQueueDepth + 2; ++i) {
    received_alert_name.clear();
    session_client()->WatchAlert().ThenExactlyOnce([&received_alert_name](auto& result) {
      ASSERT_TRUE(result.is_ok());
      received_alert_name = result.value().alert_name();
    });
    RunLoopUntilIdle();
    std::string alert_name = kAlertName;
    alert_name.append(1, 'A' + i);
    ASSERT_EQ(alert_name, received_alert_name);
  }

  ASSERT_TRUE(StopSession());
  VerifyCounts(1, 1);

  ASSERT_TRUE(TerminateSession());
  VerifyCounts(1, 1);
}

}  // namespace test
}  // namespace tracing
