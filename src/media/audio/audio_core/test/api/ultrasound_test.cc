// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/types.h>

#include "src/media/audio/audio_core/test/api/ultrasound_test_shared.h"
#include "src/media/audio/lib/clock/testing/clock_test.h"

namespace media::audio::test {

TEST_F(UltrasoundTest, CreateRenderer) {
  CreateOutput();
  auto renderer = CreateRenderer();

  clock::testing::VerifyReadOnlyRights(renderer->reference_clock());
  clock::testing::VerifyAdvances(renderer->reference_clock());
  clock::testing::VerifyCannotBeRateAdjusted(renderer->reference_clock());
  clock::testing::VerifyIsSystemMonotonic(renderer->reference_clock());
  Unbind(renderer);
}

TEST_F(UltrasoundTest, CreateRendererWithoutOutputDevice) {
  // Create a renderer but do not wait for it to fully initialize because there is no device for it
  // to link to yet.
  auto renderer = CreateUltrasoundRenderer(kUltrasoundFormat, kBufferSize,
                                           /* wait_for_creation */ false);

  // Now create an input and capturer. This is just to synchronize with audio_core to verify that
  // the above |CreateRenderer| has been processed. We're relying here on the fact that audio_core
  // will form links synchronously on the FIDL thread as part of the CreateRenderer operation, so
  // if we've linked our Capturer then we know we have not linked our renderer.
  CreateInput();
  CreateCapturer();
  EXPECT_FALSE(renderer->created());

  // Now add the output, which will allow the renderer to be linked.
  CreateOutput();
  renderer->WaitForDevice();
  EXPECT_TRUE(renderer->created());
  Unbind(renderer);
}

TEST_F(UltrasoundTest, CreateCapturer) {
  CreateInput();
  auto capturer = CreateCapturer();

  clock::testing::VerifyReadOnlyRights(capturer->reference_clock());
  clock::testing::VerifyAdvances(capturer->reference_clock());
  clock::testing::VerifyCannotBeRateAdjusted(capturer->reference_clock());
  clock::testing::VerifyIsSystemMonotonic(capturer->reference_clock());
  Unbind(capturer);
}

TEST_F(UltrasoundTest, CreateCapturerWithoutInputDevice) {
  // Create a capturer but do not wait for it to fully initialize because there is no device for it
  // to link to yet.
  auto capturer = CreateUltrasoundCapturer(kUltrasoundFormat, kBufferSize,
                                           /* wait_for_creation */ false);

  // Now create an output and renderer. This is just to synchronize with audio_core to verify that
  // the above |CreateCapturer| has been processed. We're relying here on the fact that audio_core
  // will form links synchronously on the FIDL thread as part of the CreateCapturer operation, so
  // if we've linked our renderer then we know we have not linked our capturer.
  CreateOutput();
  CreateRenderer();
  EXPECT_FALSE(capturer->created());

  // Now add the input, which will allow the capturer to be linked.
  CreateInput();
  capturer->WaitForDevice();
  EXPECT_TRUE(capturer->created());
  Unbind(capturer);
}

}  // namespace media::audio::test
