// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/codec/examples/use_media_decoder/test/use_video_decoder_test.h"
#include "src/media/codec/examples/use_media_decoder/use_video_decoder.h"

namespace {
constexpr char kInputFilePath[] = "/pkg/data/bear.mjpeg";
constexpr int kInputFileFrameCount = 30;

const char* kGoldenSha256 = "32ec7179c2b2dc19897dc908317bfa1d86e126766e144b84bf92d7d795c305b0";
}  // namespace

int main(int argc, char* argv[]) {
  UseVideoDecoderTestParams test_params = {
      .mime_type = "video/x-motion-jpeg",
      .golden_sha256 = kGoldenSha256,
  };

  return use_video_decoder_test(kInputFilePath, kInputFileFrameCount, use_mjpeg_decoder,
                                /*is_secure_output=*/false, /*is_secure_input=*/false,
                                /*min_output_buffer_count=*/0, &test_params);
}
