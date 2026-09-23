// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_LIBFUZZER_STATS_H_
#define SRC_SYS_FUZZING_LIBFUZZER_STATS_H_

#include <fuchsia/fuzzer/cpp/fidl.h>

#include <cstdint>
#include <string>
#include <string_view>

namespace fuzzing {

using fuchsia::fuzzer::Status;
using fuchsia::fuzzer::UpdateReason;

constexpr zx_duration_t kOneSecond = ZX_SEC(1);
constexpr size_t kOneKb = 1ULL << 10;
constexpr size_t kOneMb = 1ULL << 20;

bool ParseLibFuzzerStats(std::string_view line, UpdateReason* reason, Status* status);

std::string FormatLibFuzzerStats(UpdateReason reason, const Status& status);

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_LIBFUZZER_STATS_H_
