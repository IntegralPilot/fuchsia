// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/target_channel_provider.h"

#include <fidl/fuchsia.update.channelcontrol/cpp/fidl.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"

namespace forensics::feedback {
namespace {

using ::testing::Pair;
using ::testing::UnorderedElementsAreArray;

TEST(TargetChannelToAnnotationsTest, Convert) {
  TargetChannelToAnnotations convert;

  fuchsia_update_channelcontrol::ChannelControlGetTargetResponse response;
  response.channel("");
  EXPECT_THAT(convert(response), UnorderedElementsAreArray({
                                     Pair(kSystemUpdateChannelTargetKey, ErrorOrString("")),
                                 }));

  response.channel("channel");
  EXPECT_THAT(convert(response), UnorderedElementsAreArray({
                                     Pair(kSystemUpdateChannelTargetKey, ErrorOrString("channel")),
                                 }));

  EXPECT_THAT(convert(Error::kConnectionError),
              UnorderedElementsAreArray({
                  Pair(kSystemUpdateChannelTargetKey, ErrorOrString(Error::kConnectionError)),
              }));
}

using TargetChannelProviderTest = UnitTestFixture;

TEST_F(TargetChannelProviderTest, Keys) {
  // Safe to pass nullptrs b/c objects are never used.
  TargetChannelProvider provider(dispatcher(), services(), nullptr);

  EXPECT_THAT(provider.GetKeys(), UnorderedElementsAreArray({
                                      kSystemUpdateChannelTargetKey,
                                  }));
}

}  // namespace
}  // namespace forensics::feedback
