// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <string.h>

#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

using block_client::FakeBlockDevice;

const MkfsOptions default_option;

void DoMkfs(std::unique_ptr<BcacheMapper> bcache, const MkfsOptions &options, bool expect_success,
            std::unique_ptr<BcacheMapper> *out) {
  zx_status_t status = ZX_ERR_INVALID_ARGS;
  zx::result<std::unique_ptr<BcacheMapper>> make_return;

  if (status = ParseOptions(options); status == ZX_OK) {
    make_return = Mkfs(options, std::move(bcache));
    status = make_return.status_value();
  }

  if (expect_success) {
    ASSERT_EQ(status, ZX_OK);
    *out = std::move(*make_return);
  } else {
    ASSERT_NE(status, ZX_OK);
    *out = nullptr;
  }
}

void ReadSuperblock(BcacheMapper &bc, std::unique_ptr<Superblock> *out) {
  auto sb_or = LoadSuperblock(bc);
  ASSERT_TRUE(sb_or.is_ok());
  *out = std::move(*sb_or);
}

zx::result<std::unique_ptr<Superblock>> ReadSuperblock(BcacheMapper &bc) {
  std::unique_ptr<Superblock> sb;
  ReadSuperblock(bc, &sb);
  return zx::ok(std::move(sb));
}

void ReadCheckpoint(BcacheMapper *bc, Superblock &sb, Checkpoint *ckp) {
  char buf[4096];
  ASSERT_EQ(bc->Readblk(sb.segment0_blkaddr, buf), ZX_OK);
  memcpy(ckp, buf, sizeof(Checkpoint));
}

void VerifyLabel(Superblock &sb, const std::string &vol_label) {
  std::u16string volume_name;
  AsciiToUnicode(vol_label, volume_name);

  uint16_t volume_name_arr[512];
  volume_name.copy(reinterpret_cast<char16_t *>(volume_name_arr), vol_label.size());
  volume_name_arr[vol_label.size()] = '\0';
  ASSERT_EQ(memcmp(sb.volume_name, volume_name_arr, vol_label.size() + 1), 0);
}

void VerifySegsPerSec(Superblock &sb, uint32_t segs_per_sec) {
  ASSERT_EQ(sb.segs_per_sec, segs_per_sec);
}

void VerifySecsPerZone(Superblock &sb, uint32_t secs_per_zone) {
  ASSERT_EQ(sb.secs_per_zone, secs_per_zone);
}

void VerifyExtensionList(Superblock &sb, const std::string &extensions) {
  ASSERT_LE(sb.extension_count, static_cast<uint32_t>(kMaxExtension));
  uint32_t extension_iter = 0;
  for (const char *ext_item : kMediaExtList) {
    ASSERT_LT(extension_iter, sb.extension_count);
    ASSERT_EQ(strcmp(reinterpret_cast<char *>(sb.extension_list[extension_iter++]), ext_item), 0);
  }

  ASSERT_EQ(extension_iter, sizeof(kMediaExtList) / sizeof(const char *));

  std::istringstream iss(extensions);
  std::string token;

  while (std::getline(iss, token, ',')) {
    ASSERT_LE(extension_iter, sb.extension_count);
    ASSERT_EQ(strcmp(reinterpret_cast<char *>(sb.extension_list[extension_iter++]), token.c_str()),
              0);
    if (extension_iter >= kMaxExtension)
      break;
  }

  ASSERT_EQ(extension_iter, sb.extension_count);
}

void VerifyInitialLocationOfSegments(Superblock &sb, Checkpoint &ckp) {
  uint32_t total_zones = LeToCpu(sb.segment_count_main) / sb.segs_per_sec / sb.secs_per_zone;
  ASSERT_GT(total_zones, static_cast<uint32_t>(6));

  uint32_t cur_seg[6];
  cur_seg[static_cast<int>(CursegType::kCursegHotNode)] = 0;
  cur_seg[static_cast<int>(CursegType::kCursegWarmNode)] =
      cur_seg[static_cast<int>(CursegType::kCursegHotNode)] + sb.segs_per_sec * sb.secs_per_zone;
  cur_seg[static_cast<int>(CursegType::kCursegColdNode)] =
      cur_seg[static_cast<int>(CursegType::kCursegWarmNode)] + sb.segs_per_sec * sb.secs_per_zone;
  cur_seg[static_cast<int>(CursegType::kCursegHotData)] =
      cur_seg[static_cast<int>(CursegType::kCursegColdNode)] + sb.segs_per_sec * sb.secs_per_zone;
  cur_seg[static_cast<int>(CursegType::kCursegColdData)] =
      cur_seg[static_cast<int>(CursegType::kCursegHotData)] + sb.segs_per_sec * sb.secs_per_zone;
  cur_seg[static_cast<int>(CursegType::kCursegWarmData)] =
      cur_seg[static_cast<int>(CursegType::kCursegColdData)] + sb.segs_per_sec * sb.secs_per_zone;

  ASSERT_EQ(ckp.cur_node_segno[0], cur_seg[static_cast<int>(CursegType::kCursegHotNode)]);
  ASSERT_EQ(ckp.cur_node_segno[1], cur_seg[static_cast<int>(CursegType::kCursegWarmNode)]);
  ASSERT_EQ(ckp.cur_node_segno[2], cur_seg[static_cast<int>(CursegType::kCursegColdNode)]);
  ASSERT_EQ(ckp.cur_data_segno[0], cur_seg[static_cast<int>(CursegType::kCursegHotData)]);
  ASSERT_EQ(ckp.cur_data_segno[1], cur_seg[static_cast<int>(CursegType::kCursegWarmData)]);
  ASSERT_EQ(ckp.cur_data_segno[2], cur_seg[static_cast<int>(CursegType::kCursegColdData)]);
}

void VerifyOP(Superblock &sb, Checkpoint &ckp, uint32_t op_ratio) {
  size_t user_segments = LeToCpu(sb.segment_count_main) - LeToCpu(ckp.rsvd_segment_count);
  size_t maximum_op_segments =
      std::max(1UL, CheckedDivRoundUp(user_segments / LeToCpu(sb.segs_per_sec) * op_ratio, 100UL)) *
      LeToCpu(sb.segs_per_sec);
  ASSERT_GE(maximum_op_segments + LeToCpu(ckp.rsvd_segment_count),
            LeToCpu(ckp.overprov_segment_count));
}

TEST(FormatFilesystemTest, MkfsOptionsLabel) {
  auto device = std::make_unique<FakeBlockDevice>(kDefaultSectorCount, kDefaultSectorSize);
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());

  char label[20];
  const char *default_label = "F2FS";

  // Check default label is written when there is no arg for label
  std::unique_ptr<BcacheMapper> bc;
  MkfsOptions options;
  DoMkfs(std::move(*bc_or), options, true, &bc);

  auto sb_or = ReadSuperblock(*bc);
  VerifyLabel(*sb_or.value(), default_label);

  // Try with max size label (16)

  memcpy(label, "0123456789abcde", 16);
  options.label = label;
  DoMkfs(std::move(bc), options, true, &bc);
  sb_or = ReadSuperblock(*bc);
  VerifyLabel(*sb_or.value(), label);

  // Check failure with label size larger than max size
  memcpy(label, "0123456789abcdef", 17);
  options.label = label;
  DoMkfs(std::move(bc), options, false, &bc);
}

TEST(FormatFilesystemTest, MkfsOptionsSegsPerSec) {
  auto device = std::make_unique<FakeBlockDevice>(kDefaultSectorCount * 2, kDefaultSectorSize);
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());

  // Check default value
  std::unique_ptr<BcacheMapper> bc;
  MkfsOptions options;
  DoMkfs(std::move(*bc_or), options, true, &bc);
  auto sb_or = ReadSuperblock(*bc);
  VerifySegsPerSec(*sb_or.value(), default_option.segs_per_sec);

  // Try with various values
  const uint32_t segs_per_sec_list[] = {1, 2};
  for (uint32_t segs_per_sec : segs_per_sec_list) {
    FX_LOGS(INFO) << "segs_per_sec = " << segs_per_sec;
    options.segs_per_sec = segs_per_sec;
    DoMkfs(std::move(bc), options, true, &bc);
    auto sb_or = ReadSuperblock(*bc);
    VerifySegsPerSec(*sb_or.value(), segs_per_sec);
  }

  // Check failure with zero
  options.segs_per_sec = 0;
  DoMkfs(std::move(bc), options, false, &bc);
}

TEST(FormatFilesystemTest, MkfsOptionsSecsPerZone) {
  auto device = std::make_unique<FakeBlockDevice>(kDefaultSectorCount * 2, kDefaultSectorSize);
  std::unique_ptr<BcacheMapper> bc;
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());

  // Check default value
  MkfsOptions options;
  DoMkfs(std::move(*bc_or), options, true, &bc);
  auto sb_or = ReadSuperblock(*bc);
  VerifySecsPerZone(*sb_or.value(), default_option.secs_per_zone);

  // Try with various values
  const uint32_t secs_per_zone_list[] = {1, 2};
  for (uint32_t secs_per_zone : secs_per_zone_list) {
    FX_LOGS(INFO) << "secs_per_zone = " << secs_per_zone;
    options.secs_per_zone = secs_per_zone;
    DoMkfs(std::move(bc), options, true, &bc);
    auto sb_or = ReadSuperblock(*bc);
    VerifySecsPerZone(*sb_or.value(), secs_per_zone);
  }

  // Check failure with zero
  options.secs_per_zone = 0;
  DoMkfs(std::move(bc), options, false, &bc);
}

TEST(FormatFilesystemTest, MkfsOptionsExtensions) {
  auto device = std::make_unique<FakeBlockDevice>(kDefaultSectorCount, kDefaultSectorSize);
  std::unique_ptr<BcacheMapper> bc;
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());

  // Check default
  MkfsOptions options;
  DoMkfs(std::move(*bc_or), options, true, &bc);
  auto sb_or = ReadSuperblock(*bc);
  VerifyExtensionList(*sb_or.value(), "");

  // Try with max extension counts
  std::string extensions;
  for (uint32_t i = sizeof(kMediaExtList) / sizeof(const char *); i < kMaxExtension; ++i) {
    if (i > sizeof(kMediaExtList) / sizeof(const char *))
      extensions.append(",");
    extensions.append(std::to_string(i));
    options.extension_list.push_back(std::to_string(i));
  }

  DoMkfs(std::move(bc), options, true, &bc);
  sb_or = ReadSuperblock(*bc);
  VerifyExtensionList(*sb_or.value(), extensions);

  // If exceeding max extension counts, only extensions within max count are valid
  extensions.append(",foo");
  options.extension_list.push_back("foo");

  DoMkfs(std::move(bc), options, true, &bc);
  sb_or = ReadSuperblock(*bc);
  VerifyExtensionList(*sb_or.value(), extensions);
}

TEST(FormatFilesystemTest, MkfsOptionsOverprovision) {
  auto device = std::make_unique<FakeBlockDevice>(kDefaultSectorCount, kDefaultSectorSize);
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());

  // Check default
  std::unique_ptr<BcacheMapper> bc;
  MkfsOptions options;
  DoMkfs(std::move(*bc_or), options, true, &bc);
  auto sb_or = ReadSuperblock(*bc);
  Checkpoint ckp = {};
  ReadCheckpoint(bc.get(), *sb_or.value(), &ckp);

  // Try with various values
  const uint32_t overprovision_ratio_list[] = {3, 5, 7};
  for (uint32_t overprovision_ratio : overprovision_ratio_list) {
    FX_LOGS(INFO) << "overprovision_ratio = " << overprovision_ratio;
    options.overprovision_ratio = overprovision_ratio;
    DoMkfs(std::move(bc), options, true, &bc);
    ReadCheckpoint(bc.get(), *sb_or.value(), &ckp);
    VerifyOP(*sb_or.value(), ckp, overprovision_ratio);
  }
}

TEST(FormatFilesystemTest, MkfsOptionsMixed) {
  auto device = std::make_unique<FakeBlockDevice>(kDefaultSectorCount * 4, kDefaultSectorSize);
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());
  std::unique_ptr<BcacheMapper> bc = std::move(*bc_or);

  const char *label_list[] = {"aa", "bbbbb"};
  const uint32_t segs_per_sec_list[] = {1, 2};
  const uint32_t secs_per_zone_list[] = {1, 2};
  const char *ext_list[] = {"foo", "foo,bar"};
  const uint32_t overprovision_list[] = {7, 9};

  for (const char *label : label_list) {
    for (const uint32_t segs_per_sec : segs_per_sec_list) {
      for (const uint32_t secs_per_zone : secs_per_zone_list) {
        for (const char *extensions : ext_list) {
          for (const uint32_t overprovision : overprovision_list) {
            MkfsOptions options;
            options.label = label;
            options.segs_per_sec = segs_per_sec;
            options.secs_per_zone = secs_per_zone;
            options.overprovision_ratio = overprovision;

            std::string buff(extensions);
            char *ue = strtok(buff.data(), ",");
            while (ue != nullptr) {
              options.extension_list.push_back(ue);
              ue = strtok(nullptr, ",");
            }

            DoMkfs(std::move(bc), options, true, &bc);

            std::unique_ptr<Superblock> sb;
            ReadSuperblock(*bc, &sb);

            Checkpoint ckp = {};
            ReadCheckpoint(bc.get(), *sb, &ckp);

            VerifyLabel(*sb, label);
            VerifySegsPerSec(*sb, segs_per_sec);
            VerifySecsPerZone(*sb, secs_per_zone);
            VerifyExtensionList(*sb, extensions);
            VerifyInitialLocationOfSegments(*sb, ckp);
            VerifyOP(*sb, ckp, overprovision);
          }
        }
      }
    }
  }
}

TEST(FormatFilesystemTest, BlockSize) {
  uint32_t block_size_array[] = {256, 512, 1024, 2048, 4096, 8192};
  uint32_t total_size = kDefaultSectorCount * 2 * kDefaultSectorSize;

  for (uint32_t block_size : block_size_array) {
    uint32_t block_count = total_size / block_size;
    MkfsOptions mkfs_options;
    auto device = std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
        .block_count = block_count, .block_size = block_size, .supports_trim = true});
    auto bc_or = CreateBcacheMapper(std::move(device), true);
    if (block_size > 1 << kMaxLogSectorSize || block_size < 1 << kMinLogSectorSize) {
      ASSERT_TRUE(bc_or.is_error());
      ASSERT_EQ(bc_or.error_value(), ZX_ERR_INVALID_ARGS);
    } else {
      ASSERT_TRUE(bc_or.is_ok());

      MkfsWorker mkfs(std::move(*bc_or), mkfs_options);
      auto ret = mkfs.DoMkfs();
      ASSERT_EQ(ret.is_error(), false);
      auto bc = std::move(*ret);

      std::unique_ptr<F2fs> fs;
      MountOptions options{};
      async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
      FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

      fbl::RefPtr<VnodeF2fs> root;
      FileTester::CreateRoot(fs.get(), &root);
      fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

      ASSERT_EQ(uint32_t(1) << LeToCpu(fs->GetSuperblockInfo().GetSuperblock().log_sectorsize),
                block_size);
      ASSERT_EQ(uint32_t(1) << fs->GetSuperblockInfo().GetLogSectorsPerBlock(),
                (uint32_t(1) << kMaxLogSectorSize) / block_size);

      ASSERT_EQ(root_dir->Close(), ZX_OK);
      root_dir.reset();

      FileTester::Unmount(std::move(fs), &bc);
      EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}, &bc), ZX_OK);
    }
  }
}

TEST(FormatFilesystemTest, MkfsSmallVolume) {
  uint32_t volume_segments[] = {kMinVolumeSegments, kMinVolumeSegments - 1};

  for (uint32_t segments : volume_segments) {
    uint64_t block_count = segments * kDefaultBlocksPerSegment;

    auto device = std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
        .block_count = block_count, .block_size = kBlockSize, .supports_trim = true});
    auto bc_or = CreateBcacheMapper(std::move(device), true);
    if (segments >= kMinVolumeSegments) {
      ASSERT_TRUE(bc_or.is_ok());

      MkfsOptions mkfs_options;
      MkfsWorker mkfs(std::move(*bc_or), mkfs_options);
      auto ret = mkfs.DoMkfs();
      ASSERT_TRUE(ret.is_ok());
      auto bc = std::move(*ret);

      std::unique_ptr<F2fs> fs;
      MountOptions options{};
      async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
      FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

      const Superblock &sb = fs->GetSuperblockInfo().GetSuperblock();
      ASSERT_EQ(LeToCpu(sb.segment_count_main), static_cast<uint32_t>(segments - kMinMetaSegments));

      FileTester::Unmount(std::move(fs), &bc);
      EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}), ZX_OK);
    } else {
      ASSERT_TRUE(bc_or.is_error());
      ASSERT_EQ(bc_or.status_value(), ZX_ERR_NO_SPACE);
    }
  }
}

TEST(FormatFilesystemTest, PrepareSuperblockExceptionCase) {
  MkfsOptions mkfs_options;

  auto device = std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
      .block_count = kDefaultSectorCount, .block_size = kDefaultSectorSize, .supports_trim = true});

  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());

  MkfsWorker mkfs(std::move(*bc_or), mkfs_options);

  // Check invalid sector_size value
  ASSERT_EQ(MkfsTester::InitAndGetDeviceInfo(mkfs), ZX_OK);
  GlobalParameters &params = MkfsTester::GetGlobalParameters(mkfs);
  params.sector_size = kMinLogSectorSize / 2;
  auto ret = MkfsTester::FormatDevice(mkfs);
  ASSERT_TRUE(ret.is_error());

  // Check invalid sectors_per_blk value
  ASSERT_EQ(MkfsTester::InitAndGetDeviceInfo(mkfs), ZX_OK);
  params = MkfsTester::GetGlobalParameters(mkfs);
  params.sectors_per_blk = kDefaultSectorsPerBlock * 2;
  ret = MkfsTester::FormatDevice(mkfs);
  ASSERT_EQ(ret.is_error(), true);

  // Check invalid blks_per_seg value
  ASSERT_EQ(MkfsTester::InitAndGetDeviceInfo(mkfs), ZX_OK);
  params = MkfsTester::GetGlobalParameters(mkfs);
  params.blks_per_seg = kDefaultBlocksPerSegment * 2;
  ret = MkfsTester::FormatDevice(mkfs);
  ASSERT_EQ(ret.is_error(), true);

  // Check unaligned start_sector
  ASSERT_EQ(MkfsTester::InitAndGetDeviceInfo(mkfs), ZX_OK);
  params = MkfsTester::GetGlobalParameters(mkfs);
  params.start_sector = 1;
  ret = MkfsTester::FormatDevice(mkfs);
  ASSERT_EQ(ret.is_error(), false);
}

TEST(FormatFilesystemTest, LabelAsciiToUnicodeConversion) {
  // Convert empty string
  std::string label;
  std::u16string out;
  std::u16string target;

  AsciiToUnicode(label, out);
  ASSERT_EQ(out, target);

  // Convert non-empty string
  label = "alphabravo";
  target = u"alphabravo";

  AsciiToUnicode(label, out);
  ASSERT_EQ(out, target);
}

TEST(FormatFilesystemTest, DeviceFailure) {
  constexpr uint32_t kBlockCount = kDefaultSectorCount / kDefaultSectorsPerBlock;

  auto device = std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
      .block_count = kBlockCount, .block_size = kBlockSize, .supports_trim = true});
  auto bc_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(bc_or.is_ok());

  uint32_t bad_block = std::numeric_limits<uint32_t>::max();
  bool trim_error = false;
  // Set a hook to trigger an io error which causes mkfs fail.
  auto hook = [&bad_block, &trim_error](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.command.opcode == BLOCK_OPCODE_TRIM && trim_error) {
      return ZX_ERR_IO_REFUSED;
    }
    if (_req.command.opcode == BLOCK_OPCODE_WRITE && _req.dev_offset == bad_block) {
      return ZX_ERR_IO;
    }
    return ZX_OK;
  };

  bc_or->ForEachBcache([](Bcache *f2fs_device) {
    static_cast<block_client::FakeBlockDevice *>(f2fs_device->GetDevice())->Pause();
  });
  bc_or->ForEachBcache([hook](Bcache *f2fs_device) {
    static_cast<block_client::FakeBlockDevice *>(f2fs_device->GetDevice())->set_hook(hook);
  });
  bc_or->ForEachBcache([](Bcache *f2fs_device) {
    static_cast<block_client::FakeBlockDevice *>(f2fs_device->GetDevice())->Resume();
  });

  MkfsOptions mkfs_options;
  MkfsWorker mkfs(std::move(*bc_or), mkfs_options);

  // Trim Error
  {
    trim_error = true;
    auto ret = mkfs.DoMkfs();
    ASSERT_TRUE(ret.is_error());
    ASSERT_EQ(ret.error_value(), ZX_ERR_IO_REFUSED);
    trim_error = false;
  }

  constexpr uint32_t kSitDevOff = 1536;
  constexpr uint32_t kNatDevOff = 2560;
  constexpr uint32_t kChkpDevOff = 512;
  constexpr uint32_t kSuperblockDevOff = 0;
  std::vector<uint32_t> bad_blocks = {kSitDevOff, kNatDevOff, kChkpDevOff, kSuperblockDevOff};

  // Write Error
  for (auto bad_block_off : bad_blocks) {
    bad_block = bad_block_off;
    auto ret = mkfs.DoMkfs();
    ASSERT_TRUE(ret.is_error());
    ASSERT_EQ(ret.error_value(), ZX_ERR_IO);
  }
}

}  // namespace
}  // namespace f2fs
