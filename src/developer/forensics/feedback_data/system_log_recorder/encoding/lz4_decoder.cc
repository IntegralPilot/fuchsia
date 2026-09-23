// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/lz4_decoder.h"

#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/lz4_utils.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {

const std::string kDecodingErrorStr = "!!! DECODING ERROR !!!\n";
const std::string kDecodingSizeErrorStr =
    "!!! CANNOT DECODE %lu BYTES. THERE ARE ONLY %lu BYTES LEFT !!!\n";

Lz4Decoder::Lz4Decoder(const std::optional<size_t> output_preallocation_hint)
    : ring_(kDecoderRingBufferSize),
      stream_(LZ4_createStreamDecode(), LZ4_freeStreamDecode),
      output_preallocation_hint_(output_preallocation_hint) {}

std::string Lz4Decoder::DecodeWithoutReset(const std::string& chunks) {
  std::string decoded_data;
  if (output_preallocation_hint_.has_value() && *output_preallocation_hint_ != 0) {
    decoded_data.reserve(*output_preallocation_hint_);
  }

  const char* ptr = chunks.data();
  const char* end = chunks.data() + chunks.size();
  while (ptr < end) {
    const uint16_t encoded_bytes = DecodeSize(&ptr);

    // This indicates that the encoder reset its stream because it became invalid. If so, we reset
    // the decoder too.
    if (encoded_bytes == kEncodeSizeError) {
      Reset();
      continue;
    }

    if (ptr + encoded_bytes > end) {
      const size_t bytes_left = end - ptr;
      decoded_data.append(
          fxl::StringPrintf(kDecodingSizeErrorStr.c_str(), encoded_bytes, bytes_left));
      break;
    }

    const int decoded_bytes = LZ4_decompress_safe_continue(stream_.get(), ptr, ring_.GetPtr(),
                                                           encoded_bytes, kMaxChunkSize);
    // Check for decoding error.
    if (decoded_bytes < 0) {
      decoded_data.append(kDecodingErrorStr);
      break;
    }

    decoded_data.append(ring_.GetPtr(), decoded_bytes);

    // Update index variables.
    ptr += encoded_bytes;
    ring_.Advance(decoded_bytes);
  }

  return decoded_data;
}

std::string Lz4Decoder::Decode(const std::string& block) {
  const std::string output = DecodeWithoutReset(block);
  Reset();
  return output;
}

void Lz4Decoder::Reset() {
  stream_.reset(LZ4_createStreamDecode());
  ring_.Reset();
}

}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics
