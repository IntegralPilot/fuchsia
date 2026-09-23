// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/inspect.h"

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vnode.h"

namespace f2fs {

InspectTree::InspectTree(F2fs* fs) : fs_(fs) { ZX_ASSERT(fs_ != nullptr); }

void InspectTree::Initialize() {
  zx::result<fs::FilesystemInfo> fs_info = fs_->GetFilesystemInfo();
  if (fs_info.is_error()) {
    FX_LOGS(ERROR) << "Failed to initialize F2fs inspect tree: GetFilesystemInfo returned "
                   << fs_info.status_string();
    return;
  }

  {
    std::lock_guard guard(info_mutex_);
    info_ = {
        .id = fs_info.value().fs_id,
        .type = fidl::ToUnderlying(fs_info.value().fs_type),
        .name = fs_info.value().name,
        .version_major = LeToCpu(fs_->GetSuperblockInfo().GetSuperblock().major_ver),
        .version_minor = LeToCpu(fs_->GetSuperblockInfo().GetSuperblock().minor_ver),
        .block_size = fs_info.value().block_size,
        .max_filename_length = fs_info.value().max_filename_size,
    };
  }

  {
    std::lock_guard guard(usage_mutex_);
    UpdateUsage();
  }

  {
    std::lock_guard guard(fvm_mutex_);
    UpdateFvmSizeInfo();
  }

  fs_inspect_nodes_ = fs_inspect::CreateTree(inspector_.GetRoot(), CreateCallbacks());
  inspector_.CreateStatsNode();
}

void InspectTree::UpdateUsage() {
  zx::result<fs::FilesystemInfo> fs_info = fs_->GetFilesystemInfo();
  if (fs_info.is_error()) {
    FX_LOGS(ERROR) << "Failed to initialize F2fs inspect tree: GetFilesystemInfo returned "
                   << fs_info.status_string();
    return;
  }

  usage_.total_bytes = fs_info.value().total_bytes;
  usage_.used_bytes = fs_info.value().used_bytes;
  usage_.total_nodes = fs_info.value().total_nodes;
  usage_.used_nodes = fs_info.value().used_nodes;
}

void InspectTree::UpdateFvmSizeInfo() {
  zx::result<fs_inspect::FvmData::SizeInfo> size_info = zx::ok(fs_inspect::FvmData::SizeInfo{
      .size_bytes = 0, .size_limit_bytes = 0, .available_space_bytes = 0});

  {
    fs_->GetBc().ForEachBcache([&size_info](Bcache* bc) {
      if (size_info.is_error()) {
        return;
      }

      auto info = fs_inspect::FvmData::GetSizeInfoFromDevice(*bc->GetDevice());
      if (info.is_error()) {
        size_info = info.take_error();
        return;
      }

      size_info.value().size_bytes += info->size_bytes;
      size_info.value().size_limit_bytes += info->size_limit_bytes;
      size_info.value().available_space_bytes += info->available_space_bytes;
    });

    if (size_info.is_error()) {
      FX_LOGS(WARNING) << "Failed to obtain size information from block device: "
                       << size_info.status_string();
    }
  }

  if (size_info.is_ok()) {
    fvm_.size_info = size_info.value();
  }
}

void InspectTree::OnOutOfSpace() {
  zx::time curr_time = zx::clock::get_monotonic();
  std::lock_guard guard(fvm_mutex_);
  if ((curr_time - last_out_of_space_time_) > kOutOfSpaceDuration) {
    ++fvm_.out_of_space_events;
    last_out_of_space_time_ = curr_time;
  }
}

fs_inspect::NodeCallbacks InspectTree::CreateCallbacks() {
  return {
      .info_callback =
          [this] {
            std::lock_guard guard(info_mutex_);
            return info_;
          },
      .usage_callback =
          [this] {
            std::lock_guard guard(usage_mutex_);
            UpdateUsage();
            return usage_;
          },
      .fvm_callback =
          [this] {
            std::lock_guard guard(fvm_mutex_);
            UpdateFvmSizeInfo();
            return fvm_;
          },
  };
}

}  // namespace f2fs
