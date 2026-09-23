// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/fs_management/cpp/options.h"

#include <gtest/gtest.h>

#include "fidl/fuchsia.fs.startup/cpp/wire_types.h"
#include "lib/fidl/cpp/wire/arena.h"

namespace fs_management {

namespace {

void AssertStartOptionsEqual(const fuchsia_fs_startup::wire::StartOptions& a,
                             const fuchsia_fs_startup::wire::StartOptions& b) {
  ASSERT_EQ(a.has_read_only(), b.has_read_only());
  if (a.has_read_only()) {
    ASSERT_EQ(a.read_only(), b.read_only());
  }
  ASSERT_EQ(a.has_verbose(), b.has_verbose());
  if (a.has_verbose()) {
    ASSERT_EQ(a.verbose(), b.verbose());
  }
  ASSERT_EQ(a.has_startup_profiling_seconds(), b.has_startup_profiling_seconds());
  if (a.has_startup_profiling_seconds()) {
    ASSERT_EQ(a.startup_profiling_seconds(), b.startup_profiling_seconds());
  }
  ASSERT_EQ(a.has_inline_crypto_enabled(), b.has_inline_crypto_enabled());
  if (a.has_inline_crypto_enabled()) {
    ASSERT_EQ(a.inline_crypto_enabled(), b.inline_crypto_enabled());
  }
  ASSERT_EQ(a.has_barriers_enabled(), b.has_barriers_enabled());
  if (a.has_barriers_enabled()) {
    ASSERT_EQ(a.barriers_enabled(), b.barriers_enabled());
  }
  ASSERT_EQ(a.has_allow_type3_blobs(), b.has_allow_type3_blobs());
  if (a.has_allow_type3_blobs()) {
    ASSERT_EQ(a.allow_type3_blobs(), b.allow_type3_blobs());
  }
}

void AssertFormatOptionsEqual(const fuchsia_fs_startup::wire::FormatOptions& a,
                              const fuchsia_fs_startup::wire::FormatOptions& b) {
  ASSERT_EQ(a.has_verbose(), b.has_verbose());
  if (a.has_verbose())
    ASSERT_EQ(a.verbose(), b.verbose());
  ASSERT_EQ(a.has_num_inodes(), b.has_num_inodes());
  if (a.has_num_inodes())
    ASSERT_EQ(a.num_inodes(), b.num_inodes());
  ASSERT_EQ(a.has_deprecated_padded_blobfs_format(), b.has_deprecated_padded_blobfs_format());
  if (a.has_deprecated_padded_blobfs_format())
    ASSERT_EQ(a.deprecated_padded_blobfs_format(), b.deprecated_padded_blobfs_format());
  ASSERT_EQ(a.has_fvm_data_slices(), b.has_fvm_data_slices());
  if (a.has_fvm_data_slices())
    ASSERT_EQ(a.fvm_data_slices(), b.fvm_data_slices());
  ASSERT_EQ(a.has_sectors_per_cluster(), b.has_sectors_per_cluster());
  if (a.has_sectors_per_cluster())
    ASSERT_EQ(a.sectors_per_cluster(), b.sectors_per_cluster());
}

TEST(MountOptionsTest, DefaultOptions) {
  MountOptions options;
  fidl::Arena arena;
  auto builder = fuchsia_fs_startup::wire::StartOptions::Builder(arena);
  // This is the default, but we explicitly enumerate it here to be clear that it's the default.
  builder.read_only(false);
  builder.verbose(false);
  fuchsia_fs_startup::wire::StartOptions expected_start_options = builder.Build();

  auto start_options_or = options.as_start_options(arena);
  ASSERT_TRUE(start_options_or.is_ok()) << start_options_or.status_string();
  AssertStartOptionsEqual(*start_options_or, expected_start_options);
}

TEST(MountOptionsTest, AllOptionsSet) {
  MountOptions options{
      .readonly = true,
      .verbose_mount = true,
      .fsck_after_every_transaction = true,
      .startup_profiling_seconds = 5,
      .inline_crypto_enabled = true,
      .barriers_enabled = true,
  };
  fidl::Arena arena;
  auto builder = fuchsia_fs_startup::wire::StartOptions::Builder(arena);
  builder.read_only(true);
  builder.verbose(true);
  builder.startup_profiling_seconds(5);
  builder.inline_crypto_enabled(true);
  builder.barriers_enabled(true);
  fuchsia_fs_startup::wire::StartOptions expected_start_options = builder.Build();

  auto start_options_or = options.as_start_options(arena);
  ASSERT_TRUE(start_options_or.is_ok()) << start_options_or.status_string();
  AssertStartOptionsEqual(*start_options_or, expected_start_options);
}

TEST(MkfsOptionsTest, DefaultOptions) {
  MkfsOptions options;
  fidl::Arena arena;
  auto expected_format_options = fuchsia_fs_startup::wire::FormatOptions::Builder(arena)
                                     .verbose(false)
                                     .deprecated_padded_blobfs_format(false)
                                     .fvm_data_slices(1)
                                     .Build();

  AssertFormatOptionsEqual(options.as_format_options(arena), expected_format_options);
}

TEST(MkfsOptionsTest, AllOptionsSet) {
  MkfsOptions options{
      .fvm_data_slices = 10,
      .verbose = true,
      .sectors_per_cluster = 2,
      .deprecated_padded_blobfs_format = true,
      .num_inodes = 100,
  };
  fidl::Arena arena;
  auto expected_format_options = fuchsia_fs_startup::wire::FormatOptions::Builder(arena)
                                     .fvm_data_slices(10)
                                     .verbose(true)
                                     .deprecated_padded_blobfs_format(true)
                                     .num_inodes(100)
                                     .sectors_per_cluster(2)
                                     .Build();

  AssertFormatOptionsEqual(options.as_format_options(arena), expected_format_options);
}

}  // namespace
}  // namespace fs_management
