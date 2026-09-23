// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/encode.h"

#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "fuchsia/feedback/cpp/fidl.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/utils/errors.h"

namespace fuchsia::feedback {

bool operator==(const Annotation& lhs, const Annotation& rhs) {
  return lhs.key == rhs.key && lhs.value == rhs.value;
}

}  // namespace fuchsia::feedback

namespace forensics::feedback {
namespace {

using ::testing::UnorderedElementsAreArray;

TEST(EncodeTest, AnnotationsAsFidl) {
  const auto annotations = Encode<fuchsia::feedback::Annotations>(Annotations({
      {"key1", ErrorOrString("value1")},
      {"key2", ErrorOrString("value2")},
      {"key3", ErrorOrString(Error::kTimeout)},
  }));

  const std::vector<fuchsia::feedback::Annotation> expected({
      fuchsia::feedback::Annotation{.key = "key1", .value = "value1"},
      fuchsia::feedback::Annotation{.key = "key2", .value = "value2"},
  });

  ASSERT_TRUE(annotations.has_annotations2());
  EXPECT_EQ(annotations.annotations2(), expected);
}

TEST(EncodeTest, EmptyAnnotationsAsFidl) {
  const auto annotations = Encode<fuchsia::feedback::Annotations>(Annotations({}));

  EXPECT_FALSE(annotations.has_annotations2());
}

TEST(EncodeTest, AnnotationsAsFidlLargeSize) {
  Annotations annotations;
  std::vector<fuchsia::feedback::Annotation> expected;

  for (uint32_t i = 0; i < fuchsia::feedback::MAX_NUM_ANNOTATIONS2_PROVIDED; ++i) {
    const std::string key = "key" + std::to_string(i);
    annotations.insert({key, ErrorOrString("fake_value")});
    expected.push_back({key, "fake_value"});
  }

  const auto fidl_annotations = Encode<fuchsia::feedback::Annotations>(annotations);

  ASSERT_TRUE(fidl_annotations.has_annotations2());
  EXPECT_THAT(fidl_annotations.annotations2(), UnorderedElementsAreArray(expected));
}

TEST(EncodeTest, AnnotationsAsString) {
  EXPECT_EQ(Encode<std::string>(Annotations({
                {"key1", ErrorOrString("value1")},
                {"key2", ErrorOrString("value2")},
                {"key3", ErrorOrString(Error::kTimeout)},
            })),
            R"({
    "key1": "value1",
    "key2": "value2"
})");
}

TEST(EncodeTest, EmptyAnnotationsAsString) {
  EXPECT_EQ(Encode<std::string>(Annotations({})), "{}");
}

}  // namespace
}  // namespace forensics::feedback
