// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_BASE_DECODER_BUFFER_H_
#define SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_BASE_DECODER_BUFFER_H_

#include <vector>

#include <lib/stdcompat/span.h>
#include "media/base/decrypt_config.h"

#include <lib/fit/defer.h>
#include <lib/media/codec_impl/codec_buffer.h>

namespace media {
class DecoderBuffer {
 public:
  DecoderBuffer(cpp20::span<const uint8_t> buffer,
                const CodecBuffer* maybe_codec_buffer,
                uint32_t buffer_start_offset,
                fit::deferred_callback return_input_packet)
      : external_buffer_(buffer),
        maybe_codec_buffer_(maybe_codec_buffer),
        buffer_start_offset_(buffer_start_offset),
        return_input_packet_(std::move(return_input_packet)) {
    ZX_DEBUG_ASSERT(static_cast<bool>(return_input_packet_));
  }

  explicit DecoderBuffer(cpp20::span<const uint8_t> buffer)
      : external_buffer_(buffer) {}

  DecoderBuffer(std::vector<uint8_t> owning_buffer,
                const CodecBuffer* maybe_codec_buffer,
                uint32_t buffer_start_offset,
                fit::deferred_callback return_input_packet)
      : owning_buffer_(std::move(owning_buffer)),
        maybe_codec_buffer_(maybe_codec_buffer),
        buffer_start_offset_(buffer_start_offset),
        return_input_packet_(std::move(return_input_packet)) {
    ZX_DEBUG_ASSERT(static_cast<bool>(return_input_packet_));
  }

  explicit DecoderBuffer(std::vector<uint8_t> owning_buffer)
      : owning_buffer_(std::move(owning_buffer)) {}

  // Explicitly disable copying
  DecoderBuffer(const DecoderBuffer&) = delete;
  DecoderBuffer& operator=(const DecoderBuffer&) = delete;

  // Explicitly enable moving
  DecoderBuffer(DecoderBuffer&&) = default;
  DecoderBuffer& operator=(DecoderBuffer&& other) = default;
  size_t size() const { return as_span().size(); }
  cpp20::span<const uint8_t> as_span() const {
    return !owning_buffer_.empty() ? cpp20::span<const uint8_t>(owning_buffer_)
                                   : external_buffer_;
  }

  auto begin() const { return as_span().begin(); }
  auto end() const { return as_span().end(); }
  auto first(size_t count) const { return as_span().first(count); }

  auto subspan(size_t offset, size_t count) const {
    return as_span().subspan(offset, count);
  }

  const uint8_t* side_data() const { return side_data_.get(); }
  size_t side_data_size() const { return side_data_size_; }

  const DecryptConfig* decrypt_config() const { return nullptr; }
  // Fuchsia change: is_encrypted() returning false does not prevent DRM
  // playback on Fuchsia, as we have a separate DecryptAdapter and the HW reads
  // from protected VDEC memory.
  bool is_encrypted() const { return false; }

  const CodecBuffer* codec_buffer() const { return maybe_codec_buffer_; }
  uint32_t buffer_start_offset() const { return buffer_start_offset_; }

  bool end_of_stream() const { return false; }

  // Returns total memory usage for both bookkeeping and buffered data.
  size_t GetMemoryUsage() const { return sizeof(*this) + as_span().size(); }

 private:
  // Non-empty only when DecoderBuffer owns the underlying data (e.g. converted
  // AVCC-to-Annex-B data).
  std::vector<uint8_t> owning_buffer_;
  // Non-empty only when DecoderBuffer does not own the underlying data.
  cpp20::span<const uint8_t> external_buffer_;

  // If codec_buffer_, the data_ is also available at codec_buffer_.base() +
  // buffer_start_offset_ and potentially at codec_buffer_.phys_base() +
  // buffer_start_offset_.
  const CodecBuffer* maybe_codec_buffer_ = nullptr;
  // If codec_buffer_, this is the offset at which data_ starts within
  // codec_buffer_.
  uint32_t buffer_start_offset_ = 0;
  // If codec_buffer_, ~return_input_packet_ will recycle the input packet, so
  // the portion of codec_buffer_ can be re-used.
  fit::deferred_callback return_input_packet_;

  // Side data. Used for alpha channel in VPx, and for text cues.
  size_t side_data_size_;
  std::unique_ptr<uint8_t[]> side_data_;
};

}  // namespace media

#endif  // SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_BASE_DECODER_BUFFER_H_
