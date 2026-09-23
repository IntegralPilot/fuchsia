// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_SOURCE_FILE_PROVIDER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_SOURCE_FILE_PROVIDER_H_

#include <ctime>
#include <string>

#include "src/developer/debug/zxdb/common/err_or.h"

namespace zxdb {

// Interface to provide source code. The default implementation fails for all requests. See
// SourceFileProviderImpl.
class SourceFileProvider {
 public:
  struct FileMetadata {
    FileMetadata() = default;
    FileMetadata(std::string path, std::time_t mtime)
        : full_path(std::move(path)), modification_time(mtime) {}

    // Resolved file path. This will be concatenated with the search path. If the search path
    // is system-absolute, this path will be, but if the search path is relative to the current
    // working directory, so will this be.
    std::string full_path;

    std::time_t modification_time = 0;
  };

  struct FileData : public FileMetadata {
    FileData() = default;
    FileData(std::string c, FileMetadata metadata)
        : FileMetadata(std::move(metadata)), contents(std::move(c)) {}
    FileData(std::string c, std::string path, std::time_t mtime)
        : FileMetadata(std::move(path), mtime), contents(std::move(c)) {}

    std::string contents;
  };

  virtual ~SourceFileProvider() = default;

  // Attempts to get the metadata of the given file. The compilation directory referenced by this
  // file's symbols can be specified as `file_build_dir` for out of tree use.
  virtual ErrOr<FileMetadata> GetFileMetadata(const std::string& file_name,
                                              const std::string& file_build_dir) const {
    return Err("Source metadata not available.");
  }

  // Attempts to read the metadata and contents of the given file. The compilation directory
  // referenced by this file's symbols can be specified as `file_build_dir` for out of tree use.
  virtual ErrOr<FileData> GetFileData(const std::string& file_name,
                                      const std::string& file_build_dir) const {
    return Err("Source not available.");
  }
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_SOURCE_FILE_PROVIDER_H_
