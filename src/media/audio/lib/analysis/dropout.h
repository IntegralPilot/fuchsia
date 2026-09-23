// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_LIB_ANALYSIS_DROPOUT_H_
#define SRC_MEDIA_AUDIO_LIB_ANALYSIS_DROPOUT_H_

#include <fuchsia/media/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/types.h>

#include <cmath>
#include <iomanip>
#include <optional>
#include <string>
#include <string_view>

#include "src/lib/fxl/strings/string_printf.h"

namespace media::audio {

// Below are three utility classes that can be used to detect dropouts.

// SilentPacketChecker checks each audio packet, returning _false_ if the packet contains at least
// one non-silent sample. For performance reasons, it stops when the first non-zero sample is found.
// This class works with all supported sample formats.
class SilentPacketChecker {
 public:
  static bool IsSilenceFloat32(const void* samples, int64_t sample_count) {
    auto floats = static_cast<const float*>(samples);
    for (int64_t idx = 0; idx < sample_count; ++idx) {
      if (floats[idx] > std::numeric_limits<float>::epsilon() ||
          floats[idx] < -std::numeric_limits<float>::epsilon()) {
        return false;
      }
    }
    return true;
  }
  static bool IsSilenceInt16(const void* samples, int64_t sample_count) {
    auto ints = static_cast<const int16_t*>(samples);
    for (int64_t idx = 0; idx < sample_count; ++idx) {
      if (ints[idx]) {
        return false;
      }
    }
    return true;
  }
  static bool IsSilenceInt24In32(const void* samples, int64_t sample_count) {
    auto ints = static_cast<const int32_t*>(samples);
    for (int64_t idx = 0; idx < sample_count; ++idx) {
      if (ints[idx] & 0xFFFFFF00) {
        return false;
      }
    }
    return true;
  }
  static bool IsSilenceUint8(const void* samples, int64_t sample_count) {
    auto ints = static_cast<const uint8_t*>(samples);
    for (int64_t idx = 0; idx < sample_count; ++idx) {
      if (ints[idx] != 0x80) {
        return false;
      }
    }
    return true;
  }
};

// PowerChecker verifies that a stream contains a signal of expected power.
// It calculates an audio section's power, returning false if this does not meet the expected val.
//
// This checker includes each sample in exactly one calculation (it does not use overlapping RMS
// windows). E.g., for a 512-sample window, the first RMS check is based on samples 0-511, and the
// second check based on samples 512-1023 (modulo any intervening Reset() calls).
//
// For simplicity, this class is currently limited to FLOAT data only.
class PowerChecker {
 public:
  PowerChecker(int64_t rms_window_in_frames, int32_t channels, double expected_min_power_rms,
               const std::string_view& tag = "")
      : rms_window_in_frames_(rms_window_in_frames),
        channels_(channels),
        expected_power_rms_(expected_min_power_rms),
        tag_(tag.empty() ? tag : std::string_view(std::string(tag).append(": "))) {
    Reset();
  }

  void Reset() {
    running_window_frame_count_ = 0;
    running_sum_squares_ = 0.0;
  }

  bool Check(const float* samples, int64_t frame_position, int64_t frame_count,
             bool print = false) {
    if (frame_position_ != frame_position) {
      Reset();
    }
    frame_position_ = frame_position + frame_count;

    bool pass = true;
    // Ingest all the provided samples before leaving
    while (frame_count) {
      // Starting from any previously-retained running totals/counts, incorporate each additional
      // sample until we have enough to analyze. Stop earlier if we run out of samples.
      while (running_window_frame_count_ < rms_window_in_frames_ && frame_count) {
        for (auto chan = 0; chan < channels_; ++chan) {
          auto val = *samples;
          running_sum_squares_ += (val * val);
          samples++;
        }
        running_window_frame_count_++;
        frame_count--;
      }
      // If we have enough to analyze, do so now and reset our running totals/counts to zero.
      // Otherwise, just ingest these values and return true.
      if (running_window_frame_count_ == rms_window_in_frames_) {
        auto mean_squares =
            running_sum_squares_ / static_cast<double>(rms_window_in_frames_ * channels_);
        auto current_root_mean_squares = sqrt(mean_squares);
        if (current_root_mean_squares + std::numeric_limits<float>::epsilon() <
            expected_power_rms_) {
          // We might have been provided enough samples for multiple windows (thus more than one
          // success/fail calculation). If ANY of them fail, return false.
          pass = false;
          if (print) {
            FX_LOGS(ERROR) << tag_ << "XXX Dropout detected. Across window of "
                           << rms_window_in_frames_ << " frames, measured power " << std::fixed
                           << std::setprecision(6) << std::setw(8) << current_root_mean_squares
                           << " (expected " << std::fixed << std::setprecision(4) << std::setw(6)
                           << expected_power_rms_ << ")";
          }
        } else {
          if constexpr (kSuccessLogStride) {
            if (print) {
              if (success_log_count_ == 0) {
                FX_LOGS(INFO) << tag_ << "XXX Across window of " << rms_window_in_frames_
                              << " frames, successfully measured power " << std::fixed
                              << std::setprecision(6) << std::setw(8) << current_root_mean_squares
                              << " (expected " << std::fixed << std::setprecision(4) << std::setw(6)
                              << expected_power_rms_ << ")";
              }
              success_log_count_ = ++success_log_count_ % kSuccessLogStride;
            }
          }
        }
        Reset();
      }
    }
    return pass;
  }

 private:
  const int64_t rms_window_in_frames_;
  const int32_t channels_;  // only used for display purposes
  const double expected_power_rms_;
  const std::string_view tag_;

  int64_t frame_position_ = 0;
  int64_t running_window_frame_count_;
  double running_sum_squares_;

  // To calibrate appropriate RMS limits for specific content, display the calculated RMS power even
  // on success. A stride reduces log spam; making it PRIME (797 is appropriate) varies sampling
  // across periodic signals. To disable this logging altogether, set kSuccessLogStride to 0.
  static constexpr int64_t kSuccessLogStride = 0;
  int64_t success_log_count_ = 0;  // only used for display purposes
};

// SilenceChecker verifies that a stream does not contain a consecutive number of truly silent
// frames, with Check() returning false if this ever occurs. For simplicity, this class is currently
// limited to FLOAT data only.
class SilenceChecker {
  static constexpr bool kOnlyLogFailureOnEnd = true;

 public:
  explicit SilenceChecker(int64_t max_count_silent_frames_allowed, int32_t channels,
                          const std::string_view& tag = "")
      : max_silent_frames_allowed_(max_count_silent_frames_allowed),
        channels_(channels),
        tag_(tag.empty() ? tag : std::string_view(std::string(tag).append(": "))) {}

  inline void Reset(int64_t frame_position = 0, bool print = false) {
    if (running_silent_frame_count_) {
      if constexpr (kOnlyLogFailureOnEnd) {
        if (running_silent_frame_count_ > max_silent_frames_allowed_ && print) {
          LogFailure(running_silent_frames_start_, running_silent_frame_count_);
        }
      }

      if (print) {
        FX_LOGS(INFO) << tag_ << "Clearing running silent frames (was "
                      << running_silent_frame_count_ << ")";
      }
      running_silent_frame_count_ = 0;
    }
    frame_position_ = frame_position;
  }

  bool Check(const float* samples, int64_t frame_position, int64_t frame_count,
             bool print = false) {
    if (frame_position_ != frame_position) {
      if (print) {
        FX_LOGS(INFO) << tag_ << "Changing running frame position from " << frame_position_
                      << " to " << frame_position;
      }
      Reset(frame_position);
    }

    bool pass = true;
    int64_t max_silent_frames_detected = 0;
    int64_t max_silent_frames_start = running_silent_frames_start_;

    // Ingest all provided samples before leaving
    while (frame_count--) {
      // Starting from any previously-retained running count, incorporate additional silent frames.
      bool is_frame_silent = true;
      for (auto chan = 0; chan < channels_; ++chan) {
        if (samples[chan] > std::numeric_limits<float>::epsilon() ||
            samples[chan] < -std::numeric_limits<float>::epsilon()) {
          is_frame_silent = false;
          break;
        }
      }

      if (is_frame_silent) {
        if (!running_silent_frame_count_) {
          running_silent_frames_start_ = frame_position_;
        }
        ++running_silent_frame_count_;
        if (running_silent_frame_count_ > max_silent_frames_allowed_) {
          pass = false;
        }
        if (running_silent_frame_count_ > max_silent_frames_detected) {
          max_silent_frames_start = running_silent_frames_start_;
          max_silent_frames_detected = running_silent_frame_count_;
        }
      } else {
        Reset(frame_position_, print);
      }
      samples += channels_;
      frame_position_++;
    }
    if constexpr (!kOnlyLogFailureOnEnd) {
      if (max_silent_frames_detected > max_silent_frames_allowed_ && print) {
        LogFailure(max_silent_frames_start, max_silent_frames_detected);
      }
    }
    return pass;
  }

  void LogFailure(int64_t max_silent_frames_start, int64_t max_silent_frames_detected) {
    FX_LOGS(ERROR) << tag_ << "XXX Silence detected -- measured " << max_silent_frames_detected
                   << " consecutive silent frames (max allowed: " << max_silent_frames_allowed_
                   << ") starting at " << max_silent_frames_start;
  }

 private:
  const int64_t max_silent_frames_allowed_;
  const int32_t channels_;
  const std::string_view tag_;

  int64_t frame_position_ = 0;
  int64_t running_silent_frame_count_ = 0;
  int64_t running_silent_frames_start_;
};

// This rudimentary checker looks for overlaps/gaps in a sequence of PTS ranges (start and length).
//
// When given NO_TIMESTAMP, it subsequently will not trigger (i.e. it errs toward false negative).
// It also does not reason about continuity thresholds, nor whether a packet is submitted after the
// PTS. These aspects may be addressed in subsequent CL or in a different class altogether.
class BasicTimestampChecker {
 public:
  explicit BasicTimestampChecker(std::optional<int64_t> pts_start = std::nullopt,
                                 const std::string_view& tag = "")
      : last_pts_end_(pts_start),
        tag_(tag.empty() ? tag : std::string_view(std::string(tag).append(": "))) {
    Reset();
  }

  inline void Reset(std::optional<int64_t> pts_start = std::nullopt) { last_pts_end_ = pts_start; }

  bool Check(int64_t pts_start, int64_t pts_len, bool print = false) {
    if (pts_start == fuchsia::media::NO_TIMESTAMP) {
      last_pts_end_ = fuchsia::media::NO_TIMESTAMP;
      return true;
    }
    if (!last_pts_end_.has_value()) {
      last_pts_end_ = pts_start + pts_len;
      return true;
    }
    if (*last_pts_end_ == fuchsia::media::NO_TIMESTAMP) {
      return true;
    }

    int64_t pts_delta = *last_pts_end_ - pts_start;
    if (print) {
      if (pts_delta > 0) {
        FX_LOGS(ERROR) << tag_ << "XXX Packet overlap of " << separated_pts(pts_delta)
                       << " detected -- prev pts_end " << separated_pts(*last_pts_end_)
                       << ", this pts_start " << separated_pts(pts_start);
      } else if (pts_delta < 0) {
        FX_LOGS(ERROR) << tag_ << "XXX Gap of " << separated_pts(-pts_delta)
                       << " detected between packets -- prev pts_end "
                       << separated_pts(*last_pts_end_) << ", this pts_start "
                       << separated_pts(pts_start);
      }
    }
    last_pts_end_ = pts_start + pts_len;

    return (pts_delta == 0);
  }

 private:
  friend class DropoutBasicTimestampChecker;

  static std::string separated_pts(int64_t pts) {
    bool was_negative = (pts < 0);
    if (was_negative) {
      pts = -pts;
    }

    std::string pts_str = fxl::StringPrintf("%03zd", pts % 1000);
    pts /= 1000;
    while (pts > 0) {
      pts_str = fxl::StringPrintf("%03zd", pts % 1000).append("'").append(pts_str);
      pts /= 1000;
    }

    if (was_negative) {
      pts_str = "-" + pts_str;
    }
    return pts_str;
  }

  std::optional<int64_t> last_pts_end_ = std::nullopt;
  const std::string_view tag_;
};

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_LIB_ANALYSIS_DROPOUT_H_
