// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_MKFS_H_
#define SRC_STORAGE_F2FS_MKFS_H_

#include <utility>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/layout.h"

namespace f2fs {

static const char* kMediaExtList[] = {"jpg", "gif", "png",  "avi", "divx", "mp4", "mp3", "3gp",
                                      "wmv", "wma", "mpeg", "mkv", "mov",  "asx", "asf", "wmx",
                                      "svi", "wvx", "wm",   "mpg", "mpe",  "rm",  "ogg"};

struct GlobalParameters {
  uint32_t sector_size = 0;
  uint32_t reserved_segments = 0;
  uint32_t op_ratio = 0;
  uint32_t op_segments = 0;
  uint32_t cur_seg[6];
  uint32_t segs_per_sec = 0;
  uint32_t secs_per_zone = 0;
  uint32_t start_sector = 0;
  uint64_t total_sectors = 0;
  uint32_t sectors_per_blk = 0;
  uint32_t blks_per_seg = 0;
  uint8_t vol_label[kVolumeLabelLength] = {
      0,
  };
  int32_t fd = 0;
  char* device_name = nullptr;
  std::vector<std::string> extension_list;
};

struct MkfsOptions {
  std::string label;
  uint32_t overprovision_ratio = 0;
  uint32_t segs_per_sec = 1;
  uint32_t secs_per_zone = 1;
  std::vector<std::string> extension_list;
};

class BcacheMapper;

class MkfsWorker {
 public:
  explicit MkfsWorker(std::unique_ptr<BcacheMapper> bc, MkfsOptions options);

  // Not copyable or moveable
  MkfsWorker(const MkfsWorker&) = delete;
  MkfsWorker& operator=(const MkfsWorker&) = delete;
  MkfsWorker(MkfsWorker&&) = delete;
  MkfsWorker& operator=(MkfsWorker&&) = delete;

  zx::result<std::unique_ptr<BcacheMapper>> DoMkfs();

 private:
  friend class MkfsTester;
  std::unique_ptr<BcacheMapper> bc_;
  MkfsOptions mkfs_options_{};

  // F2FS Parameter
  GlobalParameters params_;
  Superblock super_block_;

  void InitGlobalParameters();
  zx_status_t GetDeviceInfo();
  zx_status_t FormatDevice();

  void ConfigureExtensionList();

  zx_status_t WriteToDisk(void* buf, block_t bno);

  zx::result<uint32_t> SetSpace();

  zx_status_t PrepareSuperblock();
  zx_status_t InitSitArea();
  zx_status_t InitNatArea();

  zx_status_t WriteCheckPointPack();
  zx_status_t WriteSuperblock();
  zx_status_t WriteRootInode();
  zx_status_t UpdateNatRoot();
  zx_status_t AddDefaultDentryRoot();
  zx_status_t CreateRootDir();
  zx_status_t PurgeNodeChain();
};

inline MkfsWorker::MkfsWorker(std::unique_ptr<BcacheMapper> bc, MkfsOptions options)
    : bc_(std::move(bc)), mkfs_options_(std::move(options)) {}

zx_status_t ParseOptions(const MkfsOptions& options);

zx::result<std::unique_ptr<BcacheMapper>> Mkfs(const MkfsOptions& options,
                                               std::unique_ptr<BcacheMapper> bc);

void AsciiToUnicode(std::string_view in_string, std::u16string& out_string);

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_MKFS_H_
