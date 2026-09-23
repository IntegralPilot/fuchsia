// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/fs/hack_filesystem.h"

#include <lib/syslog/cpp/macros.h>

#include "src/lib/files/file.h"
#include "src/lib/files/path.h"

#if defined(__Fuchsia__)
#include <fidl/fuchsia.io/cpp/fidl.h>
#endif

#if defined(__linux__)
#include <limits.h>
#include <unistd.h>

#include "src/lib/files/path.h"
#endif

namespace escher {
HackFilesystemPtr HackFilesystem::New(const char* root) {
  auto hfs = fxl::MakeRefCounted<HackFilesystem>();
  hfs->InitializeWithBasePath(root);
  return hfs;
}

#ifdef __Fuchsia__
HackFilesystemPtr HackFilesystem::New(fidl::ClientEnd<fuchsia_io::Directory> dir) {
  auto hfs = fxl::MakeRefCounted<HackFilesystem>();
  hfs->InitializeWithBaseDir(std::move(dir));
  return hfs;
}
#endif

HackFilesystem::~HackFilesystem() { FX_DCHECK(watchers_.size() == 0); }

HackFileContents HackFilesystem::ReadFile(const HackFilePath& path) {
  auto it = files_.find(path);
  if (it != files_.end()) {
    return it->second;
  }

#if defined(__Fuchsia__)
  if (base_dir().has_value()) {
    LoadFileAtDir(this, path);
  } else if (base_path_) {
    LoadFile(this, *base_path_, path);
  }
#else
  if (base_path_) {
    LoadFile(this, *base_path_, path);
  }
#endif

  it = files_.find(path);
  if (it != files_.end()) {
    return it->second;
  }
  return "";
}

void HackFilesystem::WriteFile(const HackFilePath& path, HackFileContents new_contents) {
  auto it = files_.find(path);
  if (it != files_.end() && it->second == new_contents) {
    // Avoid invalidation if the contents don't change.
    return;
  }
  bool existed = it != files_.end();
  files_[path] = std::move(new_contents);
  if (existed) {
    InvalidateFile(path);
  }
}

void HackFilesystem::InvalidateFile(const HackFilePath& path) {
  for (auto w : watchers_) {
    if (w->IsWatchingPath(path)) {
      w->callback_(path);
    }
  }
}

std::unique_ptr<HackFilesystemWatcher> HackFilesystem::RegisterWatcher(
    HackFilesystemWatcherFunc func) {
  // Private constructor, so cannot use std::make_unique.
  auto watcher = new HackFilesystemWatcher(this, std::move(func));
  return std::unique_ptr<HackFilesystemWatcher>(watcher);
}

void HackFilesystem::InitializeWithBasePath(const char* root) {
  if (!root)
    return;

#if defined(__Fuchsia__)
  base_path_.emplace(root);
#elif defined(__linux__)
  if (root[0] != '.') {
    FX_LOGS(ERROR) << "root must be a relative path: " << root;
  }
  char test_path[PATH_MAX];
  const char exe_link[] = "/proc/self/exe";
  realpath(exe_link, test_path);
  base_path_ = {files::SimplifyPath(files::JoinPath(test_path, root))};
#else
#error Unsupported Platform
#endif
}

#if defined(__Fuchsia__)
void HackFilesystem::InitializeWithBaseDir(fidl::ClientEnd<fuchsia_io::Directory> dir) {
  base_dir_.emplace(std::move(dir));
}

// static
bool HackFilesystem::LoadFileAtDir(HackFilesystem* fs, const HackFilePath& path) {
  std::string contents;
  const auto flags = fuchsia_io::Flags::kPermReadBytes | fuchsia_io::Flags::kProtocolFile;
  auto [client, server] = *fidl::CreateEndpoints<fuchsia_io::File>();
  fidl::Request<fuchsia_io::Directory::Open> dir_request;
  dir_request.path(path);
  dir_request.flags(flags);
  dir_request.options(fuchsia_io::Options{});
  dir_request.object(server.TakeChannel());
  if (auto r = (*fs->base_dir())->Open(std::move(dir_request)); r.is_error()) {
    FX_LOGS(WARNING) << "Failed to open directory: " << r.error_value();
    return false;
  }

  fidl::SyncClient<fuchsia_io::File> file(std::move(client));
  while (true) {
    fidl::Request<fuchsia_io::File::Read> file_request;
    file_request.count(fuchsia_io::kMaxBuf);
    auto r = file->Read(file_request);
    if (r.is_error()) {
      FX_LOGS(WARNING) << "Failed to read file " << path << ": " << r.error_value();
      return false;
    }
    if (r->data().empty()) {
      break;
    }
    contents.append(reinterpret_cast<const char*>(r->data().data()), r->data().size());
  }

  fs->WriteFile(path, contents);
  return true;
}
#endif

HackFilesystemWatcher::HackFilesystemWatcher(HackFilesystem* filesystem,
                                             HackFilesystemWatcherFunc callback)
    : filesystem_(filesystem), callback_(std::move(callback)) {
  filesystem_->watchers_.insert(this);
}

HackFilesystemWatcher::~HackFilesystemWatcher() {
  size_t erased = filesystem_->watchers_.erase(this);
  FX_DCHECK(erased == 1);
}

// static
bool HackFilesystem::LoadFile(HackFilesystem* fs, const HackFilePath& root,
                              const HackFilePath& path) {
  FX_DCHECK(fs);
  std::string contents;
  std::string fullpath = files::JoinPath(root, path);
  if (files::ReadFileToString(fullpath, &contents)) {
    fs->WriteFile(path, contents);
    return true;
  }
  FX_LOGS(WARNING) << "Failed to read file: " << fullpath;
  return false;
}

}  // namespace escher
