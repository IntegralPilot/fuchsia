// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/files/file.h"

#include <assert.h>
#include <fcntl.h>
#include <limits.h>
#include <sys/stat.h>
#include <unistd.h>

#define FILE_CREATE_MODE 0666
#define BINARY_MODE 0

#include <fbl/unique_fd.h>

#include "src/lib/files/eintr_wrapper.h"
#include "src/lib/files/file_descriptor.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace files {
namespace {

template <typename T>
bool ReadFileDescriptor(int fd, T* result) {
  assert(result);
  result->clear();

  struct stat s;
  if (int res = fstat(fd, &s); res < 0) {
    return false;
  }
  // Some (many?) file implementations on Fuchsia do not report their size.
  //
  // See e.g. https://fxbug.dev/42070489.
  const size_t size = s.st_size;
  if (size != 0) {
    result->resize(size);
  }

  for (size_t offset = 0;;) {
    size_t count = [&]() {
      if (size != 0) {
        return size - offset;
      }
      constexpr size_t kBufferSize = 1 << 16;
      result->resize(offset + kBufferSize);
      return kBufferSize;
    }();
    ssize_t bytes_read = HANDLE_EINTR(read(fd, &result->data()[offset], count));
    if (bytes_read < 0) {
      result->clear();
      return false;
    }
    if (bytes_read == 0) {
      result->resize(offset);
      return true;
    }
    offset += bytes_read;
  }
}

}  // namespace

bool WriteFile(const std::string& path, std::string_view data) {
  return WriteFile(path, data.data(), data.size());
}

bool WriteFile(const std::string& path, const char* data, ssize_t size) {
  return WriteFileAt(AT_FDCWD, path, data, size);
}

bool WriteFileAt(int dirfd, const std::string& path, const char* data, ssize_t size) {
  fbl::unique_fd fd(
      HANDLE_EINTR(openat(dirfd, path.c_str(), O_CREAT | O_TRUNC | O_WRONLY, FILE_CREATE_MODE)));
  if (!fd.is_valid())
    return false;
  return fxl::WriteFileDescriptor(fd.get(), data, size);
}

bool WriteFileInTwoPhases(const std::string& path, std::string_view data,
                          const std::string& temp_root) {
  ScopedTempDir temp_dir(temp_root);

  std::string temp_file_path;
  if (!temp_dir.NewTempFile(&temp_file_path)) {
    return false;
  }

  if (!WriteFile(temp_file_path, data.data(), data.size())) {
    return false;
  }

  if (rename(temp_file_path.c_str(), path.c_str()) != 0) {
    return false;
  }

  return true;
}

bool ReadFileToString(const std::string& path, std::string* result) {
  return ReadFileToStringAt(AT_FDCWD, path, result);
}

bool ReadFileDescriptorToString(int fd, std::string* result) {
  return ReadFileDescriptor(fd, result);
}

bool ReadFileToStringAt(int dirfd, const std::string& path, std::string* result) {
  fbl::unique_fd fd(openat(dirfd, path.c_str(), O_RDONLY));
  return ReadFileDescriptor(fd.get(), result);
}

bool ReadFileToVector(const std::string& path, std::vector<uint8_t>* result) {
  fbl::unique_fd fd(open(path.c_str(), O_RDONLY | BINARY_MODE));
  return ReadFileDescriptor(fd.get(), result);
}

bool ReadFileDescriptorToVector(int fd, std::vector<uint8_t>* result) {
  return ReadFileDescriptor(fd, result);
}

bool ReadFileToVectorAt(int dirfd, const std::string& path, std::vector<uint8_t>* result) {
  fbl::unique_fd fd(openat(dirfd, path.c_str(), O_RDONLY));
  return ReadFileDescriptor(fd.get(), result);
}

bool IsFile(const std::string& path) { return IsFileAt(AT_FDCWD, path); }

bool IsFileAt(int dirfd, const std::string& path) {
  struct stat stat_buffer;
  if (fstatat(dirfd, path.c_str(), &stat_buffer, /* flags = */ 0) != 0)
    return false;
  return S_ISREG(stat_buffer.st_mode);
}

bool GetFileSize(const std::string& path, uint64_t* size) {
  return GetFileSizeAt(AT_FDCWD, path, size);
}

bool GetFileSizeAt(int dirfd, const std::string& path, uint64_t* size) {
  struct stat stat_buffer;
  if (fstatat(dirfd, path.c_str(), &stat_buffer, /* flags = */ 0) != 0)
    return false;
  *size = stat_buffer.st_size;
  return true;
}

}  // namespace files
