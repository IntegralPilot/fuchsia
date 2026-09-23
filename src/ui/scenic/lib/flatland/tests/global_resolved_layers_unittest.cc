// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This suite drives the engine scene walk using hand-built UberStructs (no Flatland sessions are
// instantiated). Flatland2 API semantics through FIDL are covered by the native Flatland2 test
// plan.  At this tier `GlobalRenderListTest.FlatlandVersionGatesImageReplace` is deliberately the
// only test case which is sensitive to `UberStruct::flatland_version`.

#include "src/ui/scenic/lib/flatland/global_resolved_layers.h"

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/ui/scenic/lib/display/fidl_id_types.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/global_matrix_data.h"

#include <glm/gtc/constants.hpp>
#include <glm/gtx/matrix_transform_2d.hpp>

using flatland::GlobalTopologyData;
using flatland::kUnclippedRegion;
using flatland::ResolveBlendAndOpacity;
using flatland::ResolvedLayer;
using flatland::SrcToDest;
using flatland::TransformClipRegion;
using flatland::TransformHandle;
using flatland::UberStruct;
using flatland::UberStructLayer;
using types::BlendMode;
using types::Rectangle;
using types::RectangleF;
using types::RotateFlip;

namespace flatland::test {
namespace {

// Convenience test helper which computes inherited opacities on the fly before calling
// flatland::ComputeGlobalResolvedLayers().
std::vector<ResolvedLayer> ComputeGlobalResolvedLayers(
    const GlobalTopologyData& topology, const UberStruct::InstanceMap& snapshot,
    const std::vector<glm::mat3>& global_matrices,
    const std::vector<TransformClipRegion>& clip_regions) {
  return flatland::ComputeGlobalResolvedLayers(
      topology, snapshot, global_matrices, clip_regions,
      ComputeGlobalOpacityValues(topology.topology_vector, topology.parent_indices, snapshot));
}

// Test behavior of the `ResolveBlendAndOpacity()` helper, which is used internally
// by `ComputeGlobalResolvedLayers()` to encapsulate the semantics of both the Flatland1
// and Flatland2 APIs.
TEST(ResolveBlendAndOpacityTest, ResolvesBlendAndOpacity) {
  // Matrix: stored_blend x effective_opacity {1.0, 0.5} x pin_replace {true, false}

  // 1. kReplace, opacity 1.0, pin_replace true
  {
    auto [blend, multiply] =
        ResolveBlendAndOpacity(BlendMode::kReplace(), 1.0f, /*pin_replace=*/true);
    EXPECT_EQ(blend, BlendMode::kReplace());
    EXPECT_EQ(multiply, (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
  }

  // 2. kReplace, opacity 1.0, pin_replace false
  {
    auto [blend, multiply] =
        ResolveBlendAndOpacity(BlendMode::kReplace(), 1.0f, /*pin_replace=*/false);
    EXPECT_EQ(blend, BlendMode::kReplace());
    EXPECT_EQ(multiply, (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
  }

  // 3. kReplace, opacity 0.5, pin_replace true (Flatland1 image case: blend stays REPLACE)
  {
    auto [blend, multiply] =
        ResolveBlendAndOpacity(BlendMode::kReplace(), 0.5f, /*pin_replace=*/true);
    EXPECT_EQ(blend, BlendMode::kReplace());
    EXPECT_EQ(multiply, (std::array<float, 4>{0.5f, 0.5f, 0.5f, 0.5f}));
  }

  // 4. kReplace, opacity 0.5, pin_replace false (Demoted to PREMULTIPLIED_ALPHA)
  {
    auto [blend, multiply] =
        ResolveBlendAndOpacity(BlendMode::kReplace(), 0.5f, /*pin_replace=*/false);
    EXPECT_EQ(blend, BlendMode::kPremultipliedAlpha());
    EXPECT_EQ(multiply, (std::array<float, 4>{0.5f, 0.5f, 0.5f, 0.5f}));
  }

  // 5. kPremultipliedAlpha, opacity 1.0, pin_replace false
  {
    auto [blend, multiply] =
        ResolveBlendAndOpacity(BlendMode::kPremultipliedAlpha(), 1.0f, /*pin_replace=*/false);
    EXPECT_EQ(blend, BlendMode::kPremultipliedAlpha());
    EXPECT_EQ(multiply, (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
  }

  // 6. kPremultipliedAlpha, opacity 0.5, pin_replace false
  {
    auto [blend, multiply] =
        ResolveBlendAndOpacity(BlendMode::kPremultipliedAlpha(), 0.5f, /*pin_replace=*/false);
    EXPECT_EQ(blend, BlendMode::kPremultipliedAlpha());
    EXPECT_EQ(multiply, (std::array<float, 4>{0.5f, 0.5f, 0.5f, 0.5f}));
  }

  // 7. kStraightAlpha, opacity 1.0, pin_replace false
  {
    auto [blend, multiply] =
        ResolveBlendAndOpacity(BlendMode::kStraightAlpha(), 1.0f, /*pin_replace=*/false);
    EXPECT_EQ(blend, BlendMode::kStraightAlpha());
    EXPECT_EQ(multiply, (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
  }

  // 8. kStraightAlpha, opacity 0.5, pin_replace false
  {
    auto [blend, multiply] =
        ResolveBlendAndOpacity(BlendMode::kStraightAlpha(), 0.5f, /*pin_replace=*/false);
    EXPECT_EQ(blend, BlendMode::kStraightAlpha());
    EXPECT_EQ(multiply, (std::array<float, 4>{1.f, 1.f, 1.f, 0.5f}));
  }
}

// TODO(https://fxbug.dev/523371761): Consider rewriting these tests to share
// constant values between the scene specification and the expectations; the
// stored SrcToDest uses the same RectangleF/RotateFlip types as the
// UberStructLayer inputs, so e.g. the same rectangle can appear in the
// UberStruct and in the EXPECT_EQ.

TEST(GlobalRenderListTest, EmptyScene) {
  GlobalTopologyData topology;
  topology.topology_vector = {{1, 0}};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{{1, 0}, 0}};
  snapshot[1] = std::move(uber_struct);

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f)};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  EXPECT_TRUE(result.empty());
}

TEST(GlobalRenderListTest, SingleImageLayerIdentityMatrix) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::ImageModeProperties{
              .sample_rect = RectangleF({.x = 0.f, .y = 0.f, .width = 100.f, .height = 200.f}),
              .transform = RotateFlip::kIdentity(),
              .image_id = display::ImageId(42),
              .image_width = 100,
              .image_height = 200,
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 1.f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f)};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 1u);

  const auto& layer = result[0];
  EXPECT_EQ(layer.geometry.dest, types::RectangleF({0.f, 0.f, 100.f, 200.f}));
  EXPECT_EQ(layer.geometry.transform, types::RotateFlip::kIdentity());
  EXPECT_EQ(layer.geometry.src, types::RectangleF({0.f, 0.f, 100.f, 200.f}));
  EXPECT_EQ(layer.multiply_color, (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
  EXPECT_EQ(layer.blend_mode, BlendMode::kReplace());
  EXPECT_EQ(layer.topology_index, 0);

  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(layer.content));
  const auto& content = std::get<ResolvedLayer::ImageContent>(layer.content);
  EXPECT_EQ(content.image_id, display::ImageId(42));
  EXPECT_EQ(content.width, 100u);
  EXPECT_EQ(content.height, 200u);
}

TEST(GlobalRenderListTest, TranslationAndScaleApplyToDisplayRect) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::ImageModeProperties{
              .sample_rect = RectangleF({.x = 0.f, .y = 0.f, .width = 100.f, .height = 200.f}),
              .transform = RotateFlip::kIdentity(),
              .image_id = display::ImageId(42),
              .image_width = 100,
              .image_height = 200,
          },
      .common =
          {
              .display_rect = Rectangle({.x = 10, .y = 20, .width = 100, .height = 200}),
              .opacity = 1.f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  // Parent matrix translation of (5, 5) and scale of (2, 3)
  glm::mat3 T = glm::translate(glm::mat3(1.f), {5.f, 5.f});
  glm::mat3 S = glm::scale(glm::mat3(1.f), {2.f, 3.f});

  std::vector<glm::mat3> global_matrices = {T * S};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 1u);

  const auto& layer = result[0];
  // Expected destination:
  // display_rect.x = 10 -> transformed x = 10 * 2 + 5 = 25
  // display_rect.y = 20 -> transformed y = 20 * 3 + 5 = 65
  // display_rect.width = 100 -> transformed width = 100 * 2 = 200
  // display_rect.height = 200 -> transformed height = 200 * 3 = 600
  EXPECT_EQ(layer.geometry.dest,
            types::RectangleF({.x = 25.f, .y = 65.f, .width = 200.f, .height = 600.f}));
}

TEST(GlobalRenderListTest, Rotation90ProducesOrientationAndPermutedUVs) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::ImageModeProperties{
              .sample_rect = RectangleF({.x = 10.f, .y = 20.f, .width = 100.f, .height = 200.f}),
              .transform = RotateFlip::kIdentity(),
              .image_id = display::ImageId(42),
              .image_width = 500,
              .image_height = 500,
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 1.f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  // Rotation of 90 degrees CCW
  float angle = glm::half_pi<float>();
  float s = sin(angle);
  float c = cos(angle);
  glm::mat3 R(1.f);
  R[0][0] = c;
  R[0][1] = s;
  R[1][0] = -s;
  R[1][1] = c;

  std::vector<glm::mat3> global_matrices = {R};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 1u);

  const auto& layer = result[0];
  EXPECT_EQ(layer.geometry.transform, types::RotateFlip::kRotateCcw270());
  EXPECT_EQ(layer.geometry.src, types::RectangleF({10.f, 20.f, 100.f, 200.f}));
}

TEST(GlobalRenderListTest, FlipComposesWithRotation) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  // Flip LEFT_RIGHT under a 90° CCW parent
  UberStructLayer uber_layer{
      .content =
          UberStructLayer::ImageModeProperties{
              .sample_rect = RectangleF({.x = 10.f, .y = 20.f, .width = 100.f, .height = 200.f}),
              .transform = RotateFlip::kReflectY(),  // LEFT_RIGHT
              .image_id = display::ImageId(42),
              .image_width = 500,
              .image_height = 500,
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 1.f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  // Rotation of 90 degrees CCW on parent
  float angle = glm::half_pi<float>();
  float s = sin(angle);
  float c = cos(angle);
  glm::mat3 R(1.f);
  R[0][0] = c;
  R[0][1] = s;
  R[1][0] = -s;
  R[1][1] = c;

  std::vector<glm::mat3> global_matrices = {R};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 1u);

  const auto& layer = result[0];
  EXPECT_EQ(layer.geometry.transform, types::RotateFlip::kRotateCcw90ReflectY());
  EXPECT_EQ(layer.geometry.src, types::RectangleF({10.f, 20.f, 100.f, 200.f}));
}

TEST(GlobalRenderListTest, ClipShrinksDstAndUVsProportionally) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::ImageModeProperties{
              .sample_rect = RectangleF({.x = 0.f, .y = 0.f, .width = 100.f, .height = 200.f}),
              .transform = RotateFlip::kIdentity(),
              .image_id = display::ImageId(42),
              .image_width = 100,
              .image_height = 200,
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 1.f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f)};
  // Clip region cuts off the left half (x starts at 50) and bottom half (height is 100)
  std::vector<TransformClipRegion> clip_regions = {TransformClipRegion({50, 0, 50, 100})};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 1u);

  const auto& layer = result[0];
  EXPECT_EQ(layer.geometry.dest, types::RectangleF({50.f, 0.f, 50.f, 100.f}));
  EXPECT_EQ(layer.geometry.transform, types::RotateFlip::kIdentity());
  EXPECT_EQ(layer.geometry.src, types::RectangleF({50.f, 0.f, 50.f, 100.f}));
}

TEST(GlobalRenderListTest, ClipToEmptyDropsLayer) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::ImageModeProperties{
              .sample_rect = RectangleF({.x = 0.f, .y = 0.f, .width = 100.f, .height = 200.f}),
              .transform = RotateFlip::kIdentity(),
              .image_id = display::ImageId(42),
              .image_width = 100,
              .image_height = 200,
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 1.f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f)};
  // Clip completely outside the image bounds
  std::vector<TransformClipRegion> clip_regions = {TransformClipRegion({200, 200, 50, 50})};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  EXPECT_TRUE(result.empty());
}

TEST(GlobalRenderListTest, OpacityMultipliesDownTheChain) {
  const TransformHandle kRoot = {1, 0};
  const TransformHandle kChild = {1, 1};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot, kChild};
  topology.parent_indices = {0, 0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_shared<UberStruct>();
  uber_struct->local_topology = {{kRoot, 1}, {kChild, 0}};
  uber_struct->local_opacity_values[kRoot] = 0.5f;

  const LayerHandle kLayer1(1, 1);
  const LayerHandle kLayer2(1, 2);
  uber_struct->layer_stacks[kChild] = {kLayer1, kLayer2};

  UberStructLayer uber_layer1{
      .content =
          UberStructLayer::ImageModeProperties{
              .sample_rect = RectangleF({.x = 0.f, .y = 0.f, .width = 100.f, .height = 200.f}),
              .transform = RotateFlip::kIdentity(),
              .image_id = display::ImageId(42),
              .image_width = 100,
              .image_height = 200,
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 0.5f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  UberStructLayer uber_layer2{
      .content =
          UberStructLayer::SolidColorModeProperties{
              .color = {1.f, 1.f, 1.f, 1.f},
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 0.5f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer1] = uber_layer1;
  uber_struct->layers[kLayer2] = uber_layer2;
  snapshot[1] = uber_struct;

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f), glm::mat3(1.f)};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion, kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 2u);

  // 1. Verify ImageContent layer
  //    (effective opacity 0.25 < 1.0; flatland_version 1 -> blend mode remains REPLACE)
  {
    const auto& layer = result[0];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 0.25f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kReplace());
    EXPECT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(layer.content));
  }

  // 2. Verify SolidColorContent layer
  //    (effective opacity 0.25 < 1.0; demotes to PREMULTIPLIED_ALPHA)
  {
    const auto& layer = result[1];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 0.25f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kPremultipliedAlpha());

    ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content));
    const auto& solid_content = std::get<ResolvedLayer::SolidColorContent>(layer.content);
    EXPECT_FLOAT_EQ(solid_content.color[0], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[1], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[2], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[3], 1.f);
  }

  // For the next sub-tests, we change `flatland_version == 2` to demonstrate that image REPLACE
  // is handled differently.
  uber_struct->flatland_version = 2;
  result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 2u);

  // 3. Verify ImageContent layer
  //    (effective opacity 0.25 < 1.0; flatland_version 2 -> demotes to PREMULTIPLIED_ALPHA)
  {
    const auto& layer = result[0];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 0.25f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kPremultipliedAlpha());
    EXPECT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(layer.content));
  }

  // 4. Verify SolidColorContent layer (for completeness: identical to Flatland 1)
  //    (effective opacity 0.25 < 1.0; demotes to PREMULTIPLIED_ALPHA)
  {
    const auto& layer = result[1];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 0.25f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 0.25f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kPremultipliedAlpha());

    ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content));
    const auto& solid_content = std::get<ResolvedLayer::SolidColorContent>(layer.content);
    EXPECT_FLOAT_EQ(solid_content.color[0], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[1], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[2], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[3], 1.f);
  }

  // For the next sub-tests, we change the inherited transform opacity to 1.f.  Demotion from
  // REPLACE -> PREMULTIPLIED will still occur, because per-layer opacity is still set to < 1.0
  uber_struct->local_opacity_values[kRoot] = 1.f;
  result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 2u);

  // 5. Verify ImageContent layer
  //    (effective opacity 0.5 < 1.0; flatland_version 2 -> demotes to PREMULTIPLIED_ALPHA)
  {
    const auto& layer = result[0];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 0.5f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 0.5f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 0.5f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 0.5f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kPremultipliedAlpha());
    EXPECT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(layer.content));
  }

  // 6. Verify SolidColorContent layer (for completeness: identical to Flatland 1)
  //    (effective opacity 0.5 < 1.0; demotes to PREMULTIPLIED_ALPHA)
  {
    const auto& layer = result[1];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 0.5f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 0.5f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 0.5f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 0.5f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kPremultipliedAlpha());

    ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content));
    const auto& solid_content = std::get<ResolvedLayer::SolidColorContent>(layer.content);
    EXPECT_FLOAT_EQ(solid_content.color[0], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[1], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[2], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[3], 1.f);
  }

  // For the next sub-tests, we change per-layer opacity to 1.f.  Now, finally, demotion from
  // REPLACE -> PREMULTIPLIED will no longer occur, because the effective opacity is 1.0
  uber_struct->layers[kLayer1].common.opacity = 1.f;
  uber_struct->layers[kLayer2].common.opacity = 1.f;

  uber_struct->local_opacity_values[kRoot] = 1.f;
  result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 2u);

  // 7. Verify ImageContent layer
  //    (effective opacity 1.0; no blend mode demotion so remains REPLACE)
  {
    const auto& layer = result[0];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 1.f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 1.f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 1.f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 1.f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kReplace());
    EXPECT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(layer.content));
  }

  // 8. Verify SolidColorContent layer
  //    (effective opacity 1.0; no blend mode demotion so remains REPLACE)
  {
    const auto& layer = result[1];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 1.f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 1.f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 1.f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 1.f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kReplace());

    ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content));
    const auto& solid_content = std::get<ResolvedLayer::SolidColorContent>(layer.content);
    EXPECT_FLOAT_EQ(solid_content.color[0], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[1], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[2], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[3], 1.f);
  }

  // For the final sub-tests, for completeness we change the inherited transform opacity to 0.123f
  // This demonstrates that either layer opacity < 1 or inherited transform opacity < 1 makes the
  // effective opacity < 1, and therefore triggers demotion from REPLACE -> PREMULTIPLIED.
  uber_struct->local_opacity_values[kRoot] = 0.123f;
  result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 2u);

  // 9. Verify ImageContent layer
  //    (effective opacity 0.123 < 1.0; flatland_version 2 -> demotes to PREMULTIPLIED_ALPHA)
  {
    const auto& layer = result[0];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 0.123f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 0.123f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 0.123f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 0.123f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kPremultipliedAlpha());
    EXPECT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(layer.content));
  }

  // 10. Verify SolidColorContent layer (for completeness: identical to Flatland 1)
  //     (effective opacity 0.123 < 1.0; demotes to PREMULTIPLIED_ALPHA)
  {
    const auto& layer = result[1];
    EXPECT_FLOAT_EQ(layer.multiply_color[0], 0.123f);
    EXPECT_FLOAT_EQ(layer.multiply_color[1], 0.123f);
    EXPECT_FLOAT_EQ(layer.multiply_color[2], 0.123f);
    EXPECT_FLOAT_EQ(layer.multiply_color[3], 0.123f);
    EXPECT_EQ(layer.blend_mode, BlendMode::kPremultipliedAlpha());

    ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content));
    const auto& solid_content = std::get<ResolvedLayer::SolidColorContent>(layer.content);
    EXPECT_FLOAT_EQ(solid_content.color[0], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[1], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[2], 1.f);
    EXPECT_FLOAT_EQ(solid_content.color[3], 1.f);
  }
}

TEST(GlobalRenderListTest, EffectiveOpacityCombinesLayerAndInheritedOpacity) {
  const TransformHandle kRoot = {1, 0};
  const TransformHandle kChild = {1, 1};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot, kChild};
  topology.parent_indices = {0, 0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_shared<UberStruct>();
  uber_struct->local_topology = {{kRoot, 1}, {kChild, 0}};
  uber_struct->local_opacity_values[kRoot] = 0.5f;

  const LayerHandle kImageLayer(1, 1);
  const LayerHandle kSolidLayer(1, 2);
  uber_struct->layer_stacks[kChild] = {kImageLayer, kSolidLayer};

  uber_struct->layers[kImageLayer] = UberStructLayer{
      .content =
          UberStructLayer::ImageModeProperties{
              .sample_rect = RectangleF({.x = 0.f, .y = 0.f, .width = 100.f, .height = 200.f}),
              .transform = RotateFlip::kIdentity(),
              .image_id = display::ImageId(42),
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 0.5f,
              .blend_mode = BlendMode::kPremultipliedAlpha(),
          },
  };

  uber_struct->layers[kSolidLayer] = UberStructLayer{
      .content =
          UberStructLayer::SolidColorModeProperties{
              .color = {0.5f, 0.25f, 1.f, 0.8f},
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 0.5f,
              .blend_mode = BlendMode::kPremultipliedAlpha(),
          },
  };
  snapshot[1] = uber_struct;

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f), glm::mat3(1.f)},
                                            {kUnclippedRegion, kUnclippedRegion});
  ASSERT_EQ(result.size(), 2u);

  // Assert both layers receive effective opacity = 0.5 * 0.5 = 0.25
  EXPECT_EQ(result[0].multiply_color, (std::array<float, 4>{0.25f, 0.25f, 0.25f, 0.25f}));
  EXPECT_EQ(result[1].multiply_color, (std::array<float, 4>{0.25f, 0.25f, 0.25f, 0.25f}));

  // Solid content color is premultiplied by its own alpha (0.8)
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(result[1].content));
  const auto& solid_content = std::get<ResolvedLayer::SolidColorContent>(result[1].content);
  EXPECT_FLOAT_EQ(solid_content.color[0], 0.4f);
  EXPECT_FLOAT_EQ(solid_content.color[1], 0.2f);
  EXPECT_FLOAT_EQ(solid_content.color[2], 0.8f);
  EXPECT_FLOAT_EQ(solid_content.color[3], 0.8f);
}

TEST(GlobalRenderListTest, InvisibleLayersSkipped) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  // 1. Empty display_rect
  {
    UberStruct::InstanceMap snapshot;
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology = {{kRoot, 0}};
    const LayerHandle kLayer1(1, 1);
    const LayerHandle kLayer2(1, 2);
    uber_struct->layer_stacks[kRoot] = {kLayer1, kLayer2};
    UberStructLayer uber_layer1{
        .content =
            UberStructLayer::ImageModeProperties{
                .transform = RotateFlip::kIdentity(),
                .image_id = display::ImageId(42),
            },
        // width is 0, therefore rect is considered empty.
        .common =
            {
                .display_rect = Rectangle({.x = 0, .y = 0, .width = 0, .height = 200}),
                .opacity = 1.f,
            },
    };
    UberStructLayer uber_layer2{
        .content =
            UberStructLayer::SolidColorModeProperties{
                .color = {1.f, 1.f, 1.f, 1.f},
            },
        // height is 0, therefore rect is considered empty.
        .common =
            {
                .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 0}),
                .opacity = 1.f,
            },
    };
    uber_struct->layers[kLayer1] = uber_layer1;
    uber_struct->layers[kLayer2] = uber_layer2;
    snapshot[1] = std::move(uber_struct);
    auto result =
        ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f)}, {kUnclippedRegion});
    EXPECT_TRUE(result.empty());
  }

  // 2. Opacity = 0
  {
    UberStruct::InstanceMap snapshot;
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology = {{kRoot, 0}};
    const LayerHandle kLayer1(1, 1);
    const LayerHandle kLayer2(1, 2);
    uber_struct->layer_stacks[kRoot] = {kLayer1, kLayer2};
    UberStructLayer uber_layer1{
        .content =
            UberStructLayer::ImageModeProperties{
                .transform = RotateFlip::kIdentity(),
                .image_id = display::ImageId(42),
            },
        .common =
            {
                .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
                .opacity = 0.f,
            },
    };
    UberStructLayer uber_layer2{
        .content =
            UberStructLayer::SolidColorModeProperties{
                .color = {1.f, 1.f, 1.f, 1.f},
            },
        .common =
            {
                .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
                .opacity = 0.f,
                .blend_mode = BlendMode::kReplace(),
            },
    };
    uber_struct->layers[kLayer1] = uber_layer1;
    uber_struct->layers[kLayer2] = uber_layer2;
    snapshot[1] = std::move(uber_struct);
    auto result =
        ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f)}, {kUnclippedRegion});
    EXPECT_TRUE(result.empty());
  }

  // 3. Unbound image (image_id = kInvalidImageId)
  {
    UberStruct::InstanceMap snapshot;
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology = {{kRoot, 0}};
    const LayerHandle kLayer(1, 1);
    uber_struct->layer_stacks[kRoot] = {kLayer};
    UberStructLayer uber_layer{
        .content =
            UberStructLayer::ImageModeProperties{
                .transform = RotateFlip::kIdentity(),
                .image_id = allocation::kInvalidImageId,
            },
        .common =
            {
                .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
                .opacity = 1.f,
            },
    };
    uber_struct->layers[kLayer] = uber_layer;
    snapshot[1] = std::move(uber_struct);
    auto result =
        ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f)}, {kUnclippedRegion});
    EXPECT_TRUE(result.empty());
  }

  // 4. Hole-punch non-skip: kReplace solid with color.a == 0 and opacity == 1 is emitted verbatim.
  // Written alpha is still the authored 0; premultiplying zeros the RGB, which prevents non-zero
  // RGB at alpha 0 from additively tinting the underlay the punch is supposed to reveal.
  {
    UberStruct::InstanceMap snapshot;
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology = {{kRoot, 0}};
    const LayerHandle kLayer(1, 1);
    uber_struct->layer_stacks[kRoot] = {kLayer};
    UberStructLayer uber_layer{
        .content =
            UberStructLayer::SolidColorModeProperties{
                .color = {0.5f, 0.25f, 1.f, 0.f},
            },
        .common =
            {
                .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
                .opacity = 1.f,
                .blend_mode = BlendMode::kReplace(),
            },
    };
    uber_struct->layers[kLayer] = uber_layer;
    snapshot[1] = std::move(uber_struct);
    auto result =
        ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f)}, {kUnclippedRegion});
    ASSERT_EQ(result.size(), 1u);
    const auto& layer = result[0];
    EXPECT_EQ(layer.blend_mode, BlendMode::kReplace());
    ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content));
    const auto& content = std::get<ResolvedLayer::SolidColorContent>(layer.content);
    EXPECT_FLOAT_EQ(content.color[0], 0.f);
    EXPECT_FLOAT_EQ(content.color[1], 0.f);
    EXPECT_FLOAT_EQ(content.color[2], 0.f);
    EXPECT_FLOAT_EQ(content.color[3], 0.f);
  }
}

TEST(GlobalRenderListTest, InheritedOpacityZeroSkipsLayers) {
  const TransformHandle kRoot = {1, 0};
  const TransformHandle kChild = {1, 1};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot, kChild};
  topology.parent_indices = {0, 0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_shared<UberStruct>();
  uber_struct->local_topology = {{kRoot, 1}, {kChild, 0}};
  uber_struct->local_opacity_values[kRoot] = 0.f;

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kChild] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::SolidColorModeProperties{
              .color = {1.f, 1.f, 1.f, 1.f},
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 1.f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = uber_struct;

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f), glm::mat3(1.f)};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion, kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  EXPECT_TRUE(result.empty());
}

TEST(GlobalRenderListTest, StackZOrderIsBackToFront) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  // Three layers in stack
  const LayerHandle kLayer1(1, 1);
  const LayerHandle kLayer2(1, 2);
  const LayerHandle kLayer3(1, 3);
  uber_struct->layer_stacks[kRoot] = {kLayer1, kLayer2, kLayer3};

  const auto make_layer = [](display::ImageId image_id) {
    UberStructLayer layer;
    layer.content = UberStructLayer::ImageModeProperties{
        .transform = RotateFlip::kIdentity(),
        .image_id = image_id,
    };
    layer.common.display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200});
    layer.common.opacity = 1.f;
    return layer;
  };

  uber_struct->layers[kLayer1] = make_layer(display::ImageId(11));
  uber_struct->layers[kLayer2] = make_layer(display::ImageId(22));
  uber_struct->layers[kLayer3] = make_layer(display::ImageId(33));
  snapshot[1] = std::move(uber_struct);

  auto result =
      ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f)}, {kUnclippedRegion});
  ASSERT_EQ(result.size(), 3u);
  // Emits back-to-front (first layer in stack is furthest back, renders first)
  EXPECT_EQ(std::get<ResolvedLayer::ImageContent>(result[0].content).image_id,
            display::ImageId(11));
  EXPECT_EQ(std::get<ResolvedLayer::ImageContent>(result[1].content).image_id,
            display::ImageId(22));
  EXPECT_EQ(std::get<ResolvedLayer::ImageContent>(result[2].content).image_id,
            display::ImageId(33));
}

TEST(GlobalRenderListTest, DagInstancingEmitsPerPath) {
  const TransformHandle kParent1 = {1, 0};
  const TransformHandle kParent2 = {1, 1};
  const TransformHandle kChild = {1, 2};

  GlobalTopologyData topology;
  topology.topology_vector = {kParent1, kChild, kParent2, kChild};
  topology.parent_indices = {0, 0, 0, 2};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kParent1, 1}, {kChild, 0}, {kParent2, 1}, {kChild, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kChild] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::ImageModeProperties{
              .transform = RotateFlip::kIdentity(),
              .image_id = display::ImageId(42),
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 1.f,
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  // Two different parent matrices
  glm::mat3 M1 = glm::translate(glm::mat3(1.f), {10.f, 0.f});
  glm::mat3 M2 = glm::translate(glm::mat3(1.f), {50.f, 0.f});

  std::vector<glm::mat3> global_matrices = {M1, M1, M2, M2};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion, kUnclippedRegion,
                                                   kUnclippedRegion, kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  // Emits twice (once per topological index of child)
  ASSERT_EQ(result.size(), 2u);

  EXPECT_EQ(result[0].geometry.dest,
            types::RectangleF({.x = 10.f, .y = 0.f, .width = 100.f, .height = 200.f}));
  EXPECT_EQ(result[0].topology_index, 1);

  EXPECT_EQ(result[1].geometry.dest,
            types::RectangleF({.x = 50.f, .y = 0.f, .width = 100.f, .height = 200.f}));
  EXPECT_EQ(result[1].topology_index, 3);
}

// Pins that a solid layer with REPLACE blend mode and effective opacity < 1 is demoted to
// PREMULTIPLIED_ALPHA.
TEST(GlobalRenderListTest, SolidColorLayer_DemotedReplace) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::SolidColorModeProperties{
              .color = {0.5f, 0.25f, 1.f, 0.8f},
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 0.5f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f)};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 1u);

  const auto& layer = result[0];
  EXPECT_EQ(layer.multiply_color, (std::array<float, 4>{0.5f, 0.5f, 0.5f, 0.5f}));
  EXPECT_EQ(layer.blend_mode, BlendMode::kPremultipliedAlpha());

  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content));
  const auto& content = std::get<ResolvedLayer::SolidColorContent>(layer.content);
  // Content color is premultiplied by its own alpha (0.8):
  EXPECT_FLOAT_EQ(content.color[0], 0.4f);
  EXPECT_FLOAT_EQ(content.color[1], 0.2f);
  EXPECT_FLOAT_EQ(content.color[2], 0.8f);
  EXPECT_FLOAT_EQ(content.color[3], 0.8f);
}

// Pins that a solid layer with REPLACE blend mode and effective opacity == 1 emits surviving
// REPLACE blend mode with premultiplied content color.
TEST(GlobalRenderListTest, SolidColorLayer_SurvivingReplace) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::SolidColorModeProperties{
              .color = {0.5f, 0.25f, 1.f, 0.8f},
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 1.0f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f)};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 1u);

  const auto& layer = result[0];
  EXPECT_EQ(layer.multiply_color, (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
  EXPECT_EQ(layer.blend_mode, BlendMode::kReplace());

  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content));
  const auto& content = std::get<ResolvedLayer::SolidColorContent>(layer.content);
  // Content color is premultiplied by its own alpha (0.8):
  EXPECT_FLOAT_EQ(content.color[0], 0.4f);
  EXPECT_FLOAT_EQ(content.color[1], 0.2f);
  EXPECT_FLOAT_EQ(content.color[2], 0.8f);
  EXPECT_FLOAT_EQ(content.color[3], 0.8f);
}

// A hole punch: content alpha 0 under REPLACE. The layer is emitted
// (invisibility keys on layer and inherited opacity, never on content
// alpha), and premultiplication by content alpha zeroes the RGB
// channels, so the authored color is irrelevant: {0,0,0,0} is written
// verbatim, cutting a transparent hole for an underlay.
TEST(GlobalRenderListTest, SolidColorLayer_AlphaZeroPunch) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::SolidColorModeProperties{
              .color = {0.5f, 0.25f, 1.f, 0.f},
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 1.0f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = std::move(uber_struct);

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f)};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion};

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, global_matrices, clip_regions);
  ASSERT_EQ(result.size(), 1u);

  const auto& layer = result[0];
  EXPECT_EQ(layer.multiply_color, (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
  EXPECT_EQ(layer.blend_mode, BlendMode::kReplace());

  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content));
  const auto& content = std::get<ResolvedLayer::SolidColorContent>(layer.content);
  EXPECT_FLOAT_EQ(content.color[0], 0.f);
  EXPECT_FLOAT_EQ(content.color[1], 0.f);
  EXPECT_FLOAT_EQ(content.color[2], 0.f);
  EXPECT_FLOAT_EQ(content.color[3], 0.f);
}

TEST(GlobalRenderListTest, SolidColorLayer_StraightAlphaNormalized) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  const LayerHandle kLayer(1, 1);
  const Rectangle kDisplayRect({.x = 0, .y = 0, .width = 100, .height = 200});
  const UberStructLayer::SolidColorModeProperties kSolidColor{.color = {1.f, 0.f, 0.f, 0.5f}};
  const float kOpacity = 0.5f;

  UberStruct::InstanceMap straight_snapshot;
  {
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology = {{kRoot, 0}};
    uber_struct->layer_stacks[kRoot] = {kLayer};
    uber_struct->layers[kLayer] = UberStructLayer{
        .content = kSolidColor,
        .common =
            {
                .display_rect = kDisplayRect,
                .opacity = kOpacity,
                .blend_mode = BlendMode::kStraightAlpha(),
            },
    };
    straight_snapshot[1] = std::move(uber_struct);
  }

  UberStruct::InstanceMap premul_snapshot;
  {
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology = {{kRoot, 0}};
    uber_struct->layer_stacks[kRoot] = {kLayer};
    uber_struct->layers[kLayer] = UberStructLayer{
        .content = kSolidColor,
        .common =
            {
                .display_rect = kDisplayRect,
                .opacity = kOpacity,
                .blend_mode = BlendMode::kPremultipliedAlpha(),
            },
    };
    premul_snapshot[1] = std::move(uber_struct);
  }

  std::vector<glm::mat3> global_matrices = {glm::mat3(1.f)};
  std::vector<TransformClipRegion> clip_regions = {kUnclippedRegion};

  auto straight_result =
      ComputeGlobalResolvedLayers(topology, straight_snapshot, global_matrices, clip_regions);
  ASSERT_EQ(straight_result.size(), 1u);

  auto premul_result =
      ComputeGlobalResolvedLayers(topology, premul_snapshot, global_matrices, clip_regions);
  ASSERT_EQ(premul_result.size(), 1u);

  // Directly assert the FIDL contract: STRAIGHT_ALPHA composites identically to
  // PREMULTIPLIED_ALPHA for solid-color content.
  EXPECT_EQ(straight_result[0], premul_result[0]);

  // For readable failures, also assert on the straight-alpha result directly.
  const auto& resolved_layer = straight_result[0];
  EXPECT_EQ(resolved_layer.blend_mode, BlendMode::kPremultipliedAlpha());
  EXPECT_EQ(resolved_layer.multiply_color, (std::array<float, 4>{0.5f, 0.5f, 0.5f, 0.5f}));

  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(resolved_layer.content));
  const auto& content = std::get<ResolvedLayer::SolidColorContent>(resolved_layer.content);
  // Content color is premultiplied by its own alpha (0.5):
  EXPECT_FLOAT_EQ(content.color[0], 0.5f);
  EXPECT_FLOAT_EQ(content.color[1], 0.f);
  EXPECT_FLOAT_EQ(content.color[2], 0.f);
  EXPECT_FLOAT_EQ(content.color[3], 0.5f);
}

TEST(GlobalRenderListDeathTest, DanglingLayerHandleDies) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};
  // Intentionally omit putting kLayer in uber_struct->layers.

  snapshot[1] = std::move(uber_struct);

  EXPECT_DEATH(
      ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f)}, {kUnclippedRegion}), "");
}

TEST(GlobalRenderListTest, MultipleSessionsMerge) {
  const TransformHandle kRoot1 = {1, 0};
  const TransformHandle kRoot2 = {2, 0};

  GlobalTopologyData topology;
  topology.topology_vector = {kRoot1, kRoot2};
  topology.parent_indices = {0, 0};

  UberStruct::InstanceMap snapshot;

  // Session 1
  {
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology = {{kRoot1, 0}};
    const LayerHandle kLayer(1, 1);
    uber_struct->layer_stacks[kRoot1] = {kLayer};
    UberStructLayer uber_layer{
        .content =
            UberStructLayer::ImageModeProperties{
                .transform = RotateFlip::kIdentity(),
                .image_id = display::ImageId(11),
            },
        .common =
            {
                .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
                .opacity = 1.f,
            },
    };
    uber_struct->layers[kLayer] = uber_layer;
    snapshot[1] = std::move(uber_struct);
  }

  // Session 2
  {
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology = {{kRoot2, 0}};
    const LayerHandle kLayer(2, 1);
    uber_struct->layer_stacks[kRoot2] = {kLayer};
    UberStructLayer uber_layer{
        .content =
            UberStructLayer::ImageModeProperties{
                .transform = RotateFlip::kIdentity(),
                .image_id = display::ImageId(22),
            },
        .common =
            {
                .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
                .opacity = 1.f,
            },
    };
    uber_struct->layers[kLayer] = uber_layer;
    snapshot[2] = std::move(uber_struct);
  }

  auto result = ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f), glm::mat3(1.f)},
                                            {kUnclippedRegion, kUnclippedRegion});
  ASSERT_EQ(result.size(), 2u);
  EXPECT_EQ(std::get<ResolvedLayer::ImageContent>(result[0].content).image_id,
            display::ImageId(11));
  EXPECT_EQ(std::get<ResolvedLayer::ImageContent>(result[1].content).image_id,
            display::ImageId(22));
}

TEST(ResolvedLayerTest, EqualityComparesAllFields) {
  ResolvedLayer layer1;
  layer1.geometry = SrcToDest(types::RectangleF({0, 0, 10, 10}));
  layer1.multiply_color = {1.f, 1.f, 1.f, 1.f};
  layer1.blend_mode = BlendMode::kReplace();
  layer1.content = ResolvedLayer::ImageContent{.image_id = display::ImageId(1)};

  ResolvedLayer layer2 = layer1;
  EXPECT_EQ(layer1, layer2);

  // Flip each field and verify inequality:

  // 1. geometry
  layer2 = layer1;
  layer2.geometry = SrcToDest(types::RectangleF({1, 0, 10, 10}));
  EXPECT_NE(layer1, layer2);

  // 2. color
  layer2 = layer1;
  layer2.multiply_color = {0.f, 1.f, 1.f, 1.f};
  EXPECT_NE(layer1, layer2);

  // 3. blend_mode
  layer2 = layer1;
  layer2.blend_mode = BlendMode::kPremultipliedAlpha();
  EXPECT_NE(layer1, layer2);

  // 4. content variant alternative type (ImageContent -> SolidColorContent)
  layer2 = layer1;
  layer2.content = ResolvedLayer::SolidColorContent{.color = {1.f, 1.f, 1.f, 1.f}};
  EXPECT_NE(layer1, layer2);

  // 5. content inner fields (ImageContent image_id)
  layer2 = layer1;
  layer2.content = ResolvedLayer::ImageContent{.image_id = display::ImageId(2)};
  EXPECT_NE(layer1, layer2);

  // 6. topology_index
  layer2 = layer1;
  layer2.topology_index = 42;
  EXPECT_NE(layer1, layer2);
}

// DecomposeRotateFlip must invert RotateFlip::From for every symmetry, since the
// display path recomposes via From(orientation, flip).
TEST(DecomposeRotateFlipTest, InvertsRotateFlipFrom) {
  const types::RotateFlip::Enum kAllEnums[] = {
      types::RotateFlip::Enum::kIdentity,
      types::RotateFlip::Enum::kReflectX,
      types::RotateFlip::Enum::kReflectY,
      types::RotateFlip::Enum::kRotateCcw180,
      types::RotateFlip::Enum::kRotateCcw90,
      types::RotateFlip::Enum::kRotateCcw90ReflectX,
      types::RotateFlip::Enum::kRotateCcw90ReflectY,
      types::RotateFlip::Enum::kRotateCcw270,
  };
  for (auto enum_val : kAllEnums) {
    types::RotateFlip rf(enum_val);
    auto [orientation, flip] = flatland::DecomposeRotateFlip(rf);
    EXPECT_EQ(types::RotateFlip::From(orientation, flip), rf) << static_cast<int>(enum_val);
  }
}

// GetLayerLocalMatrix must match the reference T * R * S composition.
TEST(GetLayerLocalMatrixTest, MatchesReferenceComposition) {
  const types::Rectangle rect({.x = 10, .y = 20, .width = 100, .height = 200});
  const fuchsia_ui_composition::Orientation orientations[] = {
      fuchsia_ui_composition::Orientation::kCcw0Degrees,
      fuchsia_ui_composition::Orientation::kCcw90Degrees,
      fuchsia_ui_composition::Orientation::kCcw180Degrees,
      fuchsia_ui_composition::Orientation::kCcw270Degrees,
  };

  for (auto orientation : orientations) {
    // Reference Translate
    glm::mat3 T = glm::translate(glm::mat3(1.f), {10.f, 20.f});

    // Reference Rotate
    float angle = 0.f;
    if (orientation == fuchsia_ui_composition::Orientation::kCcw90Degrees) {
      angle = -glm::half_pi<float>();
    } else if (orientation == fuchsia_ui_composition::Orientation::kCcw180Degrees) {
      angle = -glm::pi<float>();
    } else if (orientation == fuchsia_ui_composition::Orientation::kCcw270Degrees) {
      angle = -glm::three_over_two_pi<float>();
    }

    float s = sin(angle);
    float c = cos(angle);
    glm::mat3 R(1.f);
    R[0][0] = c;
    R[0][1] = s;
    R[1][0] = -s;
    R[1][1] = c;

    // Reference Scale
    glm::mat3 S = glm::scale(glm::mat3(1.f), {100.f, 200.f});

    glm::mat3 expected = T * R * S;
    glm::mat3 actual = flatland::GetLayerLocalMatrix(rect, orientation);

    for (int col = 0; col < 3; ++col) {
      for (int row = 0; row < 3; ++row) {
        EXPECT_NEAR(expected[col][row], actual[col][row], 1e-6f)
            << "Mismatch at col " << col << ", row " << row << " for orientation "
            << static_cast<int>(orientation);
      }
    }
  }
}

// Exercises difference between how Flatland1 and Flatland2 APIs treat the interaction between
// REPLACE blend mode and opacity.
TEST(GlobalRenderListTest, FlatlandVersionGatesImageReplace) {
  const TransformHandle kRoot = {1, 0};
  GlobalTopologyData topology;
  topology.topology_vector = {kRoot};
  topology.parent_indices = {0};

  UberStruct::InstanceMap snapshot;
  auto uber_struct = std::make_shared<UberStruct>();
  uber_struct->local_topology = {{kRoot, 0}};
  uber_struct->flatland_version = 1;

  const LayerHandle kLayer(1, 1);
  uber_struct->layer_stacks[kRoot] = {kLayer};

  UberStructLayer uber_layer{
      .content =
          UberStructLayer::ImageModeProperties{
              .sample_rect = RectangleF({.x = 0.f, .y = 0.f, .width = 100.f, .height = 200.f}),
              .transform = RotateFlip::kIdentity(),
              .image_id = display::ImageId(42),
          },
      .common =
          {
              .display_rect = Rectangle({.x = 0, .y = 0, .width = 100, .height = 200}),
              .opacity = 0.5f,
              .blend_mode = BlendMode::kReplace(),
          },
  };
  uber_struct->layers[kLayer] = uber_layer;
  snapshot[1] = uber_struct;

  // Case 1: flatland_version == 1, kReplace + opacity 0.5 -> color is scaled, but blend_mode
  // remains kReplace
  {
    auto result =
        ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f)}, {kUnclippedRegion});
    ASSERT_EQ(result.size(), 1u);
    EXPECT_EQ(result[0].blend_mode, BlendMode::kReplace());
    EXPECT_EQ(result[0].multiply_color, (std::array<float, 4>{0.5f, 0.5f, 0.5f, 0.5f}));
  }

  // Case 2: flatland_version == 2, kReplace + opacity 0.5 -> blend_mode demoted to
  // kPremultipliedAlpha
  {
    uber_struct->flatland_version = 2;
    auto result =
        ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f)}, {kUnclippedRegion});
    ASSERT_EQ(result.size(), 1u);
    EXPECT_EQ(result[0].blend_mode, BlendMode::kPremultipliedAlpha());
    EXPECT_EQ(result[0].multiply_color, (std::array<float, 4>{0.5f, 0.5f, 0.5f, 0.5f}));
  }

  // Case 3: flatland_version == 2, kReplace + opacity 1.0 -> no demotion, color verbatim
  {
    uber_struct->layers[kLayer].common.opacity = 1.0f;
    auto result =
        ComputeGlobalResolvedLayers(topology, snapshot, {glm::mat3(1.f)}, {kUnclippedRegion});
    ASSERT_EQ(result.size(), 1u);
    EXPECT_EQ(result[0].blend_mode, BlendMode::kReplace());
    EXPECT_EQ(result[0].multiply_color, (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
  }
}

}  // namespace
}  // namespace flatland::test
