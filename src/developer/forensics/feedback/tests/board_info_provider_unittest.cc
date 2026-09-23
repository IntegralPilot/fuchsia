// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/board_info_provider.h"

#include <fidl/fuchsia.hwinfo/cpp/natural_types.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/annotations/constants.h"

namespace forensics::feedback {
namespace {

using ::testing::Pair;
using ::testing::UnorderedElementsAreArray;

TEST(BoardInfoToAnnotationsTest, ConvertSuccess) {
  BoardInfoToAnnotations convert;

  fuchsia_hwinfo::BoardInfo info;
  fuchsia_hwinfo::BoardGetInfoResponse response{{.info = info}};
  EXPECT_THAT(convert(response), UnorderedElementsAreArray({
                                     Pair(kHardwareBoardNameKey, Error::kMissingValue),
                                     Pair(kHardwareBoardRevisionKey, Error::kMissingValue),
                                 }));

  info.name("board_name");
  response = fuchsia_hwinfo::BoardGetInfoResponse{{.info = info}};
  EXPECT_THAT(convert(response),
              UnorderedElementsAreArray({
                  Pair(kHardwareBoardNameKey, ErrorOrString("board_name")),
                  Pair(kHardwareBoardRevisionKey, ErrorOrString(Error::kMissingValue)),
              }));

  info.revision("revision");
  response = fuchsia_hwinfo::BoardGetInfoResponse{{.info = info}};
  EXPECT_THAT(convert(response), UnorderedElementsAreArray({
                                     Pair(kHardwareBoardNameKey, ErrorOrString("board_name")),
                                     Pair(kHardwareBoardRevisionKey, ErrorOrString("revision")),
                                 }));
}

TEST(BoardInfoToAnnotationsTest, ConvertError) {
  BoardInfoToAnnotations convert;
  EXPECT_THAT(convert(Error::kConnectionError),
              UnorderedElementsAreArray({
                  Pair(kHardwareBoardNameKey, Error::kConnectionError),
                  Pair(kHardwareBoardRevisionKey, Error::kConnectionError),
              }));
}

TEST(BoardInfoProvider, Keys) {
  // Safe to pass nullptrs b/c objects are never used.
  BoardInfoProvider provider(nullptr, nullptr, nullptr);

  EXPECT_THAT(provider.GetKeys(), UnorderedElementsAreArray({
                                      kHardwareBoardNameKey,
                                      kHardwareBoardRevisionKey,
                                  }));
}

}  // namespace
}  // namespace forensics::feedback
