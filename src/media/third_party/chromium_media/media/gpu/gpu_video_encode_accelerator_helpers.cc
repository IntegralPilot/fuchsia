// Copyright 2018 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "media/gpu/gpu_video_encode_accelerator_helpers.h"

#include <algorithm>
#include <array>
#include <ostream>

#include "media/base/bitrate.h"
// Fuchsia change: Remove libraries in favor of "chromium_utils.h"
#include "chromium_utils.h"

namespace media {
namespace {
// The maximum number of supported spatial layers and temporal layers. These
// come from the maximum number of layers currently supported by
// VideoEncodeAccelerator implementation.
constexpr size_t kMaxSpatialLayers = 3;
constexpr size_t kMaxTemporalLayers = 3;

// The maximum size for output buffer, which is chosen empirically for
// 1080p video.
constexpr size_t kMaxBitstreamBufferSizeInBytes = 2 * 1024 * 1024;  // 2MB

// The frame size for 1080p (FHD) video in pixels.
constexpr uint64_t k1080PSizeInPixels = 1920 * 1080;
// The frame size for 1440p (QHD) video in pixels.
constexpr uint64_t k1440PSizeInPixels = 2560 * 1440;

// The mapping from resolution, bitrate, framerate to the bitstream buffer size.
struct BitstreamBufferSizeInfo {
  uint64_t coded_size_area;
  uint32_t bitrate_in_bps;
  uint32_t framerate;
  uint32_t buffer_size_in_bytes;
};

// The bitstream buffer size for each resolution. The table must be sorted in
// increasing order by the resolution. The value is decided by measuring the
// biggest buffer size, and then double the size as margin. (crbug.com/889739)
constexpr BitstreamBufferSizeInfo kBitstreamBufferSizeTable[] = {
    {320 * 180, 100000, 30, 15000},      {640 * 360, 500000, 30, 52000},
    {1280 * 720, 1200000, 30, 110000},   {1920 * 1080, 4000000, 30, 380000},
    {3840 * 2160, 20000000, 30, 970000},
};

// Use quadruple size of kMaxBitstreamBufferSizeInBytes when the input frame
// size is larger than 1440p, double if larger than 1080p. This is chosen
// empirically for some 4k encoding use cases and Android CTS VideoEncoderTest
// (crbug.com/927284).
size_t GetMaxEncodeBitstreamBufferSize(const gfx::Size& size) {
  if (size.Area64() > k1440PSizeInPixels) {
    return kMaxBitstreamBufferSizeInBytes * 4;
  }
  if (size.Area64() > k1080PSizeInPixels) {
    return kMaxBitstreamBufferSizeInBytes * 2;
  }
  return kMaxBitstreamBufferSizeInBytes;
}
}  // namespace

// This function sets the peak equal to the target. The peak can then be
// updated by callers.
VideoBitrateAllocation AllocateBitrateForDefaultEncodingWithBitrates(
    const std::vector<uint32_t>& sl_bitrates,
    const size_t num_temporal_layers,
    const bool uses_vbr) {
  CHECK(!sl_bitrates.empty());
  CHECK_LE(sl_bitrates.size(), kMaxSpatialLayers);

  // The same bitrate factors as the software encoder.
  // https://source.chromium.org/chromium/chromium/src/+/main:media/video/vpx_video_encoder.cc;l=131;drc=d383d0b3e4f76789a6de2a221c61d3531f4c59da
  constexpr auto kTemporalLayersBitrateScaleFactors =
      std::to_array<std::array<double, kMaxTemporalLayers>>({
          {1.00, 0.00, 0.00},  // For one temporal layer.
          {0.60, 0.40, 0.00},  // For two temporal layers.
          {0.50, 0.20, 0.30},  // For three temporal layers.
      });

  CHECK_GT(num_temporal_layers, 0u);
  CHECK_LE(num_temporal_layers, std::size(kTemporalLayersBitrateScaleFactors));
  DCHECK_EQ(std::size(kTemporalLayersBitrateScaleFactors), kMaxTemporalLayers);

  VideoBitrateAllocation bitrate_allocation;
  bitrate_allocation = VideoBitrateAllocation(
      uses_vbr ? Bitrate::Mode::kVariable : Bitrate::Mode::kConstant);
  for (size_t spatial_id = 0; spatial_id < sl_bitrates.size(); ++spatial_id) {
    const uint32_t bitrate_bps = sl_bitrates[spatial_id];
    for (size_t temporal_id = 0; temporal_id < num_temporal_layers;
         ++temporal_id) {
      const double factor =
          kTemporalLayersBitrateScaleFactors[num_temporal_layers - 1]
                                            [temporal_id];
      bitrate_allocation.SetBitrate(
          spatial_id, temporal_id,
          base::saturated_cast<uint32_t>(bitrate_bps * factor));
    }
  }

  return bitrate_allocation;
}

size_t GetEncodeBitstreamBufferSize(const gfx::Size& size,
                                    uint32_t bitrate,
                                    uint32_t framerate) {
  DCHECK_NE(framerate, 0u);
  if (framerate == 0) {
    return GetMaxEncodeBitstreamBufferSize(size);
  }
  for (auto& data : kBitstreamBufferSizeTable) {
    if (size.Area64() <= data.coded_size_area) {
      // The buffer size is proportional to (bitrate / framerate), but linear
      // interpolation for smaller ratio is not enough. Therefore we only use
      // linear extrapolation for larger ratio.
      double ratio =
          std::max(1.0f * static_cast<float>(bitrate / framerate) /
                       static_cast<float>(data.bitrate_in_bps / data.framerate),
                   1.0f);
      // Use saturated_cast to avoid float-to-int overflow UB if ratio is very
      // large before std::min clamps to GetMaxEncodeBitstreamBufferSize(size).
      return std::min(
          base::saturated_cast<size_t>(data.buffer_size_in_bytes * ratio),
          GetMaxEncodeBitstreamBufferSize(size));
    }
  }
  return GetMaxEncodeBitstreamBufferSize(size);
}

// Get the maximum output bitstream buffer size. Since we don't change the
// buffer size when we update bitrate and framerate, we have to calculate the
// buffer size for the maximum bitrate.
// However, the maximum bitrate for intel chipset is 40Mbps. The buffer size
// calculated with this bitrate is always larger than 2MB. Therefore we just
// return the value.
// TODO(crbug.com/889739): Deprecate this function after we can update the
// buffer size while requesting new bitrate and framerate.
size_t GetEncodeBitstreamBufferSize(const gfx::Size& size) {
  return GetMaxEncodeBitstreamBufferSize(size);
}

std::vector<uint8_t> GetFpsAllocation(size_t num_temporal_layers) {
  DCHECK_LT(num_temporal_layers, 4u);
  constexpr uint8_t kFullAllocation = 255;
  // The frame rate fraction is given as an 8 bit unsigned integer where 0 = 0%
  // and 255 = 100%. Each layer's allocated fps refers to the previous one, so
  // e.g. your camera is opened at 30fps, and you want to have decode targets at
  // 15fps and 7.5fps as well:
  // TL0 then gets an allocation of 7.5/30 = 1/4. TL1 adds another 7.5fps to end
  // up at (7.5 + 7.5)/30 = 15/30 = 1/2 of the total allocation. TL2 adds the
  // final 15fps to end up at (15 + 15)/30, which is the full allocation.
  // Therefore, fps_allocation values are as follows,
  // fps_allocation[0][0] = kFullAllocation / 4;
  // fps_allocation[0][1] = kFullAllocation / 2;
  // fps_allocation[0][2] = kFullAllocation;
  //  For more information, see webrtc::VideoEncoderInfo::fps_allocation.
  switch (num_temporal_layers) {
    case 1:
      // In this case, the number of spatial layers must great than 1.
      return {kFullAllocation};
    case 2:
      return {kFullAllocation / 2, kFullAllocation};
    case 3:
      return {kFullAllocation / 4, kFullAllocation / 2, kFullAllocation};
    default:
      NOTREACHED() << "Unsupported temporal layers";
  }
}

VideoBitrateAllocation AllocateBitrateForDefaultEncoding(
    const VideoEncodeAccelerator::Config& config) {
  if (config.bitrate.mode() == Bitrate::Mode::kExternal) {
    return VideoBitrateAllocation(Bitrate::Mode::kExternal);
  }

  VideoBitrateAllocation allocation;
  const bool use_vbr = config.bitrate.mode() == Bitrate::Mode::kVariable;
  if (config.spatial_layers.empty()) {
    allocation = AllocateBitrateForDefaultEncodingWithBitrates(
        {config.bitrate.target_bps()},
        /*num_temporal_layers=*/1u, use_vbr);
    if (use_vbr) {
      allocation.SetPeakBps(config.bitrate.peak_bps());
    }
    return allocation;
  }

  const size_t num_temporal_layers =
      config.spatial_layers[0].num_of_temporal_layers;
  std::vector<uint32_t> bitrates;
  bitrates.reserve(config.spatial_layers.size());
  for (const auto& spatial_layer : config.spatial_layers) {
    DCHECK_EQ(spatial_layer.num_of_temporal_layers, num_temporal_layers);
    bitrates.push_back(spatial_layer.bitrate_bps);
  }

  allocation = AllocateBitrateForDefaultEncodingWithBitrates(
      bitrates, num_temporal_layers, use_vbr);
  if (use_vbr) {
    allocation.SetPeakBps(config.bitrate.peak_bps());
  }
  return allocation;
}

VideoBitrateAllocation AllocateDefaultBitrateForTesting(
    const size_t num_spatial_layers,
    const size_t num_temporal_layers,
    const Bitrate& bitrate) {
  // Higher spatial layers (those to the right) get more bitrate.
  constexpr auto kSpatialLayersBitrateScaleFactors =
      std::to_array<std::array<double, kMaxSpatialLayers>>({
          {1.00, 0.00, 0.00},  // For one spatial layer.
          {0.30, 0.70, 0.00},  // For two spatial layers.
          {0.07, 0.23, 0.70},  // For three spatial layers.
      });

  CHECK_GT(num_spatial_layers, 0u);
  CHECK_LE(num_spatial_layers, std::size(kSpatialLayersBitrateScaleFactors));
  DCHECK_EQ(std::size(kSpatialLayersBitrateScaleFactors), kMaxSpatialLayers);

  std::vector<uint32_t> bitrates(num_spatial_layers);
  for (size_t sid = 0; sid < num_spatial_layers; ++sid) {
    const double bitrate_factor =
        kSpatialLayersBitrateScaleFactors[num_spatial_layers - 1][sid];
    bitrates[sid] =
        static_cast<uint32_t>(bitrate.target_bps() * bitrate_factor);
  }

  const bool use_vbr = bitrate.mode() == Bitrate::Mode::kVariable;
  auto allocation = AllocateBitrateForDefaultEncodingWithBitrates(
      bitrates, num_temporal_layers, use_vbr);
  if (use_vbr)
    allocation.SetPeakBps(bitrate.peak_bps());
  return allocation;
}

VideoBitrateAllocation BitrateToBitrateAllocation(const Bitrate& bitrate) {
  VideoBitrateAllocation allocation(bitrate.mode());
  switch (bitrate.mode()) {
    case Bitrate::Mode::kVariable:
      allocation.SetBitrate(0, 0, bitrate.target_bps());
      allocation.SetPeakBps(bitrate.peak_bps());
      break;
    case Bitrate::Mode::kConstant:
      allocation.SetBitrate(0, 0, bitrate.target_bps());
      break;
    case Bitrate::Mode::kExternal:
      break;
  }
  return allocation;
}

#if CHROMIUM_CODE
VEAEncodingLatencyMetricsHelper::VEAEncodingLatencyMetricsHelper(
    const std::string& uma_prefix,
    VideoCodec codec)
    : uma_name_(uma_prefix + GetCodecNameForUMA(codec)) {}

VEAEncodingLatencyMetricsHelper::~VEAEncodingLatencyMetricsHelper() {
  if (frame_count_ == 0) {
    return;
  }

  base::UmaHistogramCounts1000(uma_name_, total_encode_time_ms_ / frame_count_);
}

void VEAEncodingLatencyMetricsHelper::EncodeOneFrame(
    bool is_key_frame,
    base::TimeDelta time_delta) {
  const uint64_t delta_ms = time_delta.InMilliseconds();

  if (!base::CheckAdd(total_encode_time_ms_, delta_ms).IsValid()) {
    // If overflow happens, report the metrics and reset the counters.
    // Use checked math to detect overflow when adding delta_ms to
    // total_encode_time_ms_. This avoids manual limit arithmetic and is
    // clearer about intent.
    if (frame_count_ > 0) {
      base::UmaHistogramCounts1000(uma_name_,
                                   total_encode_time_ms_ / frame_count_);
    }
    frame_count_ = 0;
    total_encode_time_ms_ = 0;
  }

  frame_count_++;
  total_encode_time_ms_ += delta_ms;
  if (is_key_frame) {
    base::UmaHistogramCounts1000(uma_name_,
                                 total_encode_time_ms_ / frame_count_);
    frame_count_ = 0;
    total_encode_time_ms_ = 0;
  }
}
#endif

}  // namespace media
