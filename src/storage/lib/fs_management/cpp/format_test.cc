// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/fs_management/cpp/format.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/vmo.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <ramdevice-client/ramdisk.h>

namespace fs_management {
namespace {

constexpr uint32_t kBlockSize = 4096;

constexpr uint8_t kGptMagic[] = {0x45, 0x46, 0x49, 0x20, 0x50, 0x41, 0x52, 0x54,
                                 0x00, 0x00, 0x01, 0x00, 0x5c, 0x00, 0x00, 0x00};

TEST(FormatDetectionTest, TestInvalidGptIgnored) {
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(2 * kBlockSize, 0, &vmo), ZX_OK);
  zx::result ramdisk = ramdevice_client::Ramdisk::CreateWithVmo(std::move(vmo), kBlockSize);
  ASSERT_EQ(ramdisk.status_value(), ZX_OK);
  zx::result block = ramdisk->ConnectBlock();
  ASSERT_TRUE(block.is_ok()) << block.status_string();
  ASSERT_EQ(DetectDiskFormat(fidl::UnownedClientEnd<fuchsia_storage_block::Block>(
                block.value().channel().get())),
            kDiskFormatUnknown);
}

TEST(FormatDetectionTest, TestGptWithUnusualBlockSize) {
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(2 * kBlockSize, 0, &vmo), ZX_OK);
  vmo.write(kGptMagic, kBlockSize, sizeof(kGptMagic));
  zx::result ramdisk = ramdevice_client::Ramdisk::CreateWithVmo(std::move(vmo), kBlockSize);
  ASSERT_EQ(ramdisk.status_value(), ZX_OK);
  zx::result block = ramdisk->ConnectBlock();
  ASSERT_TRUE(block.is_ok()) << block.status_string();
  ASSERT_EQ(DetectDiskFormat(fidl::UnownedClientEnd<fuchsia_storage_block::Block>(
                block.value().channel().get())),
            kDiskFormatGpt);
}

TEST(FormatDetectionTest, TestVbmetaRecognised) {
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(2 * kBlockSize, 0, &vmo), ZX_OK);

  // Write the vbmeta magic string at the start of the device.
  const unsigned char kVbmetaMagic[] = {'A', 'V', 'B', '0'};
  vmo.write(kVbmetaMagic, /*offset=*/0x0, sizeof(kVbmetaMagic));

  // Add the MBR magic string to the end of the first sector. These bytes in
  // vbmeta tend to be randomish, and previously we've had bugs where if these
  // bytes happened to match the MBR magic, we would misrecognise the partition.
  // (c.f. https://fxbug.dev/42137435)
  const unsigned char kMbrMagic[] = {0x55, 0xaa};
  vmo.write(kMbrMagic, /*offset=*/510, sizeof(kMbrMagic));

  zx::result ramdisk = ramdevice_client::Ramdisk::CreateWithVmo(std::move(vmo), kBlockSize);
  ASSERT_EQ(ramdisk.status_value(), ZX_OK);
  zx::result block = ramdisk->ConnectBlock();
  ASSERT_TRUE(block.is_ok()) << block.status_string();
  ASSERT_EQ(DetectDiskFormat(fidl::UnownedClientEnd<fuchsia_storage_block::Block>(
                block.value().channel().get())),
            kDiskFormatVbmeta);
}

}  // namespace
}  // namespace fs_management
