// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/timestamp.h"

#include <gtest/gtest.h>

namespace f2fs {
namespace {

static bool operator==(const timespec &a, const timespec &b) {
  return a.tv_sec == b.tv_sec && a.tv_nsec == b.tv_nsec;
}

TEST(TimeStamp, NoAccess) {
  timespec cur;
  clock_gettime(CLOCK_REALTIME, &cur);

  --cur.tv_sec;
  Timestamps stamp(UpdateMode::kNoAccess, cur, cur, cur, cur);
  ASSERT_EQ(stamp.IsDirty(), false);

  stamp.Update<Timestamps::AccessTime>();
  ASSERT_EQ(stamp.IsDirty(), false);
}

TEST(TimeStamp, Strict) {
  timespec cur;
  clock_gettime(CLOCK_REALTIME, &cur);

  --cur.tv_sec;
  Timestamps stamp(UpdateMode::kStrict, cur, cur, cur, cur);
  ASSERT_EQ(stamp.IsDirty(), false);

  stamp.Update<Timestamps::AccessTime>();
  ASSERT_EQ(stamp.IsDirty(), true);
}

TEST(TimeStamp, Relatime) {
  timespec cur;
  clock_gettime(CLOCK_REALTIME, &cur);

  cur.tv_sec -= Timestamps::kDayInSec + 1;

  Timestamps stamp(UpdateMode::kRelative, cur, cur, cur, cur);
  ASSERT_EQ(stamp.IsDirty(), false);

  stamp.SetAccessTimeInterval(Timestamps::kDayInSec * 2);
  stamp.Update<Timestamps::AccessTime>();
  ASSERT_EQ(stamp.IsDirty(), false);

  stamp.SetAccessTimeInterval();
  stamp.Update<Timestamps::AccessTime>();
  ASSERT_EQ(stamp.IsDirty(), true);
}

TEST(TimeStamp, UpdateBirthTime) {
  timespec cur;
  clock_gettime(CLOCK_REALTIME, &cur);

  --cur.tv_sec;

  Timestamps stamp(UpdateMode::kRelative, cur, cur, cur, cur);
  ASSERT_TRUE(cur == stamp.Get<Timestamps::BirthTime>());
  ASSERT_EQ(stamp.IsDirty(), false);

  stamp.Update<Timestamps::BirthTime>();
  ASSERT_EQ(stamp.IsDirty(), true);
  ASSERT_FALSE(cur == stamp.Get<Timestamps::BirthTime>());
  ASSERT_FALSE(cur == stamp.Get<Timestamps::ChangeTime>());

  stamp.ClearDirty();
  stamp.Update<Timestamps::BirthTime>(cur);
  ASSERT_EQ(stamp.IsDirty(), true);
  ASSERT_TRUE(cur == stamp.Get<Timestamps::BirthTime>());
  ASSERT_TRUE(cur == stamp.Get<Timestamps::ChangeTime>());

  stamp.ClearDirty();
  stamp.Update<Timestamps::BirthTime>(cur);
  ASSERT_EQ(stamp.IsDirty(), false);
}

TEST(TimeStamp, UpdateModificationTime) {
  timespec cur;
  clock_gettime(CLOCK_REALTIME, &cur);

  cur.tv_nsec -= 200;

  Timestamps stamp(UpdateMode::kRelative, cur, cur, cur, cur);
  ASSERT_TRUE(cur == stamp.Get<Timestamps::ModificationTime>());
  stamp.Update<Timestamps::ModificationTime>(cur);
  ASSERT_EQ(stamp.IsDirty(), false);

  stamp.Update<Timestamps::ModificationTime>();
  ASSERT_EQ(stamp.IsDirty(), true);
  ASSERT_FALSE(cur == stamp.Get<Timestamps::ModificationTime>());
  ASSERT_FALSE(cur == stamp.Get<Timestamps::ChangeTime>());

  stamp.ClearDirty();
  stamp.SetModificationTimeInterval(ZX_SEC(1));
  stamp.Update<Timestamps::ModificationTime>();
  ASSERT_EQ(stamp.IsDirty(), false);
}

}  // namespace
}  // namespace f2fs
