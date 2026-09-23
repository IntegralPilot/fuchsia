// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_MOUNT_H_
#define SRC_STORAGE_F2FS_MOUNT_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.process.lifecycle/cpp/wire.h>

namespace f2fs {

enum class MountOption {
  kBgGcOff = 0,
  kDisableRollForward,
  kDiscard,
  kNoUserXAttr,
  kNoAcl,
  kDisableExtIdentify,
  kInlineXattr,
  kInlineDentry,
  kForceLfs,
  kReadOnly,
  kReadExtentCache,
  kActiveLogs,  // It should be (kOptMaxNum - 1).
  kMaxNum,
};

constexpr size_t kMaxOptionCount = static_cast<size_t>(MountOption::kMaxNum);

class MountOptions {
 public:
  MountOptions() = default;
  MountOptions(const MountOptions &) = default;

  zx::result<size_t> GetValue(const MountOption option) const;
  zx_status_t SetValue(const MountOption option, const size_t value);
  static std::vector<MountOption> Iter() {
    std::vector<MountOption> iter;
    for (size_t i = 0; i < kMaxOptionCount; ++i) {
      iter.push_back(static_cast<MountOption>(i));
    }
    return iter;
  }

 private:
  // default values
  // "background_gc_off", 1
  // "disable_roll_forward", 0
  // "discard", 1
  // "nouser_xattr", 1
  // "noacl", 1
  // "disable_ext_identify", 0
  // "inline_xattr", 0
  // "inline_dentry", 1
  // "mode", Lfs (1), Adaptive (0)
  // "readonly", 0
  // "read_extent_cache", 1
  // "active_logs", 6
  std::array<size_t, kMaxOptionCount> opt_ = {1, 0, 1, 1, 1, 0, 0, 1, 0, 0, 1, 6};
};

zx::result<> StartComponent(fidl::ServerEnd<fuchsia_io::Directory> root,
                            fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle);

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_MOUNT_H_
