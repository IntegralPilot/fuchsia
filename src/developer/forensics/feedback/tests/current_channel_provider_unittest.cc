// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/current_channel_provider.h"

#include <fidl/fuchsia.update.channel/cpp/fidl.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/annotations/constants.h"

namespace forensics::feedback {
namespace {

using ::testing::Pair;
using ::testing::UnorderedElementsAreArray;

TEST(CurrentChannelToAnnotationsTest, ConvertSuccess) {
  CurrentChannelToAnnotations convert;

  fuchsia_update_channel::ProviderGetCurrentResponse response{{.channel = ""}};
  EXPECT_THAT(convert(response), UnorderedElementsAreArray({
                                     Pair(kSystemUpdateChannelCurrentKey, ErrorOrString("")),
                                 }));

  response.channel("channel");
  EXPECT_THAT(convert(response), UnorderedElementsAreArray({
                                     Pair(kSystemUpdateChannelCurrentKey, ErrorOrString("channel")),
                                 }));
}

TEST(CurrentChannelToAnnotationsTest, ConvertError) {
  CurrentChannelToAnnotations convert;
  EXPECT_THAT(convert(Error::kConnectionError),
              UnorderedElementsAreArray({
                  Pair(kSystemUpdateChannelCurrentKey, Error::kConnectionError),
              }));
}

TEST(CurrentChannelrProvider, Keys) {
  // Safe to pass nullptrs b/c objects are never used.
  CurrentChannelProvider provider(nullptr, nullptr, nullptr);

  EXPECT_THAT(provider.GetKeys(), UnorderedElementsAreArray({
                                      kSystemUpdateChannelCurrentKey,
                                  }));
}

}  // namespace
}  // namespace forensics::feedback
