// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/source_location.h>
#include <lib/syslog/cpp/macros.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/ui/a11y/lib/gesture_manager/tests/mocks/mock_gesture_handler.h"
#include "src/ui/a11y/lib/magnifier/magnifier_2.h"

namespace accessibility_test {
namespace {

using GestureType = a11y::GestureHandlerV2::GestureType;
using testing::ElementsAre;

/// Returns a string describing the provided source location.
static std::string ToString(const cpp20::source_location& location) {
  std::string line = std::to_string(location.line());
  return "line " + line;
}

class MockMagnifierDelegate : public a11y::Magnifier2::Delegate {
 public:
  MockMagnifierDelegate() = default;
  ~MockMagnifierDelegate() override = default;

  void SetMagnificationTransform(float scale, float x, float y,
                                 SetMagnificationTransformCallback callback) override {
    scale_ = scale;
    x_ = x;
    y_ = y;
    callback();
  }

  float scale() { return scale_; }
  float x() { return x_; }
  float y() { return y_; }

 private:
  // Current state of the transform.
  float scale_ = 1.f;
  float x_ = 0.f;
  float y_ = 0.f;
};

class Magnifier2Test : public ::testing::Test {
 public:
  Magnifier2Test() = default;
  ~Magnifier2Test() override = default;

  a11y::Magnifier2* magnifier() { return magnifier_.get(); }

  MockMagnifierDelegate* mock_magnifier_delegate() { return mock_magnifier_delegate_; }

  MockGestureHandlerV2* mock_gesture_handler() { return mock_gesture_handler_.get(); }

  void SetUp() override {
    mock_gesture_handler_ = std::make_unique<MockGestureHandlerV2>();
    auto mock_magnifier_delegate = std::make_unique<MockMagnifierDelegate>();
    mock_magnifier_delegate_ = mock_magnifier_delegate.get();
    magnifier_ = std::make_unique<a11y::Magnifier2>(std::move(mock_magnifier_delegate));
    magnifier_->BindGestures(mock_gesture_handler_.get());
  }

  void ExpectThatTransformIs(
      float x, float y, float scale,
      const cpp20::source_location caller = cpp20::source_location::current()) {
    auto true_x = mock_magnifier_delegate_->x();
    auto true_y = mock_magnifier_delegate_->y();
    auto true_scale = mock_magnifier_delegate_->scale();
    FX_LOGS(INFO) << "true_x: " << true_x << " true_y: " << true_y << " true_scale: " << true_scale;
    FX_LOGS(INFO) << "expected_x: " << x << " expected_y: " << y << " expected_scale: " << scale;
    FX_LOGS(INFO) << "Difference: " << std::abs(true_x - x) << " " << std::abs(true_y - y) << " "
                  << std::abs(true_scale - scale) << " epsilon "
                  << scale * std::numeric_limits<float>::epsilon();
    EXPECT_FLOAT_EQ(mock_magnifier_delegate_->x(), x) << " from " << ToString(caller);
    EXPECT_FLOAT_EQ(mock_magnifier_delegate_->y(), y) << " from " << ToString(caller);
    EXPECT_FLOAT_EQ(mock_magnifier_delegate_->scale(), scale) << " from " << ToString(caller);
  }

 private:
  std::unique_ptr<MockGestureHandlerV2> mock_gesture_handler_;
  std::unique_ptr<a11y::Magnifier2> magnifier_;

  // Owned by magnifier_.
  MockMagnifierDelegate* mock_magnifier_delegate_;
};

TEST_F(Magnifier2Test, RegisterHandler) {
  EXPECT_EQ(mock_magnifier_delegate()->x(), 0.f);
  EXPECT_EQ(mock_magnifier_delegate()->y(), 0.f);
  EXPECT_EQ(mock_magnifier_delegate()->scale(), 1.f);
}

TEST_F(Magnifier2Test, GestureHandlersAreRegisteredIntheRightOrder) {
  // The order in which magnifier gestures are registered is relevant.
  EXPECT_THAT(mock_gesture_handler()->bound_gestures(),
              ElementsAre(GestureType::kOneFingerTripleTap, GestureType::kThreeFingerDoubleTap,
                          GestureType::kOneFingerTripleTapDrag,
                          GestureType::kThreeFingerDoubleTapDrag, GestureType::kTwoFingerDrag));
}

TEST_F(Magnifier2Test, OneFingerTripleTapTogglesMagnification) {
  a11y::gesture_util_v2::GestureContext gesture_context;
  gesture_context.current_pointer_locations[1].ndc_point.x = 0.4f;
  gesture_context.current_pointer_locations[1].ndc_point.y = 0.5f;
  mock_gesture_handler()->TriggerGesture(GestureType::kOneFingerTripleTap, gesture_context);
  ExpectThatTransformIs(-.4f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                        -.5f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                        a11y::Magnifier2::kDefaultScale);

  mock_gesture_handler()->TriggerGesture(GestureType::kOneFingerTripleTap);
  ExpectThatTransformIs(0 /* x */, 0 /* y */, 1 /* scale */);
}

TEST_F(Magnifier2Test, ThreeFingerDoubleTapTogglesMagnification) {
  a11y::gesture_util_v2::GestureContext gesture_context;
  gesture_context.current_pointer_locations[1].ndc_point.x = 0.3f;
  gesture_context.current_pointer_locations[1].ndc_point.y = 0.4f;
  gesture_context.current_pointer_locations[2].ndc_point.x = 0.4f;
  gesture_context.current_pointer_locations[2].ndc_point.y = 0.5f;
  gesture_context.current_pointer_locations[3].ndc_point.x = 0.5f;
  gesture_context.current_pointer_locations[3].ndc_point.y = 0.6f;
  mock_gesture_handler()->TriggerGesture(GestureType::kThreeFingerDoubleTap, gesture_context);
  ExpectThatTransformIs(-.4f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                        -.5f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                        a11y::Magnifier2::kDefaultScale);

  mock_gesture_handler()->TriggerGesture(GestureType::kThreeFingerDoubleTap);
  ExpectThatTransformIs(0 /* x */, 0 /* y */, 1 /* scale */);
}

TEST_F(Magnifier2Test, ThreeFingerDoubleTapDragTogglesTemporaryMagnification) {
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.3f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.4f;
    gesture_context.current_pointer_locations[2].ndc_point.x = 0.4f;
    gesture_context.current_pointer_locations[2].ndc_point.y = 0.5f;
    gesture_context.current_pointer_locations[3].ndc_point.x = 0.5f;
    gesture_context.current_pointer_locations[3].ndc_point.y = 0.6f;
    mock_gesture_handler()->TriggerGestureRecognize(GestureType::kThreeFingerDoubleTapDrag,
                                                    gesture_context);

    ExpectThatTransformIs(-.4f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                          -.5f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                          a11y::Magnifier2::kDefaultScale);
  }

  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.1f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.2f;
    gesture_context.current_pointer_locations[2].ndc_point.x = 0.2f;
    gesture_context.current_pointer_locations[2].ndc_point.y = 0.3f;
    gesture_context.current_pointer_locations[3].ndc_point.x = 0.3f;
    gesture_context.current_pointer_locations[3].ndc_point.y = 0.4f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kThreeFingerDoubleTapDrag,
                                                 gesture_context);

    ExpectThatTransformIs(-.2f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                          -.3f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                          a11y::Magnifier2::kDefaultScale);
  }

  mock_gesture_handler()->TriggerGestureComplete(GestureType::kThreeFingerDoubleTapDrag);

  ExpectThatTransformIs(0 /* x */, 0 /* y */, 1 /* scale */);
}

TEST_F(Magnifier2Test, OneFingerTripleTapDragTogglesTemporaryMagnification) {
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.3f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.4f;
    mock_gesture_handler()->TriggerGestureRecognize(GestureType::kOneFingerTripleTapDrag,
                                                    gesture_context);

    ExpectThatTransformIs(-.3f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                          -.4f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                          a11y::Magnifier2::kDefaultScale);
  }

  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.1f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.2f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kOneFingerTripleTapDrag,
                                                 gesture_context);

    ExpectThatTransformIs(-.1f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                          -.2f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                          a11y::Magnifier2::kDefaultScale);
  }

  mock_gesture_handler()->TriggerGestureComplete(GestureType::kOneFingerTripleTapDrag);

  ExpectThatTransformIs(0 /* x */, 0 /* y */, 1 /* scale */);
}

TEST_F(Magnifier2Test, TwoFingerDrag) {
  // One-finger-triple-tap to enter persistent magnification mode.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.4f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.5f;
    mock_gesture_handler()->TriggerGesture(GestureType::kOneFingerTripleTap, gesture_context);

    ExpectThatTransformIs(-.4f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                          -.5f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                          a11y::Magnifier2::kDefaultScale);
  }

  // Begin two-finger drag at a point different from the current magnification
  // focus to ensure that the transform does not change. Note that the fingers
  // are placed far enough apart to exceed kZoomMinFingerDistance.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.22f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.22f;
    gesture_context.current_pointer_locations[2].ndc_point.x = -0.22f;
    gesture_context.current_pointer_locations[2].ndc_point.y = -0.22f;
    mock_gesture_handler()->TriggerGestureRecognize(GestureType::kTwoFingerDrag, gesture_context);

    ExpectThatTransformIs(-.4f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                          -.5f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                          a11y::Magnifier2::kDefaultScale);
  }

  // Scale.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    // Double average distance between the fingers and the centroid,
    // while keeping the centroid fixed.
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.44f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.44f;
    gesture_context.current_pointer_locations[2].ndc_point.x = -0.44f;
    gesture_context.current_pointer_locations[2].ndc_point.y = -0.44f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kTwoFingerDrag, gesture_context);

    // The average distance between the fingers and the centroid doubled, so the
    // scale should double. The translation should be scaled accordingly.
    ExpectThatTransformIs(-2.4f,  // x translation from above, simplified, and scaled 2x
                          -3.0f,  // y translation from above, simplified, and scaled 2x
                          a11y::Magnifier2::kDefaultScale * 2);
  }

  // Pan horizontally.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    // Translate the centroid from (0, 0) to (-.1, 0).
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.34f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.44f;
    gesture_context.current_pointer_locations[2].ndc_point.x = -0.54f;
    gesture_context.current_pointer_locations[2].ndc_point.y = -0.44f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kTwoFingerDrag, gesture_context);

    ExpectThatTransformIs(-2.5f,  // x translation from above, plus offset
                          -3.0f,  // y translation from above
                          8.0f    // scale from above, simplified
    );
  }

  // Pan vertically.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    // Translate the centroid from (-.1, 0) to (-.1, -.1).
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.34f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.34f;
    gesture_context.current_pointer_locations[2].ndc_point.x = -0.54f;
    gesture_context.current_pointer_locations[2].ndc_point.y = -0.54f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kTwoFingerDrag, gesture_context);

    ExpectThatTransformIs(-2.5f,  // x translation from above
                          -3.1f,  // y translation from above, plus offset
                          8.0f    // scale from above, simplified
    );
  }

  // Intentional simultaneous pan and zoom.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    // Translate the centroid from (-.1, -.1) to (.1, .1), while zooming
    // out 2x.
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.32f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.32f;
    gesture_context.current_pointer_locations[2].ndc_point.x = -0.12f;
    gesture_context.current_pointer_locations[2].ndc_point.y = -0.12f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kTwoFingerDrag, gesture_context);

    // Consider the pixel directly underneath the midpoint ("centroid") of the
    // user's two fingers. As the centroid of the fingers moves, we want to
    // maintain that the same pixel remains under the centroid.
    //
    // To do so, we
    // 1. Start with the location of the previous centroid in the current
    //    coordinate system.
    // 2. Reverse the current transformation, to determine the location of
    //    the same pixel in the original image.
    // 3. Find a transform that will scale and translate the same pixel in
    //    the original image to the new centroid. Note that the scale is
    //    determined by the distance traveled by the fingers. We just solve
    //    for translation.
    //
    // Numerically:
    //  1) Previous centroid was a (-.1, -.1)
    // 2a) Previous transformation was |p' = 8p - (2.5, 3.1)|
    // 2b) Reverse transform is |p = (p' + (2.5, 3.1)) / 8|
    // 2c) Combing 1) and 2b), the centroid (-.1, -.1) represents original
    //     image pixel |(2.4, 3.0) / 8|, which simplifies to  (0.3, 0.375)
    // 3a) New transform is |p' = 4p + (dx, dy)|. (The distance between fingers decreased
    //     by half, so the new scale is half the old scale.)
    // 3b) New transform must map (0.3, 0.375) to (.1, .1)
    // 3c) Combining 3a) and 3b),
    //        |(.1, .1) = 4 * (0.3, 0.375) + (dx, dy)|
    //     => |(.1, .1) = (1.2, 1.5) + (dx, dy)|
    //     => |(-1.1, -1.4) = (dx, dy)|
    ExpectThatTransformIs(-1.1f,  // x translation computed in 3c
                          -1.4f,  // y translation computed in 3c
                          4.0f    // previous scale, zoomed out 2x
    );
  }

  // Accidental simultaneous pan and zoom. Only the pan should be applied.
  {
    // End the previous drag, so that we can start a new drag with a
    // chosen inter-finger distance.
    a11y::gesture_util_v2::GestureContext gesture_context;
    mock_gesture_handler()->TriggerGestureComplete(GestureType::kTwoFingerDrag, gesture_context);
    ExpectThatTransformIs(-1.1f, -1.4f, 4.0f);  // previous transform

    // Place two fingers on the screen, less than |kZoomMinFingerDistance| apart.
    // Placing the fingers shouldn't adjust the transform.
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.0f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.0f;
    gesture_context.current_pointer_locations[2].ndc_point.x = 0.2f;
    gesture_context.current_pointer_locations[2].ndc_point.y = 0.2f;
    mock_gesture_handler()->TriggerGestureRecognize(GestureType::kTwoFingerDrag, gesture_context);
    ExpectThatTransformIs(-1.1f, -1.4f, 4.0f);  // previous transform

    // Move the fingers apart while also translating the centroid.
    // Because the initial distance was small, this should just adjust
    // the translation, and not the scale.
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.0f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.0f;
    gesture_context.current_pointer_locations[2].ndc_point.x = 0.6f;
    gesture_context.current_pointer_locations[2].ndc_point.y = 0.6f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kTwoFingerDrag, gesture_context);
    ExpectThatTransformIs(
        -1.1f + 0.2f,  // previous x translation, translated by (0.6-0.0)/2 - (0.2-0.0)/2
        -1.4f + 0.2f,  // previous y translation, translated by (0.6-0.0)/2 - (0.2-0.0)/2
        4.0f           // previous scale
    );
  }
}

TEST_F(Magnifier2Test, ZoomOutIfMagnified) {
  // Magnify to some non-trivial transform state.
  a11y::gesture_util_v2::GestureContext gesture_context;
  gesture_context.current_pointer_locations[1].ndc_point.x = 0.4f;
  gesture_context.current_pointer_locations[1].ndc_point.y = 0.5f;
  mock_gesture_handler()->TriggerGesture(GestureType::kOneFingerTripleTap, gesture_context);
  ExpectThatTransformIs(-.4f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                        -.5f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                        a11y::Magnifier2::kDefaultScale);

  // Call ZoomOutIfMagnified() to ensure that we return to "normal" zoom state.
  magnifier()->ZoomOutIfMagnified();
  ExpectThatTransformIs(0 /* x */, 0 /* y */, 1 /* scale */);
}

TEST_F(Magnifier2Test, ClampPan) {
  // One-finger-triple-tap to enter persistent magnification mode.
  // Focus on the top-right corner of the screen.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 1.f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 1.f;
    mock_gesture_handler()->TriggerGesture(GestureType::kOneFingerTripleTap, gesture_context);

    ExpectThatTransformIs(-1.f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                          -1.f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                          a11y::Magnifier2::kDefaultScale);
  }

  // Begin a two-finger drag.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 1.f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 1.f;
    gesture_context.current_pointer_locations[2].ndc_point.x = .9f;
    gesture_context.current_pointer_locations[2].ndc_point.y = .9f;
    mock_gesture_handler()->TriggerGestureRecognize(GestureType::kTwoFingerDrag, gesture_context);
  }

  // Drag down and to the left. Since the focus is already on the top right
  // corner, this gesture should have no effect on the transform.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.1f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.1f;
    gesture_context.current_pointer_locations[2].ndc_point.x = 0.0f;
    gesture_context.current_pointer_locations[2].ndc_point.y = 0.0f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kTwoFingerDrag, gesture_context);

    ExpectThatTransformIs(-1.f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                          -1.f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                          a11y::Magnifier2::kDefaultScale);
  }
}

TEST_F(Magnifier2Test, ClampZoom) {
  // One-finger-triple-tap to enter persistent magnification mode.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0;
    mock_gesture_handler()->TriggerGesture(GestureType::kOneFingerTripleTap, gesture_context);

    ExpectThatTransformIs(0 /* x */, 0 /* y */, a11y::Magnifier2::kDefaultScale);
  }

  // Begin a two-finger drag with fingers just far enough part to
  // exceed |kZoonMinFingerDistance| of 0.3. Said differently,
  // the value 0.22 for each coordinate is chosen so that
  // sqrt((x1-x2)^2 + (y1-y2)^2)/2 > |kZoomFingerFingerDistance|.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = .22f;
    gesture_context.current_pointer_locations[1].ndc_point.y = .22f;
    gesture_context.current_pointer_locations[2].ndc_point.x = -.22f;
    gesture_context.current_pointer_locations[2].ndc_point.y = -.22f;
    mock_gesture_handler()->TriggerGestureRecognize(GestureType::kTwoFingerDrag, gesture_context);
  }

  // Spread fingers as far apart as possible. This should increase
  // the scale, but isn't far enough to exercise clamping.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 1.f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 1.f;
    gesture_context.current_pointer_locations[2].ndc_point.x = -1.f;
    gesture_context.current_pointer_locations[2].ndc_point.y = -1.f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kTwoFingerDrag, gesture_context);

    EXPECT_GT(mock_magnifier_delegate()->scale(), a11y::Magnifier2::kDefaultScale);
    EXPECT_GT(mock_magnifier_delegate()->scale(), 18.0f);
    EXPECT_LT(mock_magnifier_delegate()->scale(), a11y::Magnifier2::kMaxScale);
  }

  // Begin a two-finger drag with fingers just far enough part to
  // exceed |kZoonMinFingerDistance| of 0.3. Said differently,
  // the value 0.22 for each coordinate is chosen so that
  // sqrt((x1-x2)^2 + (y1-y2)^2)/2 > |kZoomFingerFingerDistance|.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = .22f;
    gesture_context.current_pointer_locations[1].ndc_point.y = .22f;
    gesture_context.current_pointer_locations[2].ndc_point.x = -.22f;
    gesture_context.current_pointer_locations[2].ndc_point.y = -.22f;
    mock_gesture_handler()->TriggerGestureRecognize(GestureType::kTwoFingerDrag, gesture_context);
  }

  // Spread fingers as far apart as possible. This should increase
  // the scale and exercise clamping. Unclamped, the scale would be
  // more than 18.0 * 18.0. (See previous |EXPECT_GT|.)
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 1.f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 1.f;
    gesture_context.current_pointer_locations[2].ndc_point.x = -1.f;
    gesture_context.current_pointer_locations[2].ndc_point.y = -1.f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kTwoFingerDrag, gesture_context);

    ExpectThatTransformIs(0 /* x */, 0 /* x */, a11y::Magnifier2::kMaxScale);
  }
}

TEST_F(Magnifier2Test, TwoFingerDragOnlyWorksInPersistentMode) {
  // The magnifier should only respond to two-finger drags when in PERSISTENT
  // mode, so the magnification transform should not change during this test
  // case.
  //
  // Begin two-finger drag at a point different from the current magnification
  // focus to ensure that the transform does not change.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.2f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.3f;
    gesture_context.current_pointer_locations[2].ndc_point.x = 0.4f;
    gesture_context.current_pointer_locations[2].ndc_point.y = 0.5f;
    mock_gesture_handler()->TriggerGestureRecognize(GestureType::kTwoFingerDrag, gesture_context);

    ExpectThatTransformIs(0 /* x */, 0 /* x */, 1.f);
  }

  // Try to scale and pan, and again, verify that the transform does not change.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    // Double average distance between the fingers and the centroid, and
    // translate the centroid from (.3, .4) to (.2, .3).
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.0f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.1f;
    gesture_context.current_pointer_locations[2].ndc_point.x = 0.4f;
    gesture_context.current_pointer_locations[2].ndc_point.y = 0.5f;
    mock_gesture_handler()->TriggerGestureUpdate(GestureType::kTwoFingerDrag, gesture_context);

    ExpectThatTransformIs(0 /* x */, 0 /* x */, 1.f);
  }
}

TEST_F(Magnifier2Test, TapDragOnlyWorksInUnmagnifiedMode) {
  // The magnifier should not respond to tap-drag gestures when in PERSISTENT
  // mode, so the magnification transform should not change during this test
  // case.
  //
  // Enter PERSISTENT mode with a one-finger-triple-tap.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.4f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.5f;
    mock_gesture_handler()->TriggerGesture(GestureType::kOneFingerTripleTap, gesture_context);
    ExpectThatTransformIs(-.4f * (a11y::Magnifier2::kDefaultScale - 1),  // x translation
                          -.5f * (a11y::Magnifier2::kDefaultScale - 1),  // y translation
                          a11y::Magnifier2::kDefaultScale);
  }

  // Attempt a one-finger-triple-tap-drag at a different location. The
  // magnifier should ignore the gesture, so the transform should not change.
  {
    a11y::gesture_util_v2::GestureContext gesture_context;
    gesture_context.current_pointer_locations[1].ndc_point.x = 0.3f;
    gesture_context.current_pointer_locations[1].ndc_point.y = 0.4f;
    mock_gesture_handler()->TriggerGestureRecognize(GestureType::kOneFingerTripleTapDrag,
                                                    gesture_context);

    // Check that the translation has not changed. X and Y translations are
    // updated together, so checking one of them is sufficient.
    EXPECT_LT(std::abs(mock_magnifier_delegate()->x() - (-1.2f)),
              a11y::Magnifier2::kDefaultScale * std::numeric_limits<float>::epsilon());
  }
}

}  // namespace
}  // namespace accessibility_test
