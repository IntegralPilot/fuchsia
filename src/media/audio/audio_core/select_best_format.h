// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_SELECT_BEST_FORMAT_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_SELECT_BEST_FORMAT_H_

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fuchsia/hardware/audio/cpp/fidl.h>
#include <fuchsia/media/cpp/fidl.h>
#include <lib/zx/result.h>
#include <stdint.h>
#include <zircon/device/audio.h>
#include <zircon/types.h>

#include <vector>

#include "src/media/audio/lib/format2/format.h"

namespace media::audio {

// Given a preferred format and a list of driver supported formats, selects the "best" form and
// update the in/out parameters, then return ZX_OK.  If no formats exist, or all format ranges get
// completely rejected, return an error and leave the in/out params as they were.
zx_status_t SelectBestFormat(const std::vector<fuchsia::hardware::audio::PcmSupportedFormats>& fmts,
                             uint32_t* frames_per_second_inout, uint32_t* channels_inout,
                             fuchsia::media::AudioSampleFormat* sample_format_inout);

zx::result<media_audio::Format> SelectBestFormat(
    const std::vector<fuchsia_audio_device::PcmFormatSet>& fmts, const media_audio::Format& pref);

// Given a format and a list of driver supported formats, if the format is found in the driver
// supported list then return true, otherwise return false.
bool IsFormatInSupported(
    const fuchsia::media::AudioStreamType& stream_type,
    const std::vector<fuchsia::hardware::audio::PcmSupportedFormats>& supported_formats);

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_SELECT_BEST_FORMAT_H_
