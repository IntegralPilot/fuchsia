// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/dir.h"

#include <dirent.h>
#include <sys/stat.h>

#include <safemath/checked_math.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vmo_manager.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
const unsigned char kFiletypeTable[static_cast<uint8_t>(FileType::kFtMax)] = {
    [static_cast<uint8_t>(FileType::kFtUnknown)] = DT_UNKNOWN,
    [static_cast<uint8_t>(FileType::kFtRegFile)] = DT_REG,
    [static_cast<uint8_t>(FileType::kFtDir)] = DT_DIR,
    [static_cast<uint8_t>(FileType::kFtChrdev)] = DT_CHR,
    [static_cast<uint8_t>(FileType::kFtBlkdev)] = DT_BLK,
    [static_cast<uint8_t>(FileType::kFtFifo)] = DT_FIFO,
    [static_cast<uint8_t>(FileType::kFtSock)] = DT_SOCK,
    [static_cast<uint8_t>(FileType::kFtSymlink)] = DT_LNK,
};

constexpr unsigned int kStatShift = 12;

const unsigned char kTypeByMode[S_IFMT >> kStatShift] = {
    [S_IFREG >> kStatShift] = static_cast<uint8_t>(FileType::kFtRegFile),
    [S_IFDIR >> kStatShift] = static_cast<uint8_t>(FileType::kFtDir),
    [S_IFCHR >> kStatShift] = static_cast<uint8_t>(FileType::kFtChrdev),
    [S_IFBLK >> kStatShift] = static_cast<uint8_t>(FileType::kFtBlkdev),
    [S_IFIFO >> kStatShift] = static_cast<uint8_t>(FileType::kFtFifo),
    [S_IFSOCK >> kStatShift] = static_cast<uint8_t>(FileType::kFtSock),
    [S_IFLNK >> kStatShift] = static_cast<uint8_t>(FileType::kFtSymlink),
};
#pragma GCC diagnostic pop

static inline uint32_t DirBuckets(uint32_t level, uint8_t dir_level) {
  if (level + dir_level < kMaxDirHashDepth / 2) {
    return 1 << (level + dir_level);
  }
  return 1 << ((kMaxDirHashDepth / 2) - 1);
}

static inline uint32_t BucketBlocks(uint32_t level) {
  if (level < kMaxDirHashDepth / 2) {
    return 2;
  }
  return 4;
}

void SetDirEntryType(DirEntry &de, VnodeF2fs &vnode) {
  de.file_type = kTypeByMode[(vnode.GetMode() & S_IFMT) >> kStatShift];
}

uint64_t DirBlockIndex(uint32_t level, uint8_t dir_level, uint32_t idx) {
  uint64_t bidx = 0;

  for (uint32_t i = 0; i < level; ++i) {
    bidx += safemath::checked_cast<uint64_t>(
        safemath::CheckMul(DirBuckets(i, dir_level), BucketBlocks(i)).ValueOrDie());
  }
  bidx +=
      safemath::checked_cast<uint64_t>(safemath::CheckMul(idx, BucketBlocks(level)).ValueOrDie());
  return bidx;
}

Dir::Dir(F2fs *fs, ino_t ino, umode_t mode, std::optional<gid_t> gid) : VnodeF2fs(fs, ino, mode) {
  if (gid) {
    SetGid(*gid);
    SetMode(mode | S_ISGID);
  }
}

Dir::Dir(F2fs *fs, ino_t ino, umode_t mode, LockedPage node_page)
    : VnodeF2fs(fs, ino, mode, std::move(node_page)) {}

fuchsia_io::NodeProtocolKinds Dir::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kDirectory;
}

DirEntry &Dir::GetDirEntry(const DentryInfo &info, fbl::RefPtr<Page> &page) {
  if (info.page_index == kCachedInlineDirEntryPageIndex) {
    ZX_DEBUG_ASSERT(TestFlag(InodeInfoFlag::kInlineDentry));
    return InlineDentryArray(page.get(), *this)[info.bit_pos];
  }
  ZX_DEBUG_ASSERT(!TestFlag(InodeInfoFlag::kInlineDentry));
  return page->GetAddress<DentryBlock>()->dentry[info.bit_pos];
}

size_t Dir::DirBlocks() { return CheckedDivRoundUp<uint64_t>(GetSize(), kBlockSize); }
bool Dir::EarlyMatchName(std::string_view name, f2fs_hash_t namehash, const DirEntry &de) {
  return LeToCpu(de.name_len) == name.length() && LeToCpu(de.hash_code) == namehash;
}

zx::result<DentryInfo> Dir::FindInBlock(fbl::RefPtr<Page> dentry_page, std::string_view name,
                                        f2fs_hash_t namehash) {
  DentryBlock *dentry_blk = dentry_page->GetAddress<DentryBlock>();
  auto bits = GetBitmap(dentry_page);
  ZX_DEBUG_ASSERT(bits.is_ok());
  size_t bit_pos = bits->FindNextBit(0);
  size_t n = 0;
  DentryInfo info;
  std::vector<nid_t> nids;
  while (bit_pos < kNrDentryInBlock) {
    const DirEntry &de = dentry_blk->dentry[bit_pos];
    nid_t ino = LeToCpu(de.ino);
    if (!info.ino && EarlyMatchName(name, namehash, de) &&
        !memcmp(dentry_blk->filename[bit_pos], name.data(), name.length())) {
      info = {ino, dentry_page->GetKey(), bit_pos};
    }
    if (info.ino) {
      nids.push_back(ino);
      GetDirEntryCache().UpdateDirEntry(
          std::string_view(reinterpret_cast<char *>(dentry_blk->filename[bit_pos]),
                           LeToCpu(de.name_len)),
          {ino, dentry_page->GetKey(), bit_pos});
      if (kDefaultNodeReadSize <= ++n) {
        break;
      }
    }
    ZX_DEBUG_ASSERT(de.name_len > 0);
    bit_pos = bits->FindNextBit(bit_pos + GetDentrySlots(LeToCpu(de.name_len)));
  }
  if (!info.ino) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  if (nids.size() > 1) {
    ZX_ASSERT(fs()->GetNodeManager().GetNodePages(nids).is_ok());
  }
  return zx::ok(info);
}

zx::result<DentryInfo> Dir::FindInLevel(unsigned int level, std::string_view name,
                                        f2fs_hash_t namehash, fbl::RefPtr<Page> *res_page) {
  unsigned int nbucket, nblock;
  uint64_t bidx, end_block;

  ZX_DEBUG_ASSERT(level <= kMaxDirHashDepth);

  nbucket = DirBuckets(level, GetDirLevel());
  nblock = BucketBlocks(level);

  bidx = DirBlockIndex(level, GetDirLevel(), namehash % nbucket);
  end_block = bidx + nblock;

  for (; bidx < end_block; ++bidx) {
    // no need to allocate new dentry pages to all the indices
    zx::result dentry_page = FindDataPage(bidx);
    if (dentry_page.is_error()) {
      continue;
    }
    if (auto info = FindInBlock((*dentry_page).CopyRefPtr(), name, namehash); info.is_ok()) {
      *res_page = (*dentry_page).release();
      return info;
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

// Find an entry in the specified directory with the wanted name.
// It returns the page where the entry was found and the entry info.
// Page is returned mapped and unlocked. Entry is guaranteed to be valid.
zx::result<DentryInfo> Dir::FindEntryOnDevice(std::string_view name, fbl::RefPtr<Page> *res_page) {
  DirHash current = {DentryHash(name), 0};
  if (TestFlag(InodeInfoFlag::kInlineDentry)) {
    return FindInInlineDir(name, res_page);
  }

  if (!DirBlocks()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  *res_page = nullptr;
  uint64_t max_depth = GetCurrentDepth();
  for (; current.level < max_depth; ++current.level) {
    if (auto info = FindInLevel(current.level, name, current.hash, res_page); info.is_ok()) {
      return info;
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

zx::result<DentryInfo> Dir::FindEntry(std::string_view name, fbl::RefPtr<Page> *res_page) {
  if (auto dentry_info = GetDirEntryCache().LookupDirEntry(name); dentry_info.is_ok()) {
    if (TestFlag(InodeInfoFlag::kInlineDentry)) {
      ZX_DEBUG_ASSERT(dentry_info->page_index == kCachedInlineDirEntryPageIndex);
      LockedPage ipage;
      if (zx_status_t ret = fs()->GetNodeManager().GetNodePage(Ino(), &ipage); ret != ZX_OK) {
        return zx::error(ZX_ERR_NOT_FOUND);
      }
      *res_page = ipage.release();
      return dentry_info;
    }
    if (dentry_info->page_index == kCachedInlineDirEntryPageIndex) {
      dentry_info->page_index = 0;
    }
    auto dentry_page = FindDataPage(dentry_info->page_index);
    ZX_ASSERT(dentry_page.is_ok());
    *res_page = (*dentry_page).release();
    return dentry_info;
  }
  return FindEntryOnDevice(name, res_page);
}

zx::result<DentryInfo> Dir::FindEntrySafe(std::string_view name, fbl::RefPtr<Page> *res_page) {
  std::lock_guard dir_lock(mutex_);
  return FindEntry(name, res_page);
}

zx::result<ino_t> Dir::LookUpEntries(std::string_view name) {
  auto dentry_info = GetDirEntryCache().LookupDirEntry(name);
  if (dentry_info.is_ok()) {
    return zx::ok(dentry_info->ino);
  }

  fbl::RefPtr<Page> page;
  auto entry = FindEntryOnDevice(name, &page);
  if (entry.is_error()) {
    return entry.take_error();
  }
  return zx::ok(entry->ino);
}

zx::result<DentryInfo> Dir::GetParentDentryInfo(fbl::RefPtr<Page> *out) {
  fs::SharedLock dir_lock(mutex_);
  LockedPage page;
  DentryInfo info = {kNullIno, 0, kParentBitPos};
  if (TestFlag(InodeInfoFlag::kInlineDentry)) {
    if (zx_status_t ret = fs()->GetNodeManager().GetNodePage(Ino(), &page); ret != ZX_OK) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    info.page_index = kCachedInlineDirEntryPageIndex;
  } else {
    if (GetLockedDataPage(0, &page) != ZX_OK) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
  }
  *out = page.release();
  info.ino = LeToCpu(GetDirEntry(info, *out).ino);
  return zx::ok(info);
}

void Dir::SetLink(const DentryInfo &info, fbl::RefPtr<Page> &page, VnodeF2fs *vnode) {
  {
    LockedPage page_lock(page);
    page_lock.WaitOnWriteback();
    DirEntry &de = GetDirEntry(info, page);
    de.ino = CpuToLe(vnode->Ino());
    SetDirEntryType(de, *vnode);
    // If |de| is an inline dentry, the inode block should be flushed.
    // Otherwise, it writes out the data block.
    page_lock.SetDirty();
    if (info.bit_pos != kParentBitPos && info.bit_pos != kCurrentBitPos) {
      GetDirEntryCache().UpdateDirEntry(vnode->GetNameView(),
                                        {vnode->GetKey(), info.page_index, info.bit_pos});
    }
    SetTime<Timestamps::ModificationTime>();
  }
  SetDirty();
}

void Dir::SetLinkSafe(const DentryInfo &info, fbl::RefPtr<Page> &page, VnodeF2fs *vnode) {
  std::lock_guard dir_lock(mutex_);
  SetLink(info, page, vnode);
}

zx_status_t Dir::InitInodeMetadata() {
  std::lock_guard lock(mutex_);
  if (zx_status_t ret = VnodeF2fs::InitInodeMetadataUnsafe(); ret != ZX_OK) {
    return ret;
  }
  if (TestFlag(InodeInfoFlag::kNewInode)) {
    if (zx_status_t ret = MakeEmpty(Ino()); ret != ZX_OK) {
      PurgeInodeBlock();
      return ret;
    }
  }
  return ZX_OK;
}

void Dir::UpdateParentMetadata(VnodeF2fs *vnode, uint64_t current_depth) {
  if (vnode->TestFlag(InodeInfoFlag::kNewInode)) {
    vnode->ClearFlag(InodeInfoFlag::kNewInode);
    if (vnode->IsDir()) {
      IncrementLinkUnsafe();
      SetFlag(InodeInfoFlag::kUpdateDir);
    }
  }

  vnode->SetParentNid(Ino());
  SetTime<Timestamps::ModificationTime>();

  if (GetCurrentDepth() != current_depth) {
    SetCurrentDepth(current_depth);
    SetFlag(InodeInfoFlag::kUpdateDir);
  }

  SetDirty();

  vnode->ClearFlag(InodeInfoFlag::kIncLink);
}

size_t Dir::RoomForFilename(const PageBitmap &bits, size_t slots) {
  size_t bit_start = 0;
  while (true) {
    size_t zero_start = bits.FindNextZeroBit(bit_start);
    if (zero_start >= kNrDentryInBlock)
      return kNrDentryInBlock;

    size_t zero_end = bits.FindNextBit(zero_start);
    if (zero_end - zero_start >= slots)
      return zero_start;

    bit_start = zero_end + 1;
    if (bit_start >= kNrDentryInBlock)
      return kNrDentryInBlock;
  }
}

zx_status_t Dir::AddLink(std::string_view name, VnodeF2fs *vnode) {
  auto set_dirty = fit::defer([&]() {
    if (TestFlag(InodeInfoFlag::kUpdateDir)) {
      ClearFlag(InodeInfoFlag::kUpdateDir);
      SetDirty();
    }
  });

  if (TestFlag(InodeInfoFlag::kInlineDentry)) {
    if (zx::result converted = AddInlineEntry(name, vnode); converted.is_error()) {
      return converted.error_value();
    } else if (!converted.value()) {
      return ZX_OK;
    }
  }

  uint32_t level = 0;
  f2fs_hash_t dentry_hash = DentryHash(name);
  uint16_t namelen = safemath::checked_cast<uint16_t>(name.length());
  size_t slots = GetDentrySlots(namelen);
  for (uint64_t current_depth = GetCurrentDepth(); current_depth < kMaxDirHashDepth; ++level) {
    // Increase the depth, if required
    if (level == current_depth) {
      ++current_depth;
    }

    uint32_t nbucket = DirBuckets(level, GetDirLevel());
    uint32_t nblock = BucketBlocks(level);
    uint64_t bidx = DirBlockIndex(level, GetDirLevel(), (dentry_hash % nbucket));

    for (uint64_t block = bidx; block <= (bidx + nblock - 1); ++block) {
      zx::result dentry_page = GetNewLockedPage(safemath::checked_cast<pgoff_t>(block));
      if (dentry_page.is_error()) {
        return dentry_page.error_value();
      }

      DentryBlock *dentry_blk = dentry_page->GetAddress<DentryBlock>();
      auto bits = GetBitmap((*dentry_page).CopyRefPtr());
      ZX_DEBUG_ASSERT(bits.is_ok());
      size_t bit_pos = RoomForFilename(*bits, slots);
      if (bit_pos >= kNrDentryInBlock)
        continue;

      if (zx_status_t status = vnode->InitInodeMetadata(); status != ZX_OK) {
        return status;
      }
      dentry_page->WaitOnWriteback();

      DirEntry &de = dentry_blk->dentry[bit_pos];
      de.hash_code = CpuToLe(dentry_hash);
      de.name_len = CpuToLe(namelen);
      std::memcpy(dentry_blk->filename[bit_pos], name.data(), namelen);
      de.ino = CpuToLe(vnode->Ino());
      SetDirEntryType(de, *vnode);

      for (size_t i = 0; i < slots; ++i) {
        bits->Set(bit_pos + i);
      }
      dentry_page->SetDirty();
      GetDirEntryCache().UpdateDirEntry(name, {vnode->Ino(), dentry_page->GetIndex(), bit_pos});
      UpdateParentMetadata(vnode, current_depth);
      return ZX_OK;
    }
  }
  return ZX_ERR_OUT_OF_RANGE;
}

zx_status_t Dir::AddLinkSafe(std::string_view name, VnodeF2fs *vnode) {
  std::lock_guard dir_lock(mutex_);
  return AddLink(name, vnode);
}

// It only removes the dentry from the dentry page, corresponding name
// entry in name page does not need to be touched during deletion.
void Dir::DeleteEntry(const DentryInfo &info, fbl::RefPtr<Page> &page, VnodeF2fs *vnode) {
  // Add to VnodeSet to ensure consistency of deleted entry.
  fs()->AddToVnodeSet(VnodeSet::kModifiedDir, Ino());

  if (TestFlag(InodeInfoFlag::kInlineDentry)) {
    return DeleteInlineEntry(info, page, vnode);
  }

  LockedPage locked_page(page);
  locked_page.WaitOnWriteback();

  DentryBlock *dentry_blk = page->GetAddress<DentryBlock>();
  DirEntry &dentry = GetDirEntry(info, page);
  auto bits = GetBitmap(locked_page.CopyRefPtr());
  ZX_DEBUG_ASSERT(bits.is_ok());
  int slots = GetDentrySlots(LeToCpu(dentry.name_len));
  size_t bit_pos = info.bit_pos;
  for (int i = 0; i < slots; ++i) {
    bits->Clear(bit_pos + i);
  }
  locked_page.SetDirty();

  std::string_view remove_name(reinterpret_cast<char *>(dentry_blk->filename[bit_pos]),
                               LeToCpu(dentry.name_len));

  GetDirEntryCache().RemoveDirEntry(remove_name);

  SetTime<Timestamps::ModificationTime>();

  if (!vnode || !vnode->IsDir()) {
    SetDirty();
  }

  if (vnode) {
    if (vnode->IsDir()) {
      DecrementLinkUnsafe();
      SetDirty();
    }

    vnode->SetDirty();
    vnode->DecrementLink();
    if (vnode->IsDir()) {
      vnode->DecrementLink();
      vnode->SetSize(0);
    }
    if (!vnode->GetLinkCount()) {
      vnode->SetOrphan();
    }
  }

  // check and deallocate dentry page if all dentries of the page are freed
  bit_pos = bits->FindNextBit(0);

  if (bit_pos == kNrDentryInBlock) {
    locked_page.Invalidate();
    TruncateHoleUnsafe(page->GetIndex(), page->GetIndex() + 1, false);
  }
}

zx_status_t Dir::MakeEmpty(ino_t parent_ino) {
  if (TestFlag(InodeInfoFlag::kInlineDentry)) {
    return MakeEmptyInlineDir(parent_ino);
  }

  zx::result dentry_page = GetNewLockedPage(0);
  if (dentry_page.is_error()) {
    return dentry_page.error_value();
  }

  DentryBlock *dentry_blk = dentry_page->GetAddress<DentryBlock>();

  DirEntry *de = &dentry_blk->dentry[kCurrentBitPos];
  de->name_len = CpuToLe(static_cast<uint16_t>(1));
  de->hash_code = 0;
  de->ino = CpuToLe(Ino());
  std::memcpy(dentry_blk->filename[kCurrentBitPos], ".", 1);
  SetDirEntryType(*de, *this);

  de = &dentry_blk->dentry[kParentBitPos];
  de->hash_code = 0;
  de->name_len = CpuToLe(static_cast<uint16_t>(2));
  de->ino = CpuToLe(parent_ino);
  std::memcpy(dentry_blk->filename[kParentBitPos], "..", 2);
  SetDirEntryType(*de, *this);

  auto bits = GetBitmap((*dentry_page).CopyRefPtr());
  ZX_DEBUG_ASSERT(bits.is_ok());
  bits->Set(0);
  bits->Set(1);
  dentry_page->SetDirty();

  return ZX_OK;
}

bool Dir::IsEmptyDir() {
  if (TestFlag(InodeInfoFlag::kInlineDentry))
    return IsEmptyInlineDir();

  const size_t nblock = DirBlocks();
  for (size_t bidx = 0; bidx < nblock; ++bidx) {
    LockedPage dentry_page;
    zx_status_t ret = GetLockedDataPage(bidx, &dentry_page);
    if (ret == ZX_ERR_NOT_FOUND)
      continue;
    if (ret != ZX_OK)
      return false;

    auto bits = GetBitmap(dentry_page.CopyRefPtr());
    ZX_DEBUG_ASSERT(bits.is_ok());
    size_t bit_pos = 0;
    if (bidx == 0)
      bit_pos = 2;

    bit_pos = bits->FindNextBit(bit_pos);
    if (bit_pos < kNrDentryInBlock)
      return false;
  }
  return true;
}

zx_status_t Dir::Readdir(fs::VdirCookie *cookie, void *dirents, size_t len, size_t *out_actual) {
  fs::DirentFiller df(dirents, len);
  uint64_t *pos_cookie = reinterpret_cast<uint64_t *>(cookie);
  uint64_t pos = *pos_cookie;
  unsigned char d_type = DT_UNKNOWN;
  zx_status_t ret = ZX_OK;

  fs::SharedLock dir_lock(mutex_);
  if (GetSize() == 0) {
    *out_actual = 0;
    return ZX_OK;
  }

  if (TestFlag(InodeInfoFlag::kInlineDentry))
    return ReadInlineDir(cookie, dirents, len, out_actual);

  const unsigned char *types = kFiletypeTable;
  const size_t npages = DirBlocks();
  size_t bit_pos = pos % kNrDentryInBlock;
  for (size_t n = pos / kNrDentryInBlock; n < npages; ++n) {
    LockedPage dentry_page;
    if (ret = GetLockedDataPage(n, &dentry_page); ret != ZX_OK)
      continue;

    const size_t start_bit_pos = bit_pos;
    DentryBlock *dentry_blk = dentry_page->GetAddress<DentryBlock>();
    auto bits = GetBitmap(dentry_page.CopyRefPtr());
    bool done = false;
    ZX_DEBUG_ASSERT(bits.is_ok());
    while (bit_pos < kNrDentryInBlock) {
      d_type = DT_UNKNOWN;
      bit_pos = bits->FindNextBit(bit_pos);
      if (bit_pos >= kNrDentryInBlock)
        break;

      DirEntry &de = dentry_blk->dentry[bit_pos];
      if (types && de.file_type < static_cast<uint8_t>(FileType::kFtMax))
        d_type = types[de.file_type];

      std::string_view name(reinterpret_cast<char *>(dentry_blk->filename[bit_pos]),
                            LeToCpu(de.name_len));

      if (de.ino && name != "..") {
        if (!df.Next(name, fuchsia_io::DirentType{d_type}, LeToCpu(de.ino))) {
          *pos_cookie += bit_pos - start_bit_pos;
          done = true;
          ret = df.BytesFilled() == 0 ? ZX_ERR_NO_SPACE : ZX_OK;
          break;
        }
      }

      size_t slots = GetDentrySlots(LeToCpu(de.name_len));
      bit_pos += slots;
    }
    if (done)
      break;

    bit_pos = 0;
    *pos_cookie = (n + 1) * kNrDentryInBlock;
  }

  *out_actual = df.BytesFilled();

  return ret;
}

void Dir::VmoRead(uint64_t offset, uint64_t length) TA_NO_THREAD_SAFETY_ANALYSIS {
  ZX_ASSERT_MSG(0,
                "Unexpected ZX_PAGER_VMO_READ request to dir node[%s:%u]. offset: %lu, size: %lu",
                GetNameViewUnsafe().data(), GetKey(), offset, length);
}

zx::result<PageBitmap> Dir::GetBitmap(fbl::RefPtr<Page> dentry_page) {
  if (TestFlag(InodeInfoFlag::kInlineDentry)) {
    if (GetKey() != dentry_page->GetKey()) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    return zx::ok(
        PageBitmap(dentry_page, InlineDentryBitmap(dentry_page.get()), MaxInlineDentry()));
  }
  if (GetKey() != dentry_page->GetVnode().GetKey()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(PageBitmap(dentry_page, dentry_page->GetAddress<DentryBlock>()->dentry_bitmap,
                           kNrDentryInBlock));
}

block_t Dir::GetBlockAddr(LockedPage &page) { return GetBlockAddrOnDataSegment(page); }

zx::result<LockedPage> Dir::FindDataPage(pgoff_t index, bool do_read) {
  fbl::RefPtr<Page> page;
  if (FindPage(index, &page) == ZX_OK) {
    LockedPage locked_page = LockedPage(std::move(page));
    if (locked_page->IsUptodate()) {
      return zx::ok(std::move(locked_page));
    }
  }

  zx::result addrs = FindAddresses(index, 1);
  if (addrs.is_error()) {
    return addrs.take_error();
  }
  block_t addr = addrs->front();
  if (addr == kNullAddr) {
    return zx::error(ZX_ERR_NOT_FOUND);
  } else if (addr == kNewAddr) {
    // By fallocate(), there is no cached page, but with kNewAddr
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  LockedPage locked_page;
  if (zx_status_t err = GrabLockedPage(index, &locked_page); err != ZX_OK) {
    return zx::error(err);
  }

  if (do_read) {
    if (auto status = fs()->MakeReadOperation(locked_page, addr, PageType::kData);
        status.is_error()) {
      return status.take_error();
    }
  }

  return zx::ok(std::move(locked_page));
}

zx_status_t Dir::GetLockedDataPage(pgoff_t index, LockedPage *out) {
  zx::result page = GetLockedDataPages(index, index + 1);
  if (page.is_error()) {
    return page.error_value();
  }
  if (page->empty() || page.value()[0] == nullptr) {
    return ZX_ERR_NOT_FOUND;
  }

  *out = std::move(page.value()[0]);
  return ZX_OK;
}

zx::result<std::vector<LockedPage>> Dir::GetLockedDataPages(const pgoff_t start,
                                                            const size_t size) {
  std::vector<LockedPage> pages(size);
  std::vector<pgoff_t> offsets;
  auto found_pages = FindLockedPages(start, start + size);
  auto target = found_pages.begin();
  bool uptodate = true;
  for (size_t i = 0; i < size; ++i) {
    size_t offset = i + start;
    if (target != found_pages.end() && offset == (*target)->GetKey()) {
      pages[i] = std::move(*target);
      ++target;
    }
    if (!pages[i] || !pages[i]->IsUptodate()) {
      uptodate = false;
      offsets.push_back(offset);
    }
  }
  // In case that all pages are found and uptodate, return them without requesting read I/Os.
  if (uptodate) {
    return zx::ok(std::move(pages));
  }

  zx::result found_addrs = FindAddresses(offsets);
  if (found_addrs.is_error()) {
    return found_addrs.take_error();
  }

  // Build addrs to request read I/Os for valid blocks.
  std::vector<block_t> addrs(size, kNullAddr);
  auto addr = found_addrs->begin();
  bool need_read_op = false;
  for (auto offset : offsets) {
    size_t index = offset - start;
    block_t current = *addr++;
    if (current == kNullAddr) {
      continue;
    }
    auto &page = pages[index];
    if (!page) {
      GrabLockedPage(offset, &page);
    }
    // Pages should be uptodate when kNewAddr are assigned.
    ZX_DEBUG_ASSERT(current != kNewAddr || page->SetUptodate());
    need_read_op = true;
    addrs[index] = current;
  }

  if (!need_read_op) {
    return zx::ok(std::move(pages));
  }

  zx::result status = fs()->MakeReadOperations(pages, addrs, PageType::kData);
  if (status.is_error()) {
    return status.take_error();
  }
  return zx::ok(std::move(pages));
}

zx::result<LockedPage> Dir::FindVictimPage(pgoff_t index) {
  zx::result page = FindDataPage(index);
  if (page.is_error()) {
    return page.take_error();
  }
  page->WaitOnWriteback();
  return page;
}

void Dir::SetSize(const size_t nbytes) { vmo_manager().SetContentSize(nbytes); }

zx::result<LockedPage> Dir::GetNewLockedPage(pgoff_t index) {
  LockedPage page;
  if (zx_status_t ret = GetNewDataPage(index, &page); ret != ZX_OK) {
    return zx::error(ret);
  }
  size_t new_size = (index + 1) * kBlockSize;
  if (GetSize() < new_size) {
    SetSize(new_size);
    SetFlag(InodeInfoFlag::kUpdateDir);
    SetDirty();
  }
  return zx::ok(std::move(page));
}

}  // namespace f2fs
