// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/codec/factory/codec_specifier.h"

#include <cinttypes>
#include <random>

#include <fbl/string_printf.h>

[[nodiscard]] std::string CreateRandomCodecSpecifier() {
  static std::random_device random_device_;
  static std::mt19937 prng_{random_device_()};
  static std::uniform_int_distribution<uint64_t> uniform_distribution_;
  std::string result;
  for (uint32_t i = 0; i < 2; ++i) {
    result += fbl::StringPrintf("%08" PRIx64, uniform_distribution_(prng_));
  }
  return result;
}
