// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_LIB_FORMAT2_STREAM_CONVERTER_H_
#define SRC_MEDIA_AUDIO_LIB_FORMAT2_STREAM_CONVERTER_H_

#include "src/media/audio/lib/format2/format.h"

namespace media_audio {

// Converts a stream of audio samples from a source sample format to a destination sample format.
class StreamConverter {
 public:
  // Creates a new `StreamConverter` for the given `source_format` and `dest_format`.
  //
  // Required: `source_format` and `dest_format` must have matching frame rates and channel counts.
  StreamConverter(const Format& source_format, const Format& dest_format);

  // Creates a new `StreamConverter` with the assumption that the source sample type is `float`.
  // TODO(https://fxbug.dev/42066206): remove when old audio_core code is gone
  static StreamConverter CreateFromFloatSource(const Format& dest_format);

  // Converts `frame_count` frames in `source_samples` from the source format into the destination
  // format, then writes the converted samples into `dest_samples`.
  void Copy(const void* source_samples, void* dest_samples, int64_t frame_count) const;

  // Like `Copy`, but also clips the output when the destination format uses floating point samples.
  void CopyAndClip(const void* source_samples, void* dest_samples, int64_t frame_count) const;

  // Writes `frame_count` silent frames to `dest_samples`.
  void WriteSilence(void* dest_samples, int64_t frame_count) const;

  const Format& source_format() const { return source_format_; }
  const Format& dest_format() const { return dest_format_; }

 private:
  Format source_format_;
  Format dest_format_;
  bool should_convert_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_LIB_FORMAT2_STREAM_CONVERTER_H_
