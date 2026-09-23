// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_LIB_PROCESSING_FLAGS_H_
#define SRC_MEDIA_AUDIO_LIB_PROCESSING_FLAGS_H_

namespace media_audio {

// Debug computation of output values (`ComputeSample`), from coefficients and input values.
// Extremely verbose, only useful in a controlled unittest setting.
inline constexpr bool kTraceFilterComputation = false;

// Enable to emit trace events containing the position state.
inline constexpr bool kTracePositionEvents = false;

// Perform consistency checks on rates and internal positions, and CHECK if we detect a problem.
// When disabled, we not only avoid doing these checks, we also eliminate this code altogether.
#if (NDEBUG)
inline constexpr bool kCheckPositionsAndRatesOrDie = false;
#else   // (NDEBUG)
inline constexpr bool kCheckPositionsAndRatesOrDie = true;
#endif  // (NDEBUG)

// Include verbose display of filter tables and computation. When disabled, we not only avoid doing
// this display, we also eliminate this code altogether.
inline constexpr bool kEnableDisplayForPositionsAndRates = false;

// Include verbose display of filter tables and computation. When disabled, we not only avoid doing
// this display, we also eliminate this code altogether.
inline constexpr bool kEnableDisplayForFilterTablesAndComputation = false;

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_LIB_PROCESSING_FLAGS_H_
