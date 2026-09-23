// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../codec_runner_app.h"
#include "codec_adapter_lc3_decoder.h"
#include "codec_adapter_lc3_encoder.h"

int main(int argc, char* argv[]) {
  ZX_DEBUG_ASSERT(argc == 1);

  CodecRunnerApp<CodecAdapterLc3Decoder, CodecAdapterLc3Encoder>().Run();

  return 0;
}
