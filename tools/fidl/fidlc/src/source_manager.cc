// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/source_manager.h"

#include <errno.h>
#include <sys/stat.h>

#include <utility>

#include "src/lib/fxl/strings/utf_codecs.h"

namespace fidlc {

bool SourceManager::CreateSource(std::string_view filename, const char** failure_reason) {
  struct stat s;
  if (stat(filename.data(), &s) != 0) {
    if (failure_reason) {
      *failure_reason = strerror(errno);
    }
    return false;
  }

  if ((s.st_mode & S_IFREG) != S_IFREG) {
    if (failure_reason) {
      *failure_reason = "Not a regular file";
    }
    return false;
  }

  FILE* file = fopen(filename.data(), "rb");
  if (!file) {
    if (failure_reason) {
      *failure_reason = strerror(errno);
    }
    return false;
  }

  // The lexer requires zero terminated data.
  std::string data;
  fseek(file, 0, SEEK_END);
  auto filesize = ftell(file);
  data.resize(filesize);
  rewind(file);
  fread(data.data(), 1, filesize, file);
  fclose(file);

  if (!fxl::IsStringUTF8(data)) {
    if (failure_reason) {
      *failure_reason = "Not valid UTF-8";
    }
    return false;
  }

  AddSourceFile(std::make_unique<SourceFile>(std::string(filename), std::move(data)));
  if (failure_reason) {
    *failure_reason = nullptr;
  }
  return true;
}

void SourceManager::AddSourceFile(std::unique_ptr<SourceFile> file) {
  sources_.push_back(std::move(file));
}

}  // namespace fidlc
