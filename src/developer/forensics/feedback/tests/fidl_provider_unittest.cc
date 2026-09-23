// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/fidl_provider.h"

#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <lib/vfs/cpp/service.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <memory>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/developer/forensics/testing/backoff.h"
#include "src/developer/forensics/testing/stubs/device_id_provider.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"

namespace forensics::feedback {
namespace {

using ::testing::IsEmpty;
using ::testing::Pair;
using ::testing::UnorderedElementsAreArray;

constexpr char kDeviceIdKey[] = "current_device_id";
constexpr char kDeviceIdValue[] = "device_id_1";

struct ConvertDeviceId {
  Annotations operator()(const fuchsia_feedback::DeviceIdProviderGetIdResponse& response) {
    return {{kDeviceIdKey, ErrorOrString(response.feedback_id())}};
  }

  Annotations operator()(const Error error) {
    return {{kDeviceIdKey, ErrorOrString(error)}};
  }
};

inline auto GetDeviceId(fidl::Client<fuchsia_feedback::DeviceIdProvider>& client) {
  return client->GetId();
}

using StaticSingleFidlMethodAnnotationProviderTest = UnitTestFixture;

class StaticDeviceIdProvider
    : public StaticSingleFidlMethodAnnotationProvider<fuchsia_feedback::DeviceIdProvider,
                                                      &GetDeviceId, ConvertDeviceId> {
 public:
  using StaticSingleFidlMethodAnnotationProvider::StaticSingleFidlMethodAnnotationProvider;

  std::set<std::string> GetKeys() const override { return {kDeviceIdKey}; }
};

TEST_F(StaticSingleFidlMethodAnnotationProviderTest, GetAll) {
  StaticDeviceIdProvider provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>());

  auto device_id_server = std::make_unique<stubs::DeviceIdProvider>(kDeviceIdValue);
  InjectServiceProvider(device_id_server.get());

  RunLoopUntilIdle();

  Annotations annotations;
  provider.GetOnce([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  EXPECT_THAT(annotations,
              UnorderedElementsAreArray({Pair(kDeviceIdKey, ErrorOrString(kDeviceIdValue))}));
  EXPECT_FALSE(device_id_server->IsBound());
}

class DeviceIdProviderClosesFirstConnection : public stubs::DeviceIdProviderBase {
 public:
  explicit DeviceIdProviderClosesFirstConnection(const std::string& device_id)
      : stubs::DeviceIdProviderBase(device_id) {}

  void GetId(GetIdCompleter::Sync& completer) override {
    if (first_call_) {
      first_call_ = false;
      CloseConnection(ZX_ERR_PEER_CLOSED);
      return;
    }

    stubs::DeviceIdProviderBase::GetId(completer);
  }

 private:
  bool first_call_ = true;
};

TEST_F(StaticSingleFidlMethodAnnotationProviderTest, Reconnects) {
  StaticDeviceIdProvider provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>());

  auto device_id_server = std::make_unique<DeviceIdProviderClosesFirstConnection>(kDeviceIdValue);
  InjectServiceProvider(device_id_server.get());

  RunLoopUntilIdle();
  EXPECT_FALSE(device_id_server->IsBound());

  Annotations annotations;
  provider.GetOnce([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  EXPECT_THAT(annotations, IsEmpty());

  RunLoopFor(zx::sec(1));
  RunLoopUntilIdle();

  EXPECT_THAT(annotations,
              UnorderedElementsAreArray({Pair(kDeviceIdKey, ErrorOrString(kDeviceIdValue))}));
  EXPECT_FALSE(device_id_server->IsBound());
}

TEST_F(StaticSingleFidlMethodAnnotationProviderTest, DoesNotReconnectIfNotFound) {
  StaticDeviceIdProvider provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>());

  auto device_id_server = std::make_unique<stubs::DeviceIdProviderNeverReturns>();
  InjectServiceProvider(device_id_server.get());

  RunLoopUntilIdle();

  Annotations annotations;
  provider.GetOnce([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  EXPECT_TRUE(device_id_server->IsBound());

  device_id_server->CloseConnection(ZX_ERR_NOT_FOUND);

  RunLoopFor(zx::sec(1));
  EXPECT_THAT(annotations, UnorderedElementsAreArray(
                               {Pair(kDeviceIdKey, ErrorOrString(Error::kNotAvailableInProduct))}));
  EXPECT_FALSE(device_id_server->IsBound());
}

TEST_F(StaticSingleFidlMethodAnnotationProviderTest, ServerDestructionDuringInFlightCall) {
  auto device_id_server = std::make_unique<stubs::DeviceIdProviderNeverReturns>();
  InjectServiceProvider(device_id_server.get());

  {
    StaticDeviceIdProvider provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>());
    provider.GetOnce([](const Annotations&) {});
    RunLoopUntilIdle();
    EXPECT_TRUE(device_id_server->IsBound());
  }

  device_id_server->CloseConnection(ZX_ERR_PEER_CLOSED);
  RunLoopUntilIdle();
}

TEST_F(StaticSingleFidlMethodAnnotationProviderTest, ServerDestructionDuringBackoff) {
  auto device_id_server = std::make_unique<DeviceIdProviderClosesFirstConnection>(kDeviceIdValue);
  InjectServiceProvider(device_id_server.get());

  {
    StaticDeviceIdProvider provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>());
    provider.GetOnce([](const Annotations&) {});
    RunLoopUntilIdle();
    EXPECT_FALSE(device_id_server->IsBound());
  }

  RunLoopFor(zx::sec(5));
}

using DynamicSingleFidlMethodAnnotationProviderTest = UnitTestFixture;

class DynamicDeviceIdProvider
    : public DynamicSingleFidlMethodAnnotationProvider<fuchsia_feedback::DeviceIdProvider,
                                                       &GetDeviceId, ConvertDeviceId> {
 public:
  using DynamicSingleFidlMethodAnnotationProvider::DynamicSingleFidlMethodAnnotationProvider;

  std::set<std::string> GetKeys() const override { return {kDeviceIdKey}; }
};

TEST_F(DynamicSingleFidlMethodAnnotationProviderTest, Get) {
  DynamicDeviceIdProvider provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>());
  auto device_id_server = std::make_unique<stubs::DeviceIdProvider>(kDeviceIdValue);
  InjectServiceProvider(device_id_server.get());

  RunLoopUntilIdle();

  Annotations annotations;
  provider.Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  EXPECT_THAT(annotations,
              UnorderedElementsAreArray({Pair(kDeviceIdKey, ErrorOrString(kDeviceIdValue))}));
  EXPECT_TRUE(device_id_server->IsBound());
}

TEST_F(DynamicSingleFidlMethodAnnotationProviderTest, GetReturnsError) {
  DynamicDeviceIdProvider provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>());
  auto device_id_server = std::make_unique<stubs::DeviceIdProviderReturnsError>(ZX_ERR_TIMED_OUT);
  InjectServiceProvider(device_id_server.get());

  RunLoopUntilIdle();

  Annotations annotations;
  provider.Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  EXPECT_THAT(annotations,
              UnorderedElementsAreArray({Pair(kDeviceIdKey, ErrorOrString(Error::kTimeout))}));
  EXPECT_FALSE(device_id_server->IsBound());
}

TEST_F(DynamicSingleFidlMethodAnnotationProviderTest, Reconnects) {
  DynamicDeviceIdProvider provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>());
  auto device_id_server = std::make_unique<stubs::DeviceIdProvider>(kDeviceIdValue);
  InjectServiceProvider(device_id_server.get());

  RunLoopUntilIdle();
  EXPECT_TRUE(device_id_server->IsBound());
  device_id_server->CloseConnection(ZX_ERR_PEER_CLOSED);
  EXPECT_FALSE(device_id_server->IsBound());

  Annotations annotations;
  provider.Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  EXPECT_THAT(annotations, UnorderedElementsAreArray(
                               {Pair(kDeviceIdKey, ErrorOrString(Error::kConnectionError))}));

  RunLoopFor(zx::sec(1));
  EXPECT_TRUE(device_id_server->IsBound());

  annotations.clear();
  provider.Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  EXPECT_THAT(annotations,
              UnorderedElementsAreArray({Pair(kDeviceIdKey, ErrorOrString(kDeviceIdValue))}));
}

TEST_F(DynamicSingleFidlMethodAnnotationProviderTest, DoesNotReconnectIfNotFound) {
  DynamicDeviceIdProvider provider(dispatcher(), services(), std::make_unique<MonotonicBackoff>());
  auto device_id_server = std::make_unique<stubs::DeviceIdProvider>(kDeviceIdValue);
  InjectServiceProvider(device_id_server.get());

  RunLoopUntilIdle();
  EXPECT_TRUE(device_id_server->IsBound());
  device_id_server->CloseConnection(ZX_ERR_NOT_FOUND);
  EXPECT_FALSE(device_id_server->IsBound());

  Annotations annotations;
  provider.Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  EXPECT_THAT(annotations, UnorderedElementsAreArray(
                               {Pair(kDeviceIdKey, ErrorOrString(Error::kNotAvailableInProduct))}));

  RunLoopFor(zx::sec(1));
  EXPECT_FALSE(device_id_server->IsBound());

  annotations.clear();
  provider.Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  EXPECT_THAT(annotations, UnorderedElementsAreArray(
                               {Pair(kDeviceIdKey, ErrorOrString(Error::kNotAvailableInProduct))}));
}

TEST_F(DynamicSingleFidlMethodAnnotationProviderTest, ProviderDestructionDuringInFlightCall) {
  auto device_id_server = std::make_unique<stubs::DeviceIdProviderNeverReturns>();
  InjectServiceProvider(device_id_server.get());

  {
    DynamicDeviceIdProvider provider(dispatcher(), services(),
                                     std::make_unique<MonotonicBackoff>());
    provider.Get([](const Annotations&) {});
    RunLoopUntilIdle();
    EXPECT_TRUE(device_id_server->IsBound());
  }

  device_id_server->CloseConnection(ZX_ERR_PEER_CLOSED);
  RunLoopUntilIdle();
}

TEST_F(DynamicSingleFidlMethodAnnotationProviderTest, ProviderDestructionDuringBackoff) {
  auto device_id_server = std::make_unique<stubs::DeviceIdProvider>(kDeviceIdValue);
  InjectServiceProvider(device_id_server.get());

  {
    DynamicDeviceIdProvider provider(dispatcher(), services(),
                                     std::make_unique<MonotonicBackoff>());
    RunLoopUntilIdle();
    EXPECT_TRUE(device_id_server->IsBound());

    device_id_server->CloseConnection(ZX_ERR_PEER_CLOSED);
    RunLoopUntilIdle();
    EXPECT_FALSE(device_id_server->IsBound());
  }

  RunLoopFor(zx::sec(5));
}

}  // namespace

}  // namespace forensics::feedback
