// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/timestamp.h"

namespace f2fs {

static constexpr time_t SecInNs = ZX_SEC(1);

static timespec AddNs(const timespec &a, time_t ns) {
  timespec ret = a;
  if (ret.tv_nsec += ns; ret.tv_nsec >= SecInNs) {
    ++ret.tv_sec;
    ret.tv_nsec -= SecInNs;
  }
  return ret;
}

static bool operator==(const timespec &a, const timespec &b) {
  return a.tv_sec == b.tv_sec && a.tv_nsec == b.tv_nsec;
}

static bool operator<(const timespec &a, const timespec &b) {
  return a.tv_sec < b.tv_sec || ((a.tv_sec == b.tv_sec) && a.tv_nsec < b.tv_nsec);
}

Timestamps::Timestamps(UpdateMode mode, const timespec &atime, const timespec &btime,
                       const timespec &ctime, const timespec &mtime)
    : mode_(mode),
      access_time_(atime),
      birth_time_(btime),
      change_time_(ctime),
      modification_time_(mtime) {}

template <>
const timespec &Timestamps::Get<Timestamps::AccessTime>() const {
  return access_time_;
}

template <>
const timespec &Timestamps::Get<Timestamps::ModificationTime>() const {
  return modification_time_;
}

template <>
const timespec &Timestamps::Get<Timestamps::ChangeTime>() const {
  return change_time_;
}

template <>
const timespec &Timestamps::Get<Timestamps::BirthTime>() const {
  return birth_time_;
}

template <>
void Timestamps::Update<Timestamps::ModificationTime>() {
  timespec cur_time;
  clock_gettime(CLOCK_REALTIME, &cur_time);
  if (cur_time < AddNs(modification_time_, modification_time_interval_)) {
    return;
  }
  access_time_ = change_time_ = modification_time_ = cur_time;
  dirty_ = true;
}

template <>
void Timestamps::Update<Timestamps::BirthTime>() {
  timespec cur_time;
  clock_gettime(CLOCK_REALTIME, &cur_time);
  if (birth_time_ == cur_time) {
    return;
  }
  birth_time_ = change_time_ = cur_time;
  dirty_ = true;
}

template <>
void Timestamps::Update<Timestamps::ChangeTime>() {
  timespec cur_time;
  clock_gettime(CLOCK_REALTIME, &cur_time);
  if (change_time_ == cur_time) {
    return;
  }
  change_time_ = cur_time;
  dirty_ = true;
}

template <>
void Timestamps::Update<Timestamps::AccessTime>() {
  if (mode_ == UpdateMode::kNoAccess) {
    return;
  }

  timespec cur_time;
  clock_gettime(CLOCK_REALTIME, &cur_time);
  if (mode_ == UpdateMode::kStrict && access_time_ == cur_time) {
    return;
  }
  if (mode_ == UpdateMode::kRelative) {
    if (access_time_interval_ && cur_time.tv_sec < access_time_.tv_sec + access_time_interval_) {
      return;
    }
    if (!access_time_interval_ && access_time_ == cur_time) {
      return;
    }
  }

  access_time_ = cur_time;
  dirty_ = true;
}

template <>
void Timestamps::Update<Timestamps::AccessTime>(const timespec &t) {
  dirty_ = !(access_time_ == t);
  access_time_ = change_time_ = t;
}

template <>
void Timestamps::Update<Timestamps::BirthTime>(const timespec &t) {
  dirty_ = !(birth_time_ == t);
  birth_time_ = change_time_ = t;
}

template <>
void Timestamps::Update<Timestamps::ChangeTime>(const timespec &t) {
  dirty_ = !(change_time_ == t);
  change_time_ = t;
}

template <>
void Timestamps::Update<Timestamps::ModificationTime>(const timespec &t) {
  dirty_ = !(modification_time_ == t);
  access_time_ = change_time_ = modification_time_ = t;
}

}  // namespace f2fs
