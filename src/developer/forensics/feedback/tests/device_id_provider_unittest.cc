// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/device_id_provider.h"

#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/testing/backoff.h"
#include "src/developer/forensics/testing/stubs/device_id_provider.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace forensics::feedback {
namespace {

using ::testing::IsEmpty;
using ::testing::Pair;
using ::testing::UnorderedElementsAreArray;

constexpr char kDefaultDeviceId[] = "00000000-0000-4000-a000-000000000001";
constexpr char kOtherDeviceId[] = "00000000-0000-4000-a000-000000000002";
constexpr char kInvalidDeviceId[] = "INVALID";

class RemoteDeviceIdProviderTest : public UnitTestFixture {
 protected:
  void SetUpDeviceIdProviderServer(
      std::unique_ptr<stubs::DeviceIdProviderBase> device_id_provider_server) {
    device_id_provider_server_ = std::move(device_id_provider_server);
    InjectServiceProvider(device_id_provider_server_.get());
  }

  std::unique_ptr<stubs::DeviceIdProviderBase> device_id_provider_server_;
};

TEST_F(RemoteDeviceIdProviderTest, GetKeys) {
  RemoteDeviceIdProvider device_id_provider(dispatcher(), services(), nullptr);
  EXPECT_THAT(device_id_provider.GetKeys(), UnorderedElementsAreArray({
                                                kDeviceFeedbackIdKey,
                                            }));
}

TEST_F(RemoteDeviceIdProviderTest, DeviceIdToAnnotations) {
  DeviceIdToAnnotations convert;

  EXPECT_THAT(convert(""), UnorderedElementsAreArray({
                               Pair(kDeviceFeedbackIdKey, ErrorOrString("")),
                           }));
  EXPECT_THAT(convert("id"), UnorderedElementsAreArray({
                                 Pair(kDeviceFeedbackIdKey, ErrorOrString("id")),
                             }));
}

TEST_F(RemoteDeviceIdProviderTest, Get) {
  SetUpDeviceIdProviderServer(std::make_unique<stubs::DeviceIdProvider>(kDefaultDeviceId));
  RemoteDeviceIdProvider device_id_provider(dispatcher(), services(),
                                            std::make_unique<MonotonicBackoff>());

  Annotations annotations;
  device_id_provider.GetOnUpdate(
      [&annotations](Annotations result) { annotations = std::move(result); });

  // |annotations| should be empty because the call hasn't completed.
  EXPECT_THAT(annotations, IsEmpty());

  RunLoopUntilIdle();
  EXPECT_THAT(annotations, UnorderedElementsAreArray({
                               Pair(kDeviceFeedbackIdKey, ErrorOrString(kDefaultDeviceId)),
                           }));

  device_id_provider_server_->SetDeviceId(kOtherDeviceId);

  // |annotations| should contain the old value because the change hasn't propagated yet.
  EXPECT_THAT(annotations, UnorderedElementsAreArray({
                               Pair(kDeviceFeedbackIdKey, ErrorOrString(kDefaultDeviceId)),
                           }));

  RunLoopUntilIdle();
  EXPECT_THAT(annotations, UnorderedElementsAreArray({
                               Pair(kDeviceFeedbackIdKey, ErrorOrString(kOtherDeviceId)),
                           }));

  device_id_provider_server_->CloseConnection(ZX_ERR_PEER_CLOSED);

  // |annotations| should still contain the last value because disconnection doesn't clear the
  // cache.
  EXPECT_THAT(annotations, UnorderedElementsAreArray({
                               Pair(kDeviceFeedbackIdKey, ErrorOrString(kOtherDeviceId)),
                           }));
}

TEST_F(RemoteDeviceIdProviderTest, Reconnects) {
  SetUpDeviceIdProviderServer(std::make_unique<stubs::DeviceIdProviderNeverReturns>());
  RemoteDeviceIdProvider device_id_provider(dispatcher(), services(),
                                            std::make_unique<MonotonicBackoff>());

  RunLoopUntilIdle();
  ASSERT_TRUE(device_id_provider_server_->IsBound());

  Annotations annotations;
  device_id_provider.GetOnUpdate(
      [&annotations](Annotations result) { annotations = std::move(result); });

  device_id_provider_server_->CloseConnection(ZX_ERR_PEER_CLOSED);
  ASSERT_FALSE(device_id_provider_server_->IsBound());

  RunLoopUntilIdle();

  // The outstanding request should complete with a connection error and not update annotations.
  EXPECT_THAT(annotations, IsEmpty());
  RunLoopFor(zx::sec(1));
  ASSERT_TRUE(device_id_provider_server_->IsBound());
}

TEST_F(RemoteDeviceIdProviderTest, DoesNotReconnectIfNotFound) {
  SetUpDeviceIdProviderServer(std::make_unique<stubs::DeviceIdProviderNeverReturns>());
  RemoteDeviceIdProvider device_id_provider(dispatcher(), services(),
                                            std::make_unique<MonotonicBackoff>());

  RunLoopUntilIdle();
  ASSERT_TRUE(device_id_provider_server_->IsBound());

  Annotations annotations;
  device_id_provider.GetOnUpdate(
      [&annotations](Annotations result) { annotations = std::move(result); });

  device_id_provider_server_->CloseConnection(ZX_ERR_NOT_FOUND);
  ASSERT_FALSE(device_id_provider_server_->IsBound());

  RunLoopFor(zx::sec(1));
  EXPECT_FALSE(device_id_provider_server_->IsBound());
}

class LocalDeviceIdProviderTest : public UnitTestFixture {
 protected:
  static std::string ReadFile(const std::string& path) {
    std::string file_contents;
    FX_CHECK(files::ReadFileToString(path, &file_contents));
    return file_contents;
  }

  files::ScopedTempDir tmp_dir_;
};

TEST_F(LocalDeviceIdProviderTest, PreservesValidId) {
  std::string device_id_path;
  ASSERT_TRUE(tmp_dir_.NewTempFileWithData(kDefaultDeviceId, &device_id_path));

  LocalDeviceIdProvider device_id_provider(device_id_path);
  Annotations annotations;
  device_id_provider.GetOnUpdate(
      [&annotations](Annotations result) { annotations = std::move(result); });

  EXPECT_THAT(annotations, UnorderedElementsAreArray({
                               Pair(kDeviceFeedbackIdKey, ErrorOrString(kDefaultDeviceId)),
                           }));
  EXPECT_EQ(ReadFile(device_id_path), kDefaultDeviceId);
}

TEST_F(LocalDeviceIdProviderTest, ReplacesInvalidId) {
  std::string device_id_path;
  ASSERT_TRUE(tmp_dir_.NewTempFileWithData(kInvalidDeviceId, &device_id_path));

  LocalDeviceIdProvider device_id_provider(device_id_path);
  Annotations annotations;
  device_id_provider.GetOnUpdate(
      [&annotations](Annotations result) { annotations = std::move(result); });

  ASSERT_TRUE(annotations.at(kDeviceFeedbackIdKey).HasValue());
  EXPECT_NE(annotations.at(kDeviceFeedbackIdKey).Value(), kInvalidDeviceId);
  EXPECT_EQ(ReadFile(device_id_path), annotations.at(kDeviceFeedbackIdKey).Value());
}

TEST_F(LocalDeviceIdProviderTest, GeneratesNewIdWhenFileMissing) {
  const std::string device_id_path = files::JoinPath(tmp_dir_.path(), "device_id_file.txt");

  LocalDeviceIdProvider device_id_provider(device_id_path);
  Annotations annotations;
  device_id_provider.GetOnUpdate(
      [&annotations](Annotations result) { annotations = std::move(result); });

  EXPECT_TRUE(annotations.contains(kDeviceFeedbackIdKey));
  EXPECT_EQ(ReadFile(device_id_path), annotations.at(kDeviceFeedbackIdKey).Value());
}

}  // namespace
}  // namespace forensics::feedback
