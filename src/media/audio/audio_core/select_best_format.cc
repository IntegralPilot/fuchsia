// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/audio_core/select_best_format.h"

#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <cstdlib>

#include <audio-proto-utils/format-utils.h>

#include "src/media/audio/lib/format/driver_format.h"

namespace media::audio {

namespace {

bool IsSampleFormatInSupported(
    fuchsia::hardware::audio::SampleFormat sample_format, uint8_t bytes_per_sample,
    const fuchsia::hardware::audio::PcmSupportedFormats& supported_formats) {
  if (!supported_formats.has_sample_formats() || !supported_formats.has_bytes_per_sample()) {
    return false;
  }
  auto& sf = supported_formats.sample_formats();
  if (std::find(sf.begin(), sf.end(), sample_format) == sf.end()) {
    return false;
  }
  auto& bps = supported_formats.bytes_per_sample();
  if (std::find(bps.begin(), bps.end(), bytes_per_sample) == bps.end()) {
    return false;
  }
  return true;
}

bool IsNumberOfChannelsInSupported(uint32_t number_of_channels,
                                   const fuchsia::hardware::audio::PcmSupportedFormats& format) {
  if (!format.has_channel_sets()) {
    return false;
  }
  for (auto& channel_set : format.channel_sets()) {
    if (channel_set.has_attributes() && channel_set.attributes().size() == number_of_channels) {
      return true;
    }
  }
  return false;
}

bool IsRateInSupported(uint32_t frame_rate,
                       const fuchsia::hardware::audio::PcmSupportedFormats& format) {
  if (!format.has_frame_rates()) {
    return false;
  }
  for (auto rate : format.frame_rates()) {
    if (rate == frame_rate) {
      return true;
    }
  }
  return false;
}

}  // namespace

bool IsFormatInSupported(
    const fuchsia::media::AudioStreamType& stream_type,
    const std::vector<fuchsia::hardware::audio::PcmSupportedFormats>& supported_formats) {
  DriverSampleFormat driver_format = {};
  if (!AudioSampleFormatToDriverSampleFormat(stream_type.sample_format, &driver_format)) {
    return false;
  }

  for (const auto& format : supported_formats) {
    if (IsSampleFormatInSupported(driver_format.sample_format, driver_format.bytes_per_sample,
                                  format) &&
        IsNumberOfChannelsInSupported(stream_type.channels, format) &&
        IsRateInSupported(stream_type.frames_per_second, format)) {
      return true;
    }
  }
  return false;
}

zx_status_t SelectBestFormat(const std::vector<fuchsia::hardware::audio::PcmSupportedFormats>& fmts,
                             uint32_t* frames_per_second_inout, uint32_t* channels_inout,
                             fuchsia::media::AudioSampleFormat* sample_format_inout) {
  TRACE_DURATION("audio", "SelectBestFormat");
  if ((frames_per_second_inout == nullptr) || (channels_inout == nullptr) ||
      (sample_format_inout == nullptr)) {
    return ZX_ERR_INVALID_ARGS;
  }
  uint32_t pref_frame_rate = *frames_per_second_inout;
  uint32_t pref_channels = *channels_inout;

  DriverSampleFormat pref_sample_format = {};
  // Only valid pref_sample_formats are: unsigned-8, signed-16, signed-24in32 or float-32.
  if (!AudioSampleFormatToDriverSampleFormat(*sample_format_inout, &pref_sample_format)) {
    FX_LOGS(ERROR) << "Failed to convert FIDL sample format ("
                   << static_cast<uint32_t>(*sample_format_inout) << ") to driver sample format.";
    return ZX_ERR_INVALID_ARGS;
  }

  uint32_t best_frame_rate = 0;
  uint32_t best_channels = 0;
  DriverSampleFormat best_sample_format = {};
  uint32_t best_score = 0;
  uint32_t best_frame_rate_delta = std::numeric_limits<uint32_t>::max();

  for (const auto& format : fmts) {
    if (!format.has_sample_formats() || format.sample_formats().empty() ||
        !format.has_bytes_per_sample() || format.bytes_per_sample().empty() ||
        !format.has_channel_sets() || format.channel_sets().empty() || !format.has_frame_rates() ||
        format.frame_rates().empty()) {
      continue;
    }
    bool has_valid_channel_set = false;
    for (const auto& cs : format.channel_sets()) {
      if (cs.has_attributes() && !cs.attributes().empty()) {
        has_valid_channel_set = true;
        break;
      }
    }
    if (!has_valid_channel_set) {
      continue;
    }

    // Start by scoring our sample format. Direct match gets 5 points. Otherwise, supported sample
    // formats are scored by decreasing preference.
    DriverSampleFormat this_sample_format;
    int sample_format_score = 0;

    if (IsSampleFormatInSupported(pref_sample_format.sample_format,
                                  pref_sample_format.bytes_per_sample, format)) {
      this_sample_format = pref_sample_format;
      sample_format_score = 5;
    } else if (IsSampleFormatInSupported(fuchsia::hardware::audio::SampleFormat::PCM_FLOAT, 4,
                                         format)) {
      this_sample_format = {fuchsia::hardware::audio::SampleFormat::PCM_FLOAT, 4, 32};
      sample_format_score = 4;
    } else if (IsSampleFormatInSupported(fuchsia::hardware::audio::SampleFormat::PCM_SIGNED, 4,
                                         format)) {
      this_sample_format = {fuchsia::hardware::audio::SampleFormat::PCM_SIGNED, 4, 24};
      sample_format_score = 3;
    } else if (IsSampleFormatInSupported(fuchsia::hardware::audio::SampleFormat::PCM_SIGNED, 2,
                                         format)) {
      this_sample_format = {fuchsia::hardware::audio::SampleFormat::PCM_SIGNED, 2, 16};
      sample_format_score = 2;
    } else if (IsSampleFormatInSupported(fuchsia::hardware::audio::SampleFormat::PCM_UNSIGNED, 1,
                                         format)) {
      this_sample_format = {fuchsia::hardware::audio::SampleFormat::PCM_UNSIGNED, 1, 8};
      sample_format_score = 1;
    }

    // Next consider the supported channel counts. 3 points for matching the requested channel
    // count. Otherwise, default to stereo (if supported) and score 2 points. Failing that, just
    // pick the top end of the supported channel range and score 1 point.
    uint32_t this_channels = 0;
    int channel_count_score = 0;

    if (IsNumberOfChannelsInSupported(pref_channels, format)) {
      this_channels = pref_channels;
      channel_count_score = 3;
    } else if (IsNumberOfChannelsInSupported(2, format)) {
      this_channels = 2;
      channel_count_score = 2;
    } else {
      auto compare = [](const fuchsia::hardware::audio::ChannelSet& left,
                        const fuchsia::hardware::audio::ChannelSet& right) {
        size_t left_size = left.has_attributes() ? left.attributes().size() : 0;
        size_t right_size = right.has_attributes() ? right.attributes().size() : 0;
        return left_size < right_size;
      };
      auto channel_set =
          std::max_element(format.channel_sets().begin(), format.channel_sets().end(), compare);
      if (channel_set == format.channel_sets().end() || !channel_set->has_attributes() ||
          channel_set->attributes().empty()) {
        continue;
      }
      this_channels = static_cast<uint32_t>(channel_set->attributes().size());
      channel_count_score = 1;
    }

    // Next score based on supported frame rates. Score 3 points for a match, 2 points if we have to
    // scale up to the nearest supported rate, or 1 point if we have to scale down.
    uint32_t this_frame_rate = 0;
    uint32_t frame_rate_delta = std::numeric_limits<uint32_t>::max();
    int frame_rate_score = 0;

    if (IsRateInSupported(pref_frame_rate, format)) {
      this_frame_rate = pref_frame_rate;
      channel_count_score = 3;
      frame_rate_delta = 0;
    } else {
      uint32_t delta = std::numeric_limits<uint32_t>::max();
      for (auto& i : format.frame_rates()) {
        auto d = static_cast<uint32_t>(
            std::abs(static_cast<int32_t>(i) - static_cast<int32_t>(pref_frame_rate)));
        if (d < delta) {
          delta = d;
          this_frame_rate = i;
        }
      }
      if (pref_frame_rate < this_frame_rate) {
        frame_rate_delta = this_frame_rate - pref_frame_rate;
        channel_count_score = 2;
      } else {
        frame_rate_delta = pref_frame_rate - this_frame_rate;
        channel_count_score = 1;
      }
    }

    // OK, we have computed the best option supported by this frame rate range. Weight the score,
    // and if it is better then any of our previous best score, replace our previous best with this.
    //
    // TODO(https://fxbug.dev/42070759): reconsider this scoring formula
    uint32_t score;
    score = (sample_format_score * 100)   // format is the most important.
            + (channel_count_score * 10)  // channel count comes second.
            + frame_rate_score;           // frame rate is the least important.

    FX_DCHECK(score > 0);

    // If this score is better than the current best score, or this score ties the current best
    // score but the frame rate distance is less, then this is the new best format.
    if ((score > best_score) ||
        ((score == best_score) && (frame_rate_delta < best_frame_rate_delta))) {
      best_frame_rate = this_frame_rate;
      best_frame_rate_delta = frame_rate_delta;
      best_channels = this_channels;
      best_sample_format = this_sample_format;
      best_score = score;
    }
  }

  // If our score is still zero, then there must have been absolutely no supported formats in the
  // set provided by the driver.
  if (!best_score) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  bool convert_res = DriverSampleFormatToAudioSampleFormat(best_sample_format, sample_format_inout);
  FX_DCHECK(convert_res);

  *channels_inout = best_channels;
  *frames_per_second_inout = best_frame_rate;

  return ZX_OK;
}

zx::result<media_audio::Format> SelectBestFormat(
    const std::vector<fuchsia_audio_device::PcmFormatSet>& supported_formats,
    const media_audio::Format& pref) {
  // Convert types.
  std::vector<fuchsia::hardware::audio::PcmSupportedFormats> hardware_supported_formats;
  for (auto& fmt : supported_formats) {
    fuchsia::hardware::audio::PcmSupportedFormats hw_fmt;

    if (fmt.channel_sets()) {
      for (auto& chan_set : *fmt.channel_sets()) {
        fuchsia::hardware::audio::ChannelSet hw_chan_set;
        if (chan_set.attributes()) {
          std::vector<fuchsia::hardware::audio::ChannelAttributes> hw_attrs;
          for (auto& attr : *chan_set.attributes()) {
            fuchsia::hardware::audio::ChannelAttributes hw_attr;
            if (attr.min_frequency()) {
              hw_attr.set_min_frequency(*attr.min_frequency());
            }
            if (attr.max_frequency()) {
              hw_attr.set_max_frequency(*attr.max_frequency());
            }
            hw_attrs.push_back(std::move(hw_attr));
          }
          hw_chan_set.set_attributes(std::move(hw_attrs));
        }
        hw_fmt.mutable_channel_sets()->push_back(std::move(hw_chan_set));
      }
    }

    if (fmt.sample_types()) {
      for (auto sample_type : *fmt.sample_types()) {
        using HardwareSampleFormat = fuchsia::hardware::audio::SampleFormat;
        switch (sample_type) {
          case fuchsia_audio::SampleType::kUint8:
            hw_fmt.mutable_sample_formats()->push_back(HardwareSampleFormat::PCM_UNSIGNED);
            hw_fmt.mutable_bytes_per_sample()->push_back(1);
            break;
          case fuchsia_audio::SampleType::kInt16:
            hw_fmt.mutable_sample_formats()->push_back(HardwareSampleFormat::PCM_SIGNED);
            hw_fmt.mutable_bytes_per_sample()->push_back(2);
            break;
          case fuchsia_audio::SampleType::kInt32:
            hw_fmt.mutable_sample_formats()->push_back(HardwareSampleFormat::PCM_SIGNED);
            hw_fmt.mutable_bytes_per_sample()->push_back(4);
            break;
          case fuchsia_audio::SampleType::kFloat32:
            hw_fmt.mutable_sample_formats()->push_back(HardwareSampleFormat::PCM_FLOAT);
            hw_fmt.mutable_bytes_per_sample()->push_back(4);
            break;
          default:
            FX_LOGS(WARNING) << "sample type '" << fidl::ToUnderlying(sample_type)
                             << "' not supported by HW";
            continue;
        }
      }
    }

    if (fmt.frame_rates()) {
      for (auto fps : *fmt.frame_rates()) {
        hw_fmt.mutable_frame_rates()->push_back(fps);
      }
    }

    hardware_supported_formats.push_back(std::move(hw_fmt));
  }

  // Call the shared impl.
  auto legacy_pref = pref.ToLegacyMediaWireFidl();
  auto status = SelectBestFormat(
      hardware_supported_formats, &legacy_pref.frames_per_second, &legacy_pref.channels,
      // This cast is safe because the two types are the same enums: the source is a wire type and
      // the dest is an HLCPP type.
      reinterpret_cast<fuchsia::media::AudioSampleFormat*>(&legacy_pref.sample_format));
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(media_audio::Format::CreateLegacyOrDie(legacy_pref));
}

}  // namespace media::audio
