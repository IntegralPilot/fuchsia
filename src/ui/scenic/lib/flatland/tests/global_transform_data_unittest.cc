// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include <memory>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/global_matrix_data.h"
#include "src/ui/scenic/lib/flatland/global_resolved_layers.h"
#include "src/ui/scenic/lib/flatland/global_topology_data.h"

#include <glm/glm.hpp>
#include <glm/gtc/constants.hpp>
#include <glm/gtc/type_ptr.hpp>
#include <glm/gtx/matrix_transform_2d.hpp>

namespace flatland {
namespace test {

namespace {

using fuchsia_ui_composition::ImageFlip;
using fuchsia_ui_composition::Orientation;

constexpr int kImageWidth = 1000;
constexpr int kImageHeight = 500;

// Helper function to generate a SrcToDest from a glm::mat3 for tests that are strictly testing the
// conversion math.
SrcToDest GetSrcToDestForMatrix(const glm::mat3& matrix, ImageFlip image_flip = ImageFlip::kNone) {
  const types::RectangleF unclipped_src({0.f, 0.f, kImageWidth, kImageHeight});
  return CreateSrcToDest(matrix, kUnclippedRegion, unclipped_src, image_flip);
}

// Helper function to generate a SrcToDest from a glm::mat3 for tests that are strictly testing the
// conversion math.
SrcToDest GetSrcToDestForMatrixAndClip(const glm::mat3& matrix, const TransformClipRegion& clip,
                                       ImageFlip image_flip = ImageFlip::kNone) {
  const types::RectangleF unclipped_src({0.f, 0.f, kImageWidth, kImageHeight});
  return CreateSrcToDest(matrix, clip, unclipped_src, image_flip);
}

// Helper function for getting the correct rotation angle. Matrices are specified in view-space
// coordinates, in which the +y axis points downwards (not upwards). Rotations which are specified
// as counter-clockwise must actually occur in a clockwise fashion in this coordinate space (a
// vector on the +x axis rotates towards -y axis to give the appearance of a counter-clockwise
// rotation).
float GetOrientationAngleInViewSpaceCoordinates(Orientation angle) {
  switch (angle) {
    case Orientation::kCcw90Degrees:
      return -glm::half_pi<float>();
    case Orientation::kCcw180Degrees:
      return -glm::pi<float>();
    case Orientation::kCcw270Degrees:
      return -glm::three_over_two_pi<float>();
    case Orientation::kCcw0Degrees:
      return 0.f;
  }
}

}  // namespace

// The following tests ensure the transform hierarchy is properly reflected in the list of global
// rectangles.

TEST(GlobalMatrixDataTest, EmptyTopologyReturnsEmptyMatrices) {
  UberStruct::InstanceMap uber_structs;
  GlobalTopologyData::TopologyVector topology_vector;
  GlobalTopologyData::ParentIndexVector parent_indices;

  auto global_matrices = ComputeGlobalMatrices(topology_vector, parent_indices, uber_structs);
  EXPECT_TRUE(global_matrices.empty());
}

TEST(GlobalMatrixDataTest, EmptyLocalMatricesAreIdentity) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing the following graph:
  //
  // 1:0 - 1:1
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {1, 1}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0};

  // The UberStruct for instance ID 1 must exist, but it contains no local matrices.
  auto uber_struct = std::make_unique<UberStruct>();
  uber_structs[1] = std::move(uber_struct);

  // The root matrix is set to the identity matrix, and the second inherits that.
  std::vector<glm::mat3> expected_matrices = {
      glm::mat3(),
      glm::mat3(),
  };

  auto global_matrices = ComputeGlobalMatrices(topology_vector, parent_indices, uber_structs);
  EXPECT_THAT(global_matrices, ::testing::ElementsAreArray(expected_matrices));
}

TEST(GlobalMatrixDataTest, GlobalMatricesIncludeParentMatrix) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing the following graph:
  //
  //    1:0 - 1:1 - 1:2
  //       \
  //       1:3 - 1:4
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {1, 1}, {1, 2}, {1, 3}, {1, 4}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0, 1, 0, 3};

  auto uber_struct = std::make_unique<UberStruct>();

  static const glm::vec2 kTranslation = {1.f, 2.f};
  static const float kRotation = glm::half_pi<float>();
  static const glm::vec2 kScale = {3.f, 5.f};

  // All transforms will get the translation from 1:0
  uber_struct->local_matrices[{1, 0}] = glm::translate(glm::mat3(), kTranslation);

  // The 1:1 - 1:2 branch rotates, then scales.
  uber_struct->local_matrices[{1, 1}] = glm::rotate(glm::mat3(), kRotation);
  uber_struct->local_matrices[{1, 2}] = glm::scale(glm::mat3(), kScale);

  // The 1:3 - 1:4 branch scales, then rotates.
  uber_struct->local_matrices[{1, 3}] = glm::scale(glm::mat3(), kScale);
  uber_struct->local_matrices[{1, 4}] = glm::rotate(glm::mat3(), kRotation);

  uber_structs[1] = std::move(uber_struct);

  // The expected matrices apply the operations in the correct order. The translation always comes
  // first, followed by the operations of the children.
  std::vector<glm::mat3> expected_matrices = {
      glm::translate(glm::mat3(), kTranslation),
      glm::rotate(glm::translate(glm::mat3(), kTranslation), kRotation),
      glm::scale(glm::rotate(glm::translate(glm::mat3(), kTranslation), kRotation), kScale),
      glm::scale(glm::translate(glm::mat3(), kTranslation), kScale),
      glm::rotate(glm::scale(glm::translate(glm::mat3(), kTranslation), kScale), kRotation),
  };

  auto global_matrices = ComputeGlobalMatrices(topology_vector, parent_indices, uber_structs);
  EXPECT_THAT(global_matrices, ::testing::ElementsAreArray(expected_matrices));
}

TEST(GlobalMatrixDataTest, GlobalMatricesMultipleUberStructs) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing the following graph:
  //
  // 1:0 - 2:0
  //     \
  //       1:1
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {2, 0}, {1, 1}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0, 0};

  auto uber_struct1 = std::make_unique<UberStruct>();
  auto uber_struct2 = std::make_unique<UberStruct>();

  // Each matrix scales by a different prime number to distinguish the branches.
  uber_struct1->local_matrices[{1, 0}] = glm::scale(glm::mat3(), {2.f, 2.f});
  uber_struct1->local_matrices[{1, 1}] = glm::scale(glm::mat3(), {3.f, 3.f});

  uber_struct2->local_matrices[{2, 0}] = glm::scale(glm::mat3(), {5.f, 5.f});

  uber_structs[1] = std::move(uber_struct1);
  uber_structs[2] = std::move(uber_struct2);

  std::vector<glm::mat3> expected_matrices = {
      glm::scale(glm::mat3(), glm::vec2(2.f)),   // 1:0 = 2
      glm::scale(glm::mat3(), glm::vec2(10.f)),  // 1:0 * 2:0 = 2 * 5 = 10
      glm::scale(glm::mat3(), glm::vec2(6.f)),   // 1:0 * 1:1 = 2 * 3 = 6
  };

  auto global_matrices = ComputeGlobalMatrices(topology_vector, parent_indices, uber_structs);
  EXPECT_THAT(global_matrices, ::testing::ElementsAreArray(expected_matrices));
}

// The following tests ensure that different clip boundaries affect rectangles in the proper manner.

// TODO(https://fxbug.dev/523371761): When revisiting these tests, also rename
// locals like expected_rectangle/rectangle to match the SrcToDest type (e.g.
// expected/actual).
//
// Test that if a clip region is completely larger than the rectangle, it has no effect on the
// rectangle.
TEST(SrcToDestTest, ParentCompletelyBiggerThanChildClipTest) {
  const glm::vec2 extent(100.f, 50.f);
  auto matrix = glm::scale(glm::mat3(), extent);

  TransformClipRegion clip({.x = 0, .y = 0, .width = 120, .height = 60});

  const SrcToDest expected_rectangle(types::RectangleF({0, 0, kImageWidth, kImageHeight}),
                                     types::RectangleF({0, 0, 100, 50}),
                                     types::RotateFlip::kIdentity());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// Test that if the child is completely bigger on all sides than the clip, that it gets clamped
// exactly to the clip region.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipTest) {
  const glm::vec2 extent(100.f, 90.f);
  auto matrix = glm::scale(glm::mat3(), extent);

  TransformClipRegion clip({20, 30, 35, 40});

  const SrcToDest expected_rectangle(types::RectangleF({200.f, 500.f / 3.f, 350.f, 2000.f / 9.f}),
                                     types::RectangleF::From(clip), types::RotateFlip::kIdentity());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// Test that if the child is completely bigger on all sides than the clip and is rotated by 90
// degrees, that it gets clamped exactly to the clip region.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipRotatedBy90Test) {
  const glm::vec2 extent(100.f, 90.f);
  // Since rotation occurs around the top-left corner, translate the rectangle so that it has the
  // same origin after rotation.
  glm::mat3 matrix = glm::translate(glm::mat3(), {0, extent.x});
  matrix =
      glm::rotate(matrix, GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees));
  matrix = glm::scale(matrix, extent);

  // Note that this clip region is specified in global space and will not be modified by the matrix.
  // The clip's y-interval is deliberately off-center in the rotated height so
  // that mirrored or reversed u-intervals differ from unflipped ones.
  TransformClipRegion clip({20, 35, 35, 40});

  // The rectangle is rotated by 90 such that, prior to clipping, it has a new extent of (90, 100).
  // The texel u-coordinate is now linearly interpolated vertically and the v-coordinate is now
  // linearly interpolated horizontally.
  const SrcToDest expected_rectangle(types::RectangleF({250.f, 1000.f / 9.f, 400.f, 3500.f / 18.f}),
                                     types::RectangleF::From(clip),
                                     types::RotateFlip::kRotateCcw90());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// Test that if the child is completely bigger on all sides than the clip and is rotated by 180
// degrees, that it gets clamped exactly to the clip region.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipRotatedBy180Test) {
  const glm::vec2 extent(100.f, 90.f);
  // Since rotation occurs around the top-left corner, translate the rectangle so that it has the
  // same origin after rotation.
  glm::mat3 matrix = glm::translate(glm::mat3(), {extent.x, extent.y});
  matrix =
      glm::rotate(matrix, GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw180Degrees));
  matrix = glm::scale(matrix, extent);

  // Note that this clip region is specified in global space and will not be modified by the matrix.
  TransformClipRegion clip({20, 30, 35, 40});

  // After clipping, the UV coordinates are reversed. I.e. if the coordinate was initially 0.2, then
  // it would instead be equal to 0.8.
  const SrcToDest expected_rectangle(types::RectangleF({450.f, 1000.f / 9.f, 350.f, 2000.f / 9.f}),
                                     types::RectangleF::From(clip),
                                     types::RotateFlip::kRotateCcw180());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// Test that if the child is completely bigger on all sides than the clip and is rotated by 270
// degrees, that it gets clamped exactly to the clip region.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipRotatedBy270Test) {
  const glm::vec2 extent(100.f, 90.f);
  // Since rotation occurs around the top-left corner, translate the rectangle so that it has the
  // same origin after rotation.
  glm::mat3 matrix = glm::translate(glm::mat3(), {extent.y, 0});
  matrix =
      glm::rotate(matrix, GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw270Degrees));
  matrix = glm::scale(matrix, extent);

  // Note that this clip region is specified in global space and will not be modified by the matrix.
  // The clip's y-interval is deliberately off-center in the rotated height so
  // that mirrored or reversed u-intervals differ from unflipped ones.
  TransformClipRegion clip({20, 35, 35, 40});

  // The rectangle was rotated by 90, such that, prior to clipping, it has a new_extent of (90, 100)
  // and reordered_uvs of [(0, 1), (0, 0), (1, 0), (1, 1)]. The u-coordinate is now linearly
  // interpolated vertically and the v coordinate is now linearly interpolated horizontally.
  const SrcToDest expected_rectangle(
      types::RectangleF({350.f, 3500.f / 18.f, 400.f, 3500.f / 18.f}),
      types::RectangleF::From(clip), types::RotateFlip::kRotateCcw270());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// Test that if the child doesn't overlap the clip region at all, that the
// rectangle has zero size.
TEST(SrcToDestTest, RectangleAndClipNoOverlap) {
  const glm::vec2 offset(5, 10);
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::translate(glm::mat3(), offset);
  matrix = glm::scale(matrix, extent);

  TransformClipRegion clip({0, 0, 2, 2});

  const SrcToDest expected_rectangle(types::RectangleF({0, 0, 0, 0}),
                                     types::RectangleF({0, 0, 0, 0}),
                                     types::RotateFlip::kIdentity());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// Test that clipping works in the case of partial overlap.
TEST(SrcToDestTest, RectangleAndClipPartialOverlap) {
  const glm::vec2 offset(20, 30);
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::translate(glm::mat3(), offset);
  matrix = glm::scale(matrix, extent);

  TransformClipRegion clip({10, 30, 80, 40});

  const SrcToDest expected_rectangle(types::RectangleF({0, 0, 700, 400}),
                                     types::RectangleF({20, 30, 70, 40}),
                                     types::RotateFlip::kIdentity());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// The following tests ensure that different geometric attributes (translation, rotation, scale)
// modify the final rectangle as expected.

TEST(SrcToDestTest, ScaleAndRotate90DegreesTest) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees));
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({0.f, -100.f, 50.f, 100.f}),
                                     types::RotateFlip::kRotateCcw90());

  const auto rectangle = GetSrcToDestForMatrix(matrix);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleAndRotate180DegreesTest) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw180Degrees));
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({-100.f, -50.f, 100.f, 50.f}),
                                     types::RotateFlip::kRotateCcw180());

  const auto rectangle = GetSrcToDestForMatrix(matrix);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleAndRotate270DegreesTest) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw270Degrees));
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({-50.f, 0.f, 50.f, 100.f}),
                                     types::RotateFlip::kRotateCcw270());

  const auto rectangle = GetSrcToDestForMatrix(matrix);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleAndFlipHorizontal) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::scale(glm::mat3(), extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({0.f, 0.f, 100.f, 50.f}),
                                     types::RotateFlip::kReflectY());

  const auto rectangle = GetSrcToDestForMatrix(matrix, ImageFlip::kLeftRight);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleRotate90DegreesAndFlipHorizontal) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees));
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({0.f, -100.f, 50.f, 100.f}),
                                     types::RotateFlip::kRotateCcw90ReflectX());

  const auto rectangle = GetSrcToDestForMatrix(matrix, ImageFlip::kLeftRight);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleRotate180DegreesAndFlipHorizontal) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw180Degrees));
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({-100.f, -50.f, 100.f, 50.f}),
                                     types::RotateFlip::kReflectX());

  const auto rectangle = GetSrcToDestForMatrix(matrix, ImageFlip::kLeftRight);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleRotate270DegreesAndFlipHorizontal) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw270Degrees));
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({-50.f, 0.f, 50.f, 100.f}),
                                     types::RotateFlip::kRotateCcw90ReflectY());

  const auto rectangle = GetSrcToDestForMatrix(matrix, ImageFlip::kLeftRight);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleAndFlipVertical) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::scale(glm::mat3(), extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({0.f, 0.f, 100.f, 50.f}),
                                     types::RotateFlip::kReflectX());

  const auto rectangle = GetSrcToDestForMatrix(matrix, ImageFlip::kUpDown);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleRotate90DegreesAndFlipVertical) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees));
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({0.f, -100.f, 50.f, 100.f}),
                                     types::RotateFlip::kRotateCcw90ReflectY());

  const auto rectangle = GetSrcToDestForMatrix(matrix, ImageFlip::kUpDown);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleRotate180DegreesAndFlipVertical) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw180Degrees));
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({-100.f, -50.f, 100.f, 50.f}),
                                     types::RotateFlip::kReflectY());

  const auto rectangle = GetSrcToDestForMatrix(matrix, ImageFlip::kUpDown);
  EXPECT_EQ(rectangle, expected_rectangle);
}

TEST(SrcToDestTest, ScaleRotate270DegreesAndFlipVertical) {
  const glm::vec2 extent(100.f, 50.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw270Degrees));
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({-50.f, 0.f, 50.f, 100.f}),
                                     types::RotateFlip::kRotateCcw90ReflectX());

  const auto rectangle = GetSrcToDestForMatrix(matrix, ImageFlip::kUpDown);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// NOTE: This test is the same as |ChildCompletelyBiggerThanParentClipTest|, except that the image
// is flipped before clipping - this is reflected in the x-coordinate UVs
// i.e. 200 --> kImageWidth - 200 = 800.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipFlipLeftRightTest) {
  const glm::vec2 extent(100.f, 90.f);
  auto matrix = glm::scale(glm::mat3(), extent);

  TransformClipRegion clip({20, 30, 35, 40});

  const SrcToDest expected_rectangle(types::RectangleF({450.f, 500.f / 3.f, 350.f, 2000.f / 9.f}),
                                     types::RectangleF::From(clip), types::RotateFlip::kReflectY());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip, ImageFlip::kLeftRight);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// NOTE: This test is the same as |ChildCompletelyBiggerThanParentClipRotatedBy90Test|, except that
// the image is flipped before rotating/clipping - this is reflected in the x-coordinate UVs:
// the unflipped source interval [250, 650] mirrors to [350, 750].
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipFlipLeftRightRotatedBy90Test) {
  const glm::vec2 extent(100.f, 90.f);
  // Since rotation occurs around the top-left corner, translate the rectangle so that it has the
  // same origin after rotation.
  glm::mat3 matrix = glm::translate(glm::mat3(), {0, extent.x});
  matrix =
      glm::rotate(matrix, GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees));
  matrix = glm::scale(matrix, extent);

  // Note that this clip region is specified in global space and will not be modified by the matrix.
  // The clip's y-interval is deliberately off-center in the rotated height so
  // that mirrored or reversed u-intervals differ from unflipped ones.
  TransformClipRegion clip({20, 35, 35, 40});

  // The rectangle is rotated by 90 such that, prior to clipping, it has a new extent of (90, 100).
  // The texel u-coordinate is now linearly interpolated vertically and the v-coordinate is now
  // linearly interpolated horizontally.
  const SrcToDest expected_rectangle(types::RectangleF({350.f, 1000.f / 9.f, 400.f, 3500.f / 18.f}),
                                     types::RectangleF::From(clip),
                                     types::RotateFlip::kRotateCcw90ReflectX());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip, ImageFlip::kLeftRight);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// NOTE: This test is the same as |ChildCompletelyBiggerThanParentClipRotatedBy180Test|, except that
// the image is flipped before rotating/clipping - this is reflected in the x-coordinate UVs
// i.e. 450 --> kImageWidth - 450 = 550.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipFlipLeftRightRotatedBy180Test) {
  const glm::vec2 extent(100.f, 90.f);
  // Since rotation occurs around the top-left corner, translate the rectangle so that it has the
  // same origin after rotation.
  glm::mat3 matrix = glm::translate(glm::mat3(), {extent.x, extent.y});
  matrix =
      glm::rotate(matrix, GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw180Degrees));
  matrix = glm::scale(matrix, extent);

  // Note that this clip region is specified in global space and will not be modified by the matrix.
  TransformClipRegion clip({20, 30, 35, 40});

  // After clipping, the UV coordinates are reversed. I.e. if the coordinate was initially 0.2, then
  // it would instead be equal to 0.8.
  const SrcToDest expected_rectangle(types::RectangleF({200.f, 1000.f / 9.f, 350.f, 2000.f / 9.f}),
                                     types::RectangleF::From(clip), types::RotateFlip::kReflectX());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip, ImageFlip::kLeftRight);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// NOTE: This test is the same as |ChildCompletelyBiggerThanParentClipRotatedBy270Test|, except that
// the image is flipped before rotating/clipping - this is reflected in the x-coordinate UVs:
// the unflipped source interval [350, 750] mirrors to [250, 650].
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipFlipLeftRightRotatedBy270Test) {
  const glm::vec2 extent(100.f, 90.f);
  // Since rotation occurs around the top-left corner, translate the rectangle so that it has the
  // same origin after rotation.
  glm::mat3 matrix = glm::translate(glm::mat3(), {extent.y, 0});
  matrix =
      glm::rotate(matrix, GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw270Degrees));
  matrix = glm::scale(matrix, extent);

  // Note that this clip region is specified in global space and will not be modified by the matrix.
  // The clip's y-interval is deliberately off-center in the rotated height so
  // that mirrored or reversed u-intervals differ from unflipped ones.
  TransformClipRegion clip({20, 35, 35, 40});

  // The rectangle was rotated by 90, such that, prior to clipping, it has a new_extent of (90, 100)
  // and reordered_uvs of [(0, 1), (0, 0), (1, 0), (1, 1)]. The u-coordinate is now linearly
  // interpolated vertically and the v coordinate is now linearly interpolated horizontally.
  const SrcToDest expected_rectangle(
      types::RectangleF({250.f, 3500.f / 18.f, 400.f, 3500.f / 18.f}),
      types::RectangleF::From(clip), types::RotateFlip::kRotateCcw90ReflectY());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip, ImageFlip::kLeftRight);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// NOTE: This test is the same as |ChildCompletelyBiggerThanParentClipTest|, except that the image
// is flipped before clipping - this is reflected in the y-coordinate UVs:
// i.e. 167 --> kImageHeight - 167 = 333.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipFlipUpDownTest) {
  const glm::vec2 extent(100.f, 90.f);
  auto matrix = glm::scale(glm::mat3(), extent);

  TransformClipRegion clip({20, 30, 35, 40});

  const SrcToDest expected_rectangle(types::RectangleF({200.f, 1000.f / 9.f, 350.f, 2000.f / 9.f}),
                                     types::RectangleF::From(clip), types::RotateFlip::kReflectX());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip, ImageFlip::kUpDown);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// NOTE: This test is the same as |ChildCompletelyBiggerThanParentClipRotatedBy90Test|, except that
// the image is flipped before rotating/clipping - this is reflected in the y-coordinate UVs
//  i.e. 111 --> kImageHeight - 111 = 389.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipFlipUpDownRotatedBy90Test) {
  const glm::vec2 extent(100.f, 90.f);
  // Since rotation occurs around the top-left corner, translate the rectangle so that it has the
  // same origin after rotation.
  glm::mat3 matrix = glm::translate(glm::mat3(), {0, extent.x});
  matrix =
      glm::rotate(matrix, GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees));
  matrix = glm::scale(matrix, extent);

  // Note that this clip region is specified in global space and will not be modified by the matrix.
  // The clip's y-interval is deliberately off-center in the rotated height so
  // that mirrored or reversed u-intervals differ from unflipped ones.
  TransformClipRegion clip({20, 35, 35, 40});

  // The rectangle is rotated by 90 such that, prior to clipping, it has a new extent of (90, 100).
  // The texel u-coordinate is now linearly interpolated vertically and the y-coordinate is now
  // linearly interpolated horizontally.
  const SrcToDest expected_rectangle(
      types::RectangleF({250.f, 3500.f / 18.f, 400.f, 3500.f / 18.f}),
      types::RectangleF::From(clip), types::RotateFlip::kRotateCcw90ReflectY());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip, ImageFlip::kUpDown);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// NOTE: This test is the same as |ChildCompletelyBiggerThanParentClipRotatedBy180Test|, except that
// the image is flipped before rotating/clipping - this is reflected in the y-coordinate UVs
// i.e. 111 --> kImageHeight - 111 = 389.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipFlipUpDownRotatedBy180Test) {
  const glm::vec2 extent(100.f, 90.f);
  // Since rotation occurs around the top-left corner, translate the rectangle so that it has the
  // same origin after rotation.
  glm::mat3 matrix = glm::translate(glm::mat3(), {extent.x, extent.y});
  matrix =
      glm::rotate(matrix, GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw180Degrees));
  matrix = glm::scale(matrix, extent);

  // Note that this clip region is specified in global space and will not be modified by the matrix.
  TransformClipRegion clip({20, 30, 35, 40});

  // After clipping, the UV coordinates are reversed. I.e. if the coordinate was initially 0.2, then
  // it would instead be equal to 0.8.
  const SrcToDest expected_rectangle(types::RectangleF({450.f, 500.f / 3.f, 350.f, 2000.f / 9.f}),
                                     types::RectangleF::From(clip), types::RotateFlip::kReflectY());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip, ImageFlip::kUpDown);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// NOTE: This test is the same as |ChildCompletelyBiggerThanParentClipRotatedBy270Test|, except that
// the image is flipped before rotating/clipping - this is reflected in the y-coordinate UVs
// i.e. 194 --> kImageHeight - 194 = 306.
TEST(SrcToDestTest, ChildCompletelyBiggerThanParentClipFlipUpDownRotatedBy270Test) {
  const glm::vec2 extent(100.f, 90.f);
  // Since rotation occurs around the top-left corner, translate the rectangle so that it has the
  // same origin after rotation.
  glm::mat3 matrix = glm::translate(glm::mat3(), {extent.y, 0});
  matrix =
      glm::rotate(matrix, GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw270Degrees));
  matrix = glm::scale(matrix, extent);

  // Note that this clip region is specified in global space and will not be modified by the matrix.
  // The clip's y-interval is deliberately off-center in the rotated height so
  // that mirrored or reversed u-intervals differ from unflipped ones.
  TransformClipRegion clip({20, 35, 35, 40});

  // The rectangle was rotated by 90, such that, prior to clipping, it has a new_extent of (90, 100)
  // and reordered_uvs of [(0, 1), (0, 0), (1, 0), (1, 1)]. The u-coordinate is now linearly
  // interpolated vertically and the v coordinate is now linearly interpolated horizontally.
  const SrcToDest expected_rectangle(types::RectangleF({350.f, 1000.f / 9.f, 400.f, 3500.f / 18.f}),
                                     types::RectangleF::From(clip),
                                     types::RotateFlip::kRotateCcw90ReflectX());

  const auto rectangle = GetSrcToDestForMatrixAndClip(matrix, clip, ImageFlip::kUpDown);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// Make sure that floating point transform values that aren't exactly
// integers are also respected.
TEST(SrcToDestTest, FloatingPointTranslateAndScaleTest) {
  const glm::vec2 offset(10.9f, 20.5f);
  const glm::vec2 extent(100.3f, 200.7f);
  glm::mat3 matrix = glm::translate(glm::mat3(), offset);
  matrix = glm::scale(matrix, extent);

  const SrcToDest expected_rectangle(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                     types::RectangleF({offset.x, offset.y, extent.x, extent.y}),
                                     types::RotateFlip::kIdentity());

  const auto rectangle = GetSrcToDestForMatrix(matrix);
  EXPECT_EQ(rectangle, expected_rectangle);
}

// The same operations of translate/rotate/scale on a single matrix.
TEST(SrcToDestTest, OrderOfOperationsTest) {
  // First subtest tests swapping scaling and translation.
  {
    // Here we scale and then translate. The origin should be at (10,5) and the extent should also
    // still be (2,2) since the scale is being applied on the untranslated coordinates.
    const glm::mat3 test_1 =
        glm::scale(glm::translate(glm::mat3(), glm::vec2(10.f, 5.f)), glm::vec2(2.f, 2.f));

    const SrcToDest expected_rectangle_1(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                         types::RectangleF({10.f, 5.f, 2.f, 2.f}),
                                         types::RotateFlip::kIdentity());

    const auto rectangle_1 = GetSrcToDestForMatrix(test_1);
    EXPECT_EQ(rectangle_1, expected_rectangle_1);

    // Here we translate first, and then scale the translation, resulting in the origin point
    // doubling from (10, 5) to (20, 10).
    const glm::mat3 test_2 =
        glm::translate(glm::scale(glm::mat3(), glm::vec2(2.f, 2.f)), glm::vec2(10.f, 5.f));

    const SrcToDest expected_rectangle_2(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                         types::RectangleF({20.f, 10.f, 2.f, 2.f}),
                                         types::RotateFlip::kIdentity());

    const auto rectangle_2 = GetSrcToDestForMatrix(test_2);
    EXPECT_EQ(rectangle_2, expected_rectangle_2);
  }

  // Second subtest tests swapping translation and rotation.
  {
    // Since the rotation is applied first, the origin point rotates around (0,0), placing the
    // origin at (0, -1). We then translate by (10, 5) to wind up at (10, 4).
    const glm::mat3 test_1 =
        glm::rotate(glm::translate(glm::mat3(), glm::vec2(10.f, 5.f)),
                    GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees));

    const SrcToDest expected_rectangle_1(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                         types::RectangleF({10.f, 4.f, 1.f, 1.f}),
                                         types::RotateFlip::kRotateCcw90());

    const auto rectangle_1 = GetSrcToDestForMatrix(test_1);
    EXPECT_EQ(rectangle_1, expected_rectangle_1);

    // Since we translated first here, the point goes from (0,0) to (10,5) and then rotates
    // 90 degrees counterclockwise. This places the origin at (5, -11).
    const glm::mat3 test_2 = glm::translate(
        glm::rotate(glm::mat3(),
                    GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees)),
        glm::vec2(10.f, 5.f));

    const SrcToDest expected_rectangle_2(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                         types::RectangleF({5.f, -11.f, 1.f, 1.f}),
                                         types::RotateFlip::kRotateCcw90());

    const auto rectangle_2 = GetSrcToDestForMatrix(test_2);
    EXPECT_EQ(rectangle_2, expected_rectangle_2);
  }

  // Third subtest tests swapping non-uniform scaling and rotation.
  {
    // We rotate first and then scale, so the scaling isn't affected by the rotation.
    const glm::mat3 test_1 =
        glm::rotate(glm::scale(glm::mat3(), glm::vec2(9.f, 7.f)),
                    GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees));

    const SrcToDest expected_rectangle_1(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                         types::RectangleF({0.f, -7.f, 9.f, 7.f}),
                                         types::RotateFlip::kRotateCcw90());

    const auto rectangle_1 = GetSrcToDestForMatrix(test_1);
    EXPECT_EQ(rectangle_1, expected_rectangle_1);

    // Here we scale and then rotate so the scale winds up rotated.
    const glm::mat3 test_2 = glm::scale(
        glm::rotate(glm::mat3(),
                    GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees)),
        glm::vec2(9.f, 7.f));

    const SrcToDest expected_rectangle_2(types::RectangleF({0.f, 0.f, kImageWidth, kImageHeight}),
                                         types::RectangleF({0.f, -9.f, 7.f, 9.f}),
                                         types::RotateFlip::kRotateCcw90());

    const auto rectangle_2 = GetSrcToDestForMatrix(test_2);
    EXPECT_EQ(rectangle_2, expected_rectangle_2);
  }
}

// Ensure that when a transform node has two parents, that its data is duplicated in
// the global topology vector, with the proper global data (i.e. matrices, images,
// clip regions and hit regions) for each entry, respecting each separate chain
// up the hierarchy. This is used for A11Y Magnification.
TEST(SrcToDestTest, MultipleParentTest) {
  // Make a global topology representing the following graph.
  // We have a diamond pattern hierarchy where transform 1:4
  // is children to both 1:1 and 1:3.
  //
  // 1:0 - 1:1
  //     \    \
  //       1:3 - 1:4
  UberStruct::InstanceMap uber_structs;
  auto uber_struct = std::make_unique<UberStruct>();

  // Set up the uber struct with the above topology. Set the doubly-parented child (1,4) up
  // with an image, hit region and clip region to make sure those get duplicated properly.
  constexpr TransformClipRegion kClipRegion({.x = 5, .y = 10, .width = 30, .height = 40});
  const flatland::HitRegion kHitRegion({.x = 1, .y = 2, .width = 10, .height = 20});
  const float kScale = 2.0f;

  uber_struct->local_topology = {{{1, 0}, 2}, {{1, 1}, 1}, {{1, 4}, 0}, {{1, 3}, 1}, {{1, 4}, 0}};
  uber_struct->local_matrices[{1, 3}] = glm::mat3(kScale);
  uber_struct->local_hit_regions_map[{1, 4}] = {kHitRegion};
  uber_struct->local_clip_regions[{1, 4}] = kClipRegion;
  uber_structs[1] = std::move(uber_struct);

  auto global_topology_data =
      GlobalTopologyData::ComputeGlobalTopologyData(uber_structs, {}, {}, {1, 0});
  GlobalTopologyData::TopologyVector topology_vector = global_topology_data.topology_vector;
  auto parent_indices = global_topology_data.parent_indices;

  GlobalTopologyData::TopologyVector expected_topology_vector = {
      {1, 0}, {1, 1}, {1, 4}, {1, 3}, {1, 4}};
  GlobalTopologyData::ParentIndexVector expected_parent_indices = {0, 0, 1, 0, 3};

  for (uint32_t i = 0; i < topology_vector.size(); i++) {
    EXPECT_EQ(topology_vector[i], expected_topology_vector[i]);
  }

  for (uint32_t i = 0; i < parent_indices.size(); i++) {
    EXPECT_EQ(parent_indices[i], expected_parent_indices[i]);
  }

  // Each entry for the doubly parented node should have a different global matrix.
  const auto matrix_vector = ComputeGlobalMatrices(topology_vector, parent_indices, uber_structs);
  EXPECT_EQ(matrix_vector.size(), 5U);
  EXPECT_EQ(matrix_vector[2], glm::mat3(1.0));
  EXPECT_EQ(matrix_vector[4], glm::mat3(2.0));

  // Each entry for the doubly parented node should have different clip regions.
  {
    const auto clip_vector = ComputeGlobalTransformClipRegions(topology_vector, parent_indices,
                                                               matrix_vector, uber_structs);
    EXPECT_EQ(clip_vector.size(), 5U);

    // The first clip region should match exactly the clip region above.
    EXPECT_EQ(clip_vector[2], kClipRegion);

    // The second one should be magnified by the scale factor.
    EXPECT_EQ(clip_vector[4],
              TransformClipRegion({.x = static_cast<int32_t>(kScale * kClipRegion.x()),
                                   .y = static_cast<int32_t>(kScale * kClipRegion.y()),
                                   .width = static_cast<int32_t>(kScale * kClipRegion.width()),
                                   .height = static_cast<int32_t>(kScale * kClipRegion.height())}));
  }

  // Each entry for the doubly parented node should have different hit regions.
  {
    const auto hit_map =
        ComputeGlobalHitRegions(topology_vector, parent_indices, matrix_vector, uber_structs);
    auto itr = hit_map.find({1, 4});
    EXPECT_NE(itr, hit_map.end());

    auto vec = itr->second;
    EXPECT_EQ(vec.size(), 2U);

    const auto first = vec[0];
    const auto second = vec[1];

    // The first clip region should match exactly the hit region above.
    EXPECT_EQ(first.region(), kHitRegion.region());

    // The second one should be magnified by the scale factor.
    EXPECT_EQ(second.region(), kHitRegion.region().ScaledBy(kScale));
  }
}

// The following tests test for transform clip regions

// Test that an empty uber struct returns empty clip regions.
TEST(GlobalTransformClipTest, EmptyTopologyReturnsEmptyClipRegions) {
  UberStruct::InstanceMap uber_structs;
  GlobalTopologyData::TopologyVector topology_vector;
  GlobalTopologyData::ParentIndexVector parent_indices;
  GlobalMatrixVector global_matrices;

  auto global_clip_regions = ComputeGlobalTransformClipRegions(topology_vector, parent_indices,
                                                               global_matrices, uber_structs);
  EXPECT_TRUE(global_clip_regions.empty());
}

// Check that if there are no clip regions provided, they default to
// non-clipped regions.
TEST(GlobalTransformClipTest, EmptyClipRegionsAreInvalid) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing the following graph:
  //
  // 1:0 - 1:1
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {1, 1}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0};
  GlobalMatrixVector global_matrices = {glm::mat3(1.0), glm::mat3(1.0)};

  // The UberStruct for instance ID 1 must exist, but it contains no local opacity values.
  auto uber_struct = std::make_unique<UberStruct>();
  uber_structs[1] = std::move(uber_struct);

  GlobalTransformClipRegionVector expected_clip_regions = {kUnclippedRegion, kUnclippedRegion};

  auto global_clip_regions = ComputeGlobalTransformClipRegions(topology_vector, parent_indices,
                                                               global_matrices, uber_structs);
  EXPECT_EQ(expected_clip_regions.size(), global_clip_regions.size());
  for (uint32_t i = 0; i < global_clip_regions.size(); i++) {
    EXPECT_EQ(expected_clip_regions[i], global_clip_regions[i]);
  }
}

// The parent and child regions do not overlap, so the child region should
// be completely empty.
TEST(GlobalTransformClipTest, NoOverlapClipRegions) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing the following graph:
  //
  // 1:0 - 1:1
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {1, 1}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0};
  GlobalMatrixVector global_matrices = {glm::mat3(1.0), glm::mat3(1.0)};

  auto uber_struct = std::make_unique<UberStruct>();

  // The two regions do not overlap.
  GlobalTransformClipRegionVector clip_regions = {
      TransformClipRegion({.x = 0, .y = 0, .width = 100, .height = 200}),
      TransformClipRegion({.x = 200, .y = 300, .width = 100, .height = 200})};

  uber_struct->local_clip_regions[{1, 0}] = clip_regions[0];
  uber_struct->local_clip_regions[{1, 1}] = clip_regions[1];

  uber_structs[1] = std::move(uber_struct);

  GlobalTransformClipRegionVector expected_clip_regions = {
      clip_regions[0], TransformClipRegion({.x = 0, .y = 0, .width = 0, .height = 0})};

  auto global_clip_regions = ComputeGlobalTransformClipRegions(topology_vector, parent_indices,
                                                               global_matrices, uber_structs);
  EXPECT_EQ(global_clip_regions.size(), expected_clip_regions.size());
  for (uint64_t i = 0; i < global_clip_regions.size(); i++) {
    EXPECT_EQ(global_clip_regions[i], expected_clip_regions[i]);
  }

  // Now translate the child transform, to (-200, -300). Since the clip region's region is specified
  // to be (200,300) in the local coordinate space of the child transform, its global space should
  // therefore be (0,0) and it should line up with the clip region of the parent.
  global_matrices[1] = glm::translate(glm::mat3(1.0), glm::vec2(-200, -300));
  global_clip_regions = ComputeGlobalTransformClipRegions(topology_vector, parent_indices,
                                                          global_matrices, uber_structs);

  // Both clip regions should be the same.
  expected_clip_regions[1] = clip_regions[0];
  EXPECT_EQ(global_clip_regions.size(), expected_clip_regions.size());
  for (uint64_t i = 0; i < global_clip_regions.size(); i++) {
    EXPECT_EQ(global_clip_regions[i], expected_clip_regions[i]);
  }
}

// The following tests ensure scale and rotate modify the clip region as expected.

TEST(GlobalTransformClipTest, ScaleAndRotate90DegreesTest) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing a single node.
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0};

  const glm::vec2 scale(3.f, 2.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw90Degrees));
  matrix = glm::scale(matrix, scale);
  GlobalMatrixVector global_matrices = {matrix};

  auto uber_struct = std::make_unique<UberStruct>();

  uber_struct->local_clip_regions[{1, 0}] =
      TransformClipRegion({.x = 0, .y = 0, .width = 100, .height = 50});

  uber_structs[1] = std::move(uber_struct);

  GlobalTransformClipRegionVector expected_clip_regions = {
      TransformClipRegion({.x = 0, .y = -300, .width = 100, .height = 300})};

  auto global_clip_regions = ComputeGlobalTransformClipRegions(topology_vector, parent_indices,
                                                               global_matrices, uber_structs);
  EXPECT_EQ(global_clip_regions.size(), expected_clip_regions.size());
  EXPECT_EQ(global_clip_regions[0], expected_clip_regions[0]);
}

TEST(GlobalTransformClipTest, ScaleAndRotate180DegreesTest) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing a single node.
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0};

  const glm::vec2 scale(3.f, 2.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw180Degrees));
  matrix = glm::scale(matrix, scale);
  GlobalMatrixVector global_matrices = {matrix};

  auto uber_struct = std::make_unique<UberStruct>();

  uber_struct->local_clip_regions[{1, 0}] =
      TransformClipRegion({.x = 0, .y = 0, .width = 100, .height = 50});

  uber_structs[1] = std::move(uber_struct);

  GlobalTransformClipRegionVector expected_clip_regions = {
      TransformClipRegion({.x = -300, .y = -100, .width = 300, .height = 100})};

  auto global_clip_regions = ComputeGlobalTransformClipRegions(topology_vector, parent_indices,
                                                               global_matrices, uber_structs);
  EXPECT_EQ(global_clip_regions.size(), expected_clip_regions.size());
  EXPECT_EQ(global_clip_regions[0], expected_clip_regions[0]);
}

TEST(GlobalTransformClipTest, ScaleAndRotate270DegreesTest) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing a single node.
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0};

  const glm::vec2 scale(3.f, 2.f);
  glm::mat3 matrix = glm::rotate(
      glm::mat3(), GetOrientationAngleInViewSpaceCoordinates(Orientation::kCcw270Degrees));
  matrix = glm::scale(matrix, scale);
  GlobalMatrixVector global_matrices = {matrix};

  auto uber_struct = std::make_unique<UberStruct>();

  uber_struct->local_clip_regions[{1, 0}] =
      TransformClipRegion({.x = 0, .y = 0, .width = 100, .height = 50});

  uber_structs[1] = std::move(uber_struct);

  GlobalTransformClipRegionVector expected_clip_regions = {
      TransformClipRegion({.x = -100, .y = 0, .width = 100, .height = 300})};

  auto global_clip_regions = ComputeGlobalTransformClipRegions(topology_vector, parent_indices,
                                                               global_matrices, uber_structs);
  EXPECT_EQ(global_clip_regions.size(), expected_clip_regions.size());
  EXPECT_EQ(global_clip_regions[0], expected_clip_regions[0]);
}

// Test a more complicated scenario with multiple transforms, each with its own
// clip region and transform matrix set.
TEST(GlobalTransformClipTest, ComplicatedGraphClipRegions) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing the following graph:
  //
  // 1:0 - 1:1 - 1:2
  //     \
  //       1:3 - 1:4
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {1, 1}, {1, 2}, {1, 3}, {1, 4}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0, 1, 0, 3};
  GlobalMatrixVector global_matrices = {glm::translate(glm::mat3(1.0), glm::vec2(5, 10)),
                                        glm::translate(glm::mat3(1.0), glm::vec2(-5, -10)),
                                        glm::translate(glm::mat3(1.0), glm::vec2(20, 30)),
                                        glm::translate(glm::mat3(1.0), glm::vec2(-5, -10)),
                                        glm::translate(glm::mat3(1.0), glm::vec2(-10, -20))};

  auto uber_struct = std::make_unique<UberStruct>();

  GlobalTransformClipRegionVector clip_regions = {
      TransformClipRegion({0, 0, 100, 200}),   TransformClipRegion({-1000, -1000, 2000, 2000}),
      TransformClipRegion({0, 0, 110, 300}),   TransformClipRegion({-5, -10, 300, 400}),
      TransformClipRegion({-15, -30, 20, 30}),
  };

  uber_struct->local_clip_regions[{1, 0}] = clip_regions[0];

  uber_struct->local_clip_regions[{1, 1}] = clip_regions[1];
  uber_struct->local_clip_regions[{1, 2}] = clip_regions[2];

  uber_struct->local_clip_regions[{1, 3}] = clip_regions[3];
  uber_struct->local_clip_regions[{1, 4}] = clip_regions[4];

  uber_structs[1] = std::move(uber_struct);

  GlobalTransformClipRegionVector expected_clip_regions = {
      TransformClipRegion({.x = 5, .y = 10, .width = 100, .height = 200}),
      TransformClipRegion({.x = 5, .y = 10, .width = 100, .height = 200}),
      TransformClipRegion({.x = 20, .y = 30, .width = 85, .height = 180}),
      TransformClipRegion({.x = 5, .y = 10, .width = 100, .height = 200}),
      TransformClipRegion({.x = 0, .y = 0, .width = 0, .height = 0}),
  };

  auto global_clip_regions = ComputeGlobalTransformClipRegions(topology_vector, parent_indices,
                                                               global_matrices, uber_structs);
  EXPECT_EQ(global_clip_regions.size(), expected_clip_regions.size());
  for (uint64_t i = 0; i < global_clip_regions.size(); i++) {
    EXPECT_EQ(global_clip_regions[i], expected_clip_regions[i]);
  }
}

// We recreate several of the matrix tests above with opacity values here,
// since the logic for calculating opacities is largely the same as calculating
// matrices, where child values are the product of their local values and their
// ancestors' values.
//
// TODO(https://fxbug.dev/42153097): Since the logic between matrices and opacity is very similar,
// in the future we may want to consolidate |ComputeGlobalMatrices| and |ComputeGlobalOpacityValues|
// into a single (potentially templated) function, which would allow us to consolidate these tests
// into one. But for now, we have to keep them separate.

TEST(GlobalImageDataTest, EmptyTopologyReturnsEmptyOpacityValues) {
  UberStruct::InstanceMap uber_structs;
  GlobalTopologyData::TopologyVector topology_vector;
  GlobalTopologyData::ParentIndexVector parent_indices;

  auto global_opacity_values =
      ComputeGlobalOpacityValues(topology_vector, parent_indices, uber_structs);
  EXPECT_TRUE(global_opacity_values.empty());
}

// Check that if there are no opacity values provided, they default to 1.0 for
// parent and child.
TEST(GlobalImageDataTest, EmptyLocalOpacitiesAreOpaque) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing the following graph:
  //
  // 1:0 - 1:1
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {1, 1}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0};

  // The UberStruct for instance ID 1 must exist, but it contains no local opacity values.
  auto uber_struct = std::make_unique<UberStruct>();
  uber_structs[1] = std::move(uber_struct);

  // The root opacity value is set to 1.0, and the second inherits that.
  std::vector<float> expected_opacities = {
      1.f,
      1.f,
  };

  auto global_opacities = ComputeGlobalOpacityValues(topology_vector, parent_indices, uber_structs);
  EXPECT_THAT(global_opacities, ::testing::ElementsAreArray(expected_opacities));
}

// Test a more complicated scenario with multiple parent-child relationships and make
// sure all of the opacity values are being inherited properly.
TEST(GlobalImageDataTest, GlobalImagesIncludeParentImage) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing the following graph:
  //
  // 1:0 - 1:1 - 1:2
  //     \
  //       1:3 - 1:4
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {1, 1}, {1, 2}, {1, 3}, {1, 4}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0, 1, 0, 3};

  auto uber_struct = std::make_unique<UberStruct>();

  const float opacities[] = {0.9f, 0.8f, 0.7f, 0.6f, 0.5f};

  uber_struct->local_opacity_values[{1, 0}] = opacities[0];

  uber_struct->local_opacity_values[{1, 1}] = opacities[1];
  uber_struct->local_opacity_values[{1, 2}] = opacities[2];

  uber_struct->local_opacity_values[{1, 3}] = opacities[3];
  uber_struct->local_opacity_values[{1, 4}] = opacities[4];

  uber_structs[1] = std::move(uber_struct);

  std::vector<float> expected_opacities = {
      opacities[0],
      opacities[0] * opacities[1],
      opacities[0] * opacities[1] * opacities[2],
      opacities[0] * opacities[3],
      opacities[0] * opacities[3] * opacities[4],
  };

  auto global_opacities = ComputeGlobalOpacityValues(topology_vector, parent_indices, uber_structs);
  EXPECT_THAT(global_opacities, ::testing::ElementsAreArray(expected_opacities));
}

TEST(GlobalImageDataTest, GlobalImagesMultipleUberStructs) {
  UberStruct::InstanceMap uber_structs;

  // Make a global topology representing the following graph:
  //
  // 1:0 - 2:0
  //     \
  //       1:1
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {2, 0}, {1, 1}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0, 0};

  auto uber_struct1 = std::make_unique<UberStruct>();
  auto uber_struct2 = std::make_unique<UberStruct>();

  const float opacity_values[] = {0.5f, 0.3f, 0.9f};

  uber_struct1->local_opacity_values[{1, 0}] = opacity_values[0];
  uber_struct2->local_opacity_values[{2, 0}] = opacity_values[1];
  uber_struct1->local_opacity_values[{1, 1}] = opacity_values[2];

  uber_structs[1] = std::move(uber_struct1);
  uber_structs[2] = std::move(uber_struct2);

  std::vector<float> expected_opacity_values = {opacity_values[0],
                                                opacity_values[0] * opacity_values[1],
                                                opacity_values[0] * opacity_values[2]};

  auto global_opacity_values =
      ComputeGlobalOpacityValues(topology_vector, parent_indices, uber_structs);
  EXPECT_THAT(global_opacity_values, ::testing::ElementsAreArray(expected_opacity_values));
}

TEST(GlobalImageDataTest, OutputVectorVariantPopulatesCorrectly) {
  UberStruct::InstanceMap uber_structs;
  GlobalTopologyData::TopologyVector topology_vector = {{1, 0}, {2, 0}, {1, 1}};
  GlobalTopologyData::ParentIndexVector parent_indices = {0, 0, 0};

  auto uber_struct1 = std::make_unique<UberStruct>();
  auto uber_struct2 = std::make_unique<UberStruct>();

  const float opacity_values[] = {0.5f, 0.3f, 0.9f};

  uber_struct1->local_opacity_values[{1, 0}] = opacity_values[0];
  uber_struct2->local_opacity_values[{2, 0}] = opacity_values[1];
  uber_struct1->local_opacity_values[{1, 1}] = opacity_values[2];

  uber_structs[1] = std::move(uber_struct1);
  uber_structs[2] = std::move(uber_struct2);

  std::vector<float> expected_opacity_values = {opacity_values[0],
                                                opacity_values[0] * opacity_values[1],
                                                opacity_values[0] * opacity_values[2]};

  GlobalOpacityVector output;
  output.push_back(-1.0);  // verify that vector is cleared by ComputeGlobalOpacityValues().
  ComputeGlobalOpacityValues(output, topology_vector, parent_indices, uber_structs);
  EXPECT_THAT(output, ::testing::ElementsAreArray(expected_opacity_values));
}

}  // namespace test
}  // namespace flatland

#undef EXPECT_VEC2
