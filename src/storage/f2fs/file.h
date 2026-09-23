// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_FILE_H_
#define SRC_STORAGE_F2FS_FILE_H_

#include "src/storage/f2fs/vnode.h"

namespace f2fs {

class FileTester;
class File : public VnodeF2fs, public fbl::Recyclable<File> {
 public:
  // Constructor for files loaded from disk
  explicit File(F2fs* fs, ino_t ino, umode_t mode, LockedPage node_page);
  // Constructor for newly created files
  explicit File(F2fs* fs, ino_t ino, umode_t mode, std::optional<gid_t> gid = std::nullopt);
  virtual ~File() = default;

  // Required for memory management, see the class comment above Vnode for more.
  void fbl_recycle() { RecycleNode(); }

#if 0  // porting needed
  // int F2fsVmPageMkwrite(vm_area_struct* vma, vm_fault* vmf);
  // int F2fsFileMmap(/*file *file,*/ vm_area_struct* vma);
  // void FillZero(pgoff_t index, loff_t start, loff_t len);
  // int PunchHole(loff_t offset, loff_t len, int mode);
  // int ExpandInodeData(loff_t offset, off_t len, int mode);
  // long F2fsFallocate(int mode, loff_t offset, loff_t len);
  // uint32_t F2fsMaskFlags(umode_t mode, uint32_t flags);
  // long F2fsIoctl(/*file *filp,*/ unsigned int cmd, uint64_t arg);
#endif

  zx_status_t Truncate(size_t len) final __TA_EXCLUDES(mutex_);
  zx_status_t RecoverInlineData(NodePage& node_page) final;
  zx_status_t GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) final
      __TA_EXCLUDES(mutex_);
  void VmoDirty(uint64_t offset, uint64_t length) final
      __TA_EXCLUDES(mutex_, f2fs::GetGlobalLock());
  void VmoRead(uint64_t offset, uint64_t length) final __TA_EXCLUDES(mutex_);
  zx::result<zx::stream> CreateStream(uint32_t stream_options) final;
  block_t GetBlockAddr(LockedPage& page) final;
  zx_status_t ConvertInlineData() __TA_EXCLUDES(mutex_);
  zx::result<LockedPage> FindVictimPage(pgoff_t index) final;

 private:
  friend FileTester;
  zx_status_t ReadInline(void* data, size_t len, size_t off, size_t* out_actual);
  zx_status_t TruncateInline(size_t len) __TA_EXCLUDES(mutex_);
  // for tests
  zx_status_t WriteInline(const void* data, size_t len, size_t offset, size_t* out_actual);

  size_t MaxFileSize();
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_FILE_H_
