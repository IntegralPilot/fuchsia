// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_READER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_READER_H_

#include <lib/fit/result.h>

#include <string>

#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/decoder.h"
#include "src/developer/forensics/utils/storage_size.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {

enum class ReaderError : std::uint8_t {
  kIoError,
  kDecompressionError,
};

// Reads the encoded logs in |logs_dir|, decodes them using |decoder|, sorts log messages
// chronologically by timestamp, and aggregates consecutive repeated messages. Calculates the
// resulting |compression_ratio|.
//
// Returns the processed log string on success, or a ReaderError on failure.
fit::result<ReaderError, std::string> Concatenate(const std::string& logs_dir,
                                                  StorageSize max_decompressed_size,
                                                  Decoder* decoder, float* compression_ratio);

}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_READER_H_
