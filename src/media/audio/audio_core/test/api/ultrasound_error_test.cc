// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/types.h>

#include "src/media/audio/audio_core/stream_usage.h"
#include "src/media/audio/audio_core/test/api/ultrasound_test_shared.h"
#include "src/media/audio/lib/clock/utils.h"

namespace media::audio::test {

class UltrasoundErrorTest : public UltrasoundTest {};

TEST_F(UltrasoundErrorTest, RendererDoesNotSupportSetPcmStreamType) {
  CreateOutput();
  auto renderer = CreateRenderer();

  std::optional<zx_status_t> renderer_error;
  renderer->fidl().set_error_handler([&renderer_error](auto status) { renderer_error = {status}; });

  // Call SetPcmStreamType. We use the current stream type here to ensure we're definitely
  // requesting a supported stream type.
  renderer->fidl()->SetPcmStreamType(kUltrasoundFormat.stream_type());

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&renderer_error] { return renderer_error.has_value(); });
  ASSERT_TRUE(renderer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *renderer_error);
  Unbind(renderer);
}

TEST_F(UltrasoundErrorTest, RendererDoesNotSupportSetUsage) {
  CreateOutput();
  auto renderer = CreateRenderer();

  std::optional<zx_status_t> renderer_error;
  renderer->fidl().set_error_handler([&renderer_error](auto status) { renderer_error = {status}; });

  renderer->fidl()->SetUsage(*ToFidlRenderUsageTry(fuchsia::media::AudioRenderUsage2::MEDIA));

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&renderer_error] { return renderer_error.has_value(); });
  ASSERT_TRUE(renderer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *renderer_error);
  Unbind(renderer);
}

TEST_F(UltrasoundErrorTest, RendererDoesNotSupportSetUsage2) {
  CreateOutput();
  auto renderer = CreateRenderer();

  std::optional<zx_status_t> renderer_error;
  renderer->fidl().set_error_handler([&renderer_error](auto status) { renderer_error = {status}; });

  renderer->fidl()->SetUsage2(fuchsia::media::AudioRenderUsage2::ACCESSIBILITY);

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&renderer_error] { return renderer_error.has_value(); });
  ASSERT_TRUE(renderer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *renderer_error);
  Unbind(renderer);
}

TEST_F(UltrasoundErrorTest, RendererDoesNotSupportBindGainControl) {
  CreateOutput();
  auto renderer = CreateRenderer();

  std::optional<zx_status_t> renderer_error;
  renderer->fidl().set_error_handler([&renderer_error](auto status) { renderer_error = {status}; });

  fuchsia::media::audio::GainControlPtr gain_control;
  renderer->fidl()->BindGainControl(gain_control.NewRequest());

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&renderer_error] { return renderer_error.has_value(); });
  ASSERT_TRUE(renderer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *renderer_error);
  Unbind(renderer);
}

TEST_F(UltrasoundErrorTest, RendererDoesNotSupportSetReferenceClock) {
  CreateOutput();
  auto renderer = CreateRenderer();

  std::optional<zx_status_t> renderer_error;
  renderer->fidl().set_error_handler([&renderer_error](auto status) { renderer_error = {status}; });
  renderer->fidl()->SetReferenceClock(clock::DuplicateClock(renderer->reference_clock()));

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&renderer_error] { return renderer_error.has_value(); });
  ASSERT_TRUE(renderer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *renderer_error);
  Unbind(renderer);
}

TEST_F(UltrasoundErrorTest, CapturerDoesNotSupportSetPcmStreamType) {
  CreateInput();
  auto capturer = CreateCapturer();

  std::optional<zx_status_t> capturer_error;
  capturer->fidl().set_error_handler([&capturer_error](auto status) { capturer_error = {status}; });

  // Call SetPcmStreamType. We use the current stream type here to ensure we're definitely
  // requesting a supported stream type.
  capturer->fidl()->SetPcmStreamType(kUltrasoundFormat.stream_type());

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&capturer_error] { return capturer_error.has_value(); });
  ASSERT_TRUE(capturer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *capturer_error);
  Unbind(capturer);
}

TEST_F(UltrasoundErrorTest, CapturerDoesNotSupportSetUsage) {
  CreateInput();
  auto capturer = CreateCapturer();

  std::optional<zx_status_t> capturer_error;
  capturer->fidl().set_error_handler([&capturer_error](auto status) { capturer_error = {status}; });

  capturer->fidl()->SetUsage(fuchsia::media::AudioCaptureUsage::SYSTEM_AGENT);

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&capturer_error] { return capturer_error.has_value(); });
  ASSERT_TRUE(capturer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *capturer_error);
  Unbind(capturer);
}

TEST_F(UltrasoundErrorTest, CapturerDoesNotSupportSetUsage2) {
  CreateInput();
  auto capturer = CreateCapturer();

  std::optional<zx_status_t> capturer_error;
  capturer->fidl().set_error_handler([&capturer_error](auto status) { capturer_error = {status}; });

  capturer->fidl()->SetUsage2(fuchsia::media::AudioCaptureUsage2::SYSTEM_AGENT);

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&capturer_error] { return capturer_error.has_value(); });
  ASSERT_TRUE(capturer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *capturer_error);
  Unbind(capturer);
}

TEST_F(UltrasoundErrorTest, CapturerDoesNotSupportBindGainControl) {
  CreateInput();
  auto capturer = CreateCapturer();

  std::optional<zx_status_t> capturer_error;
  capturer->fidl().set_error_handler([&capturer_error](auto status) { capturer_error = {status}; });

  fuchsia::media::audio::GainControlPtr gain_control;
  capturer->fidl()->BindGainControl(gain_control.NewRequest());

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&capturer_error] { return capturer_error.has_value(); });
  ASSERT_TRUE(capturer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *capturer_error);
  Unbind(capturer);
}

TEST_F(UltrasoundErrorTest, CapturerDoesNotSupportSetReferenceClock) {
  CreateInput();
  auto capturer = CreateCapturer();

  std::optional<zx_status_t> capturer_error;
  capturer->fidl().set_error_handler([&capturer_error](auto status) { capturer_error = {status}; });
  capturer->fidl()->SetReferenceClock(clock::DuplicateClock(capturer->reference_clock()));

  // Now expect we get disconnected with ZX_ERR_NOT_SUPPORTED.
  RunLoopUntil([&capturer_error] { return capturer_error.has_value(); });
  ASSERT_TRUE(capturer_error);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, *capturer_error);
  Unbind(capturer);
}

}  // namespace media::audio::test
