// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_TIMESTAMP_H_
#define SRC_STORAGE_F2FS_TIMESTAMP_H_

#include <zircon/types.h>

namespace f2fs {

enum class UpdateMode {
  kRelative = 0,
  kNoAccess,
  kStrict,
  kNum,
};

class Timestamps {
 public:
  Timestamps(UpdateMode mode, const timespec &atime, const timespec &btime, const timespec &ctime,
             const timespec &mtime);

  // Tagging types
  struct AccessTime {};
  struct BirthTime {};
  struct ChangeTime {};
  struct ModificationTime {};

  template <typename T>
  const timespec &Get() const;

  template <typename U>
  void Update();

  template <typename W>
  void Update(const timespec &t);

  // Adjust time intervals for access time and modification time. A call to Update() is effective
  // only when |atime_| or |mtime_| has a time value before the interval.
  void SetAccessTimeInterval(time_t secs = kDayInSec) { access_time_interval_ = secs; }
  void SetModificationTimeInterval(time_t nsecs = 100) { modification_time_interval_ = nsecs; }

  // |dirty_| is set when a call to Update() methods is effective. A caller can use it to detect any
  // change on time values after the last call to ClearDirty().
  void ClearDirty() { dirty_ = false; }
  bool IsDirty() const { return dirty_; }

  static constexpr size_t kDayInSec = 60 * 60 * 24;

 private:
  UpdateMode mode_{};
  timespec access_time_{};
  timespec birth_time_{};
  timespec change_time_{};
  timespec modification_time_{};
  bool dirty_ = false;
  time_t access_time_interval_ = kDayInSec;  // sec
  time_t modification_time_interval_ = 100;  // ns
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_TIMESTAMP_H_
