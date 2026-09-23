// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/dir.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/file.h"
#include "src/storage/f2fs/node.h"
#include "src/storage/f2fs/node_page.h"
#include "src/storage/f2fs/superblock_info.h"

namespace f2fs {

uint8_t *Dir::InlineDentryBitmap(Page *page) {
  Inode &inode = page->GetAddress<Node>()->i;
  return reinterpret_cast<uint8_t *>(
      &inode.i_addr[GetExtraSize() / sizeof(uint32_t) + kInlineStartOffset]);
}

uint64_t Dir::InlineDentryBitmapSize() const {
  return CheckedDivRoundUp<uint64_t>(MaxInlineDentry(), kBitsPerByte);
}

DirEntry *Dir::InlineDentryArray(Page *page, VnodeF2fs &vnode) {
  uint8_t *base = InlineDentryBitmap(page);
  size_t reserved = safemath::checked_cast<uint32_t>(
      (vnode.MaxInlineData() -
       safemath::CheckMul(vnode.MaxInlineDentry(), (kSizeOfDirEntry + kDentrySlotLen)))
          .ValueOrDie());

  return reinterpret_cast<DirEntry *>(base + reserved);
}

char (*Dir::InlineDentryFilenameArray(Page *page, VnodeF2fs &vnode)) [kDentrySlotLen] {
  uint8_t *base = InlineDentryBitmap(page);
  size_t reserved = safemath::checked_cast<uint32_t>(
      (vnode.MaxInlineData() - safemath::CheckMul(vnode.MaxInlineDentry(), kDentrySlotLen))
          .ValueOrDie());
  return reinterpret_cast<char (*)[kDentrySlotLen]>(base + reserved);
}

zx::result<DentryInfo> Dir::FindInInlineDir(std::string_view name, fbl::RefPtr<Page> *res_page) {
  LockedPage ipage;
  if (zx_status_t ret = fs()->GetNodeManager().GetNodePage(Ino(), &ipage); ret != ZX_OK) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  std::vector<nid_t> nids;
  size_t n = 0;
  f2fs_hash_t namehash = DentryHash(name);
  auto bits = GetBitmap(ipage.CopyRefPtr());
  ZX_DEBUG_ASSERT(bits.is_ok());
  DentryInfo ret = {kNullIno, kCachedInlineDirEntryPageIndex, 0};
  res_page->reset();
  for (size_t bit_pos = 0; (bit_pos = bits->FindNextBit(bit_pos)) < MaxInlineDentry();) {
    const DirEntry &de = InlineDentryArray(ipage.get(), *this)[bit_pos];
    std::string_view entry_name(InlineDentryFilenameArray(ipage.get(), *this)[bit_pos],
                                LeToCpu(de.name_len));
    nid_t ino = LeToCpu(de.ino);
    if (!*res_page) {
      if (EarlyMatchName(name, namehash, de) &&
          !memcmp(entry_name.data(), name.data(), name.length())) {
        ret = {ino, kCachedInlineDirEntryPageIndex, bit_pos};
        *res_page = ipage.CopyRefPtr();
      }
    }
    if (*res_page) {
      nids.push_back(ino);
      GetDirEntryCache().UpdateDirEntry(entry_name, {ino, kCachedInlineDirEntryPageIndex, bit_pos});
      if (kDefaultNodeReadSize <= ++n) {
        break;
      }
    }
    // For the most part, it should be a bug when name_len is zero.
    // We stop here for figuring out where the bugs are occurred.
    ZX_DEBUG_ASSERT(de.name_len > 0);
    bit_pos += GetDentrySlots(LeToCpu(de.name_len));
  }
  if (!*res_page) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  ipage.reset();
  if (nids.size() > 1) {
    ZX_ASSERT(fs()->GetNodeManager().GetNodePages(nids).is_ok());
  }
  return zx::ok(ret);
}

zx_status_t Dir::MakeEmptyInlineDir(ino_t parent_ino) {
  LockedPage ipage;

  if (zx_status_t err = fs()->GetNodeManager().GetNodePage(Ino(), &ipage); err != ZX_OK)
    return err;

  DirEntry *de = &InlineDentryArray(&(*ipage), *this)[0];
  de->name_len = CpuToLe(static_cast<uint16_t>(1));
  de->hash_code = 0;
  de->ino = CpuToLe(Ino());
  std::memcpy(InlineDentryFilenameArray(&(*ipage), *this)[0], ".", 1);
  SetDirEntryType(*de, *this);

  de = &InlineDentryArray(&(*ipage), *this)[1];
  de->hash_code = 0;
  de->name_len = CpuToLe(static_cast<uint16_t>(2));
  de->ino = CpuToLe(parent_ino);
  std::memcpy(InlineDentryFilenameArray(&(*ipage), *this)[1], "..", 2);
  SetDirEntryType(*de, *this);

  auto bits = GetBitmap(ipage.CopyRefPtr());
  ZX_DEBUG_ASSERT(bits.is_ok());
  bits->Set(0);
  bits->Set(1);

  ipage.SetDirty();

  if (GetSize() < MaxInlineData()) {
    SetSize(MaxInlineData());
    SetFlag(InodeInfoFlag::kUpdateDir);
  }

  return ZX_OK;
}

size_t Dir::RoomInInlineDir(const PageBitmap &bits, size_t slots) {
  size_t bit_start = 0;
  while (true) {
    size_t zero_start = bits.FindNextZeroBit(bit_start);
    if (zero_start >= MaxInlineDentry())
      return MaxInlineDentry();

    size_t zero_end = bits.FindNextBit(zero_start);
    if (zero_end - zero_start >= slots)
      return zero_start;

    bit_start = zero_end + 1;
    if (bit_start >= MaxInlineDentry()) {
      return MaxInlineDentry();
    }
  }
}

zx_status_t Dir::ConvertInlineDir() {
  LockedPage data_page;
  if (zx_status_t ret = GrabLockedPage(0, &data_page); ret != ZX_OK) {
    return ret;
  }

  zx::result path = GetNodePath(0);
  if (path.is_error()) {
    return path.error_value();
  }
  zx::result dnode_page = fs()->GetNodeManager().GetLockedDnodePage(*path, IsDir());
  if (dnode_page.is_error()) {
    return dnode_page.error_value();
  }
  size_t ofs_in_dnode = GetOfsInDnode(*path);
  NodePage &ipage = (*dnode_page).GetPage<NodePage>();
  block_t data_blkaddr = ipage.GetBlockAddr(ofs_in_dnode);
  ZX_DEBUG_ASSERT(data_blkaddr == kNullAddr);

  if (zx_status_t err = ReserveNewBlock(*dnode_page, ofs_in_dnode); err != ZX_OK) {
    return err;
  }
  AddBlocksUnsafe(path->num_new_nodes + 1);

  data_page.WaitOnWriteback();
  data_page.Zero();

  DentryBlock *dentry_blk = data_page->GetAddress<DentryBlock>();

  // copy data from inline dentry block to new dentry block
  std::memcpy(dentry_blk->dentry_bitmap, InlineDentryBitmap(&ipage), InlineDentryBitmapSize());
  std::memcpy(dentry_blk->dentry, InlineDentryArray(&ipage, *this),
              sizeof(DirEntry) * MaxInlineDentry());
  std::memcpy(
      dentry_blk->filename, InlineDentryFilenameArray(&ipage, *this),
      safemath::CheckMul(safemath::checked_cast<size_t>(MaxInlineDentry()), kNameLen).ValueOrDie());

  data_page.SetDirty();
  // clear inline dir and flag after data writeback
  (*dnode_page).WaitOnWriteback();
  (*dnode_page).Zero(InlineDataOffset(), InlineDataOffset() + MaxInlineData());
  ClearFlag(InodeInfoFlag::kInlineDentry);

  if (!TestFlag(InodeInfoFlag::kInlineXattr)) {
    SetInlineXattrSize(0);
  }

  if (GetSize() < kBlockSize) {
    SetSize(kBlockSize);
    SetFlag(InodeInfoFlag::kUpdateDir);
  }

  SetDirty();
#if 0  // porting needed
  // stat_dec_inline_inode(dir);
#endif
  return ZX_OK;
}

zx::result<bool> Dir::AddInlineEntry(std::string_view name, VnodeF2fs *vnode) {
  {
    LockedPage ipage;
    if (zx_status_t err = fs()->GetNodeManager().GetNodePage(Ino(), &ipage); err != ZX_OK) {
      return zx::error(err);
    }

    f2fs_hash_t name_hash = DentryHash(name);
    uint16_t slots = GetDentrySlots(safemath::checked_cast<uint16_t>(name.length()));
    auto bits = GetBitmap(ipage.CopyRefPtr());
    ZX_DEBUG_ASSERT(bits.is_ok());
    size_t bit_pos = RoomInInlineDir(*bits, slots);
    if (bit_pos + slots <= MaxInlineDentry()) {
      ipage.WaitOnWriteback();

      if (zx_status_t err = vnode->InitInodeMetadata(); err != ZX_OK) {
        if (TestFlag(InodeInfoFlag::kUpdateDir)) {
          ClearFlag(InodeInfoFlag::kUpdateDir);
          SetDirty();
        }
        return zx::error(err);
      }

      DirEntry &de = InlineDentryArray(ipage.get(), *this)[bit_pos];
      de.hash_code = name_hash;
      de.name_len = static_cast<uint16_t>(CpuToLe(name.length()));
      std::memcpy(InlineDentryFilenameArray(ipage.get(), *this)[bit_pos], name.data(),
                  name.length());
      de.ino = CpuToLe(vnode->Ino());
      SetDirEntryType(de, *vnode);
      for (int i = 0; i < slots; ++i) {
        bits->Set(bit_pos + i);
      }
      DentryInfo info = {vnode->Ino(), kCachedInlineDirEntryPageIndex, bit_pos};
      GetDirEntryCache().UpdateDirEntry(name, info);
      ipage.SetDirty();
      UpdateParentMetadata(vnode, 0);
      vnode->SetDirty();
      SetDirty();

      ClearFlag(InodeInfoFlag::kUpdateDir);
      return zx::ok(false);
    }
  }

  if (auto ret = ConvertInlineDir(); ret != ZX_OK) {
    return zx::error(ret);
  }
  return zx::ok(true);
}

void Dir::DeleteInlineEntry(const DentryInfo &info, fbl::RefPtr<Page> &page, VnodeF2fs *vnode) {
  LockedPage lock_page(page);
  lock_page.WaitOnWriteback();

  ZX_DEBUG_ASSERT(info.page_index == kCachedInlineDirEntryPageIndex);
  DirEntry &dentry = InlineDentryArray(lock_page.get(), *this)[info.bit_pos];
  int slots = GetDentrySlots(LeToCpu(dentry.name_len));
  auto bits = GetBitmap(lock_page.CopyRefPtr());
  ZX_DEBUG_ASSERT(bits.is_ok());
  for (int i = 0; i < slots; ++i) {
    bits->Clear(info.bit_pos + i);
  }

  lock_page.SetDirty();

  std::string_view remove_name(
      reinterpret_cast<char *>(InlineDentryFilenameArray(lock_page.get(), *this)[info.bit_pos]),
      LeToCpu(dentry.name_len));

  GetDirEntryCache().RemoveDirEntry(remove_name);

  SetTime<Timestamps::ModificationTime>();

  if (vnode && vnode->IsDir()) {
    DecrementLinkUnsafe();
  }

  if (vnode) {
    vnode->SetDirty();
    vnode->DecrementLink();
    if (vnode->IsDir()) {
      vnode->DecrementLink();
      vnode->SetSize(0);
    }
    if (vnode->GetLinkCount() == 0) {
      vnode->SetOrphan();
    }
  }
  SetDirty();
}

bool Dir::IsEmptyInlineDir() {
  LockedPage ipage;

  if (zx_status_t err = fs()->GetNodeManager().GetNodePage(Ino(), &ipage); err != ZX_OK)
    return false;

  size_t bit_pos = 2;
  auto bits = GetBitmap(ipage.CopyRefPtr());
  ZX_DEBUG_ASSERT(bits.is_ok());
  bit_pos = bits->FindNextBit(bit_pos);
  return bit_pos >= MaxInlineDentry();
}

zx_status_t Dir::ReadInlineDir(fs::VdirCookie *cookie, void *dirents, size_t len,
                               size_t *out_actual) {
  fs::DirentFiller df(dirents, len);
  uint64_t *pos_cookie = reinterpret_cast<uint64_t *>(cookie);

  if (*pos_cookie == MaxInlineDentry()) {
    *out_actual = 0;
    return ZX_OK;
  }

  LockedPage ipage;

  if (zx_status_t err = fs()->GetNodeManager().GetNodePage(Ino(), &ipage); err != ZX_OK)
    return err;

  const unsigned char *types = kFiletypeTable;
  size_t bit_pos = *pos_cookie % MaxInlineDentry();
  auto bits = GetBitmap(ipage.CopyRefPtr());
  ZX_DEBUG_ASSERT(bits.is_ok());

  while (bit_pos < MaxInlineDentry()) {
    if (bit_pos = bits->FindNextBit(bit_pos); bit_pos >= MaxInlineDentry()) {
      break;
    }

    DirEntry *de = &InlineDentryArray(ipage.get(), *this)[bit_pos];
    fuchsia_io::DirentType d_type = fuchsia_io::DirentType::kUnknown;
    if (de->file_type < static_cast<uint8_t>(FileType::kFtMax))
      d_type = fuchsia_io::DirentType{types[de->file_type]};

    std::string_view name(InlineDentryFilenameArray(ipage.get(), *this)[bit_pos],
                          LeToCpu(de->name_len));

    if (de->ino && name != "..") {
      if (!df.Next(name, d_type, LeToCpu(de->ino))) {
        *pos_cookie = bit_pos;

        *out_actual = df.BytesFilled();
        return *out_actual == 0 ? ZX_ERR_NO_SPACE : ZX_OK;
      }
    }

    bit_pos += GetDentrySlots(LeToCpu(de->name_len));
  }

  *pos_cookie = MaxInlineDentry();
  *out_actual = df.BytesFilled();

  return ZX_OK;
}

zx_status_t File::ConvertInlineData() {
  if (!TestFlag(InodeInfoFlag::kInlineData)) {
    return ZX_OK;
  }
  LockedPage page;
  if (TestFlag(InodeInfoFlag::kDataExist)) {
    if (zx_status_t ret = GrabLockedPage(0, &page); ret != ZX_OK) {
      return ret;
    }
  }

  zx::result path = GetNodePath(0);
  if (path.is_error()) {
    return path.error_value();
  }
  zx::result dnode_page = fs()->GetNodeManager().GetLockedDnodePage(*path, IsDir());
  if (dnode_page.is_error()) {
    return dnode_page.error_value();
  }
  size_t ofs_in_dnode = GetOfsInDnode(*path);
  NodePage &ipage = (*dnode_page).GetPage<NodePage>();
  block_t data_blkaddr = ipage.GetBlockAddr(ofs_in_dnode);
  ZX_DEBUG_ASSERT(data_blkaddr == kNullAddr);

  if (zx_status_t err = ReserveNewBlock(*dnode_page, ofs_in_dnode); err != ZX_OK) {
    return err;
  }
  AddBlocks(path->num_new_nodes + 1);

  if (TestFlag(InodeInfoFlag::kDataExist)) {
    page.WaitOnWriteback();
    page->Write(ipage.GetAddress<uint8_t>() + InlineDataOffset(), 0, GetSize());
    page.SetDirty();

    (*dnode_page).WaitOnWriteback();
    (*dnode_page).Zero(InlineDataOffset(), InlineDataOffset() + MaxInlineData());
  }
  ClearFlag(InodeInfoFlag::kInlineData);
  ClearFlag(InodeInfoFlag::kDataExist);

  SetDirty();

  return ZX_OK;
}

zx_status_t File::WriteInline(const void *data, size_t len, size_t offset,
                              size_t *out_actual) TA_NO_THREAD_SAFETY_ANALYSIS {
  LockedPage inline_page;
  if (zx_status_t ret = fs()->GetNodeManager().GetNodePage(Ino(), &inline_page); ret != ZX_OK) {
    return ret;
  }

  inline_page.WaitOnWriteback();
  inline_page->Write(data, InlineDataOffset() + offset, len);

  SetFlag(InodeInfoFlag::kDataExist);
  inline_page.SetDirty();

  SetTime<Timestamps::ModificationTime>();
  SetDirty();

  *out_actual = len;

  return ZX_OK;
}

zx_status_t File::TruncateInline(size_t len) {
  LockedPage inline_page;
  if (zx_status_t ret = fs()->GetNodeManager().GetNodePage(Ino(), &inline_page); ret != ZX_OK) {
    return ret;
  }

  inline_page.WaitOnWriteback();

  size_t size = GetSize();
  size_t size_diff = (len > size) ? (len - size) : (size - len);
  size_t offset = InlineDataOffset() + ((len > size) ? size : len);
  inline_page.Zero(offset, offset + size_diff);

  if (len == 0) {
    ClearFlag(InodeInfoFlag::kDataExist);
  }
  inline_page.SetDirty();

  SetDirty();

  return ZX_OK;
}

zx_status_t File::RecoverInlineData(NodePage &page) {
  // The inline_data recovery policy is as follows.
  // [prev.] [next] of inline_data flag
  //    o       o  -> recover inline_data
  //    o       x  -> remove inline_data, and then recover data blocks
  //    x       o  -> remove data blocks, and then recover inline_data (not happen)
  //    x       x  -> recover data blocks
  // ([prev.] is checkpointed data. And [next] is data written and fsynced after checkpoint.)

  if (page.IsInode()) {
    Inode &inode = page.GetAddress<Node>()->i;

    // [next] have inline data.
    if (inode.i_inline & kInlineData) {
      // Process inline.
      LockedPage ipage;
      if (zx_status_t err = fs()->GetNodeManager().GetNodePage(Ino(), &ipage); err != ZX_OK) {
        return err;
      }
      BlockBuffer block;
      ipage.WaitOnWriteback();
      page.Read(block.get(), InlineDataOffset(), MaxInlineData());
      ipage->Write(block.get(), InlineDataOffset(), MaxInlineData());

      SetFlag(InodeInfoFlag::kInlineData);
      SetFlag(InodeInfoFlag::kDataExist);

      ipage.SetDirty();
      return ZX_OK;
    }
  }

  // [prev.] has inline data but [next] has no inline data.
  if (TestFlag(InodeInfoFlag::kInlineData)) {
    TruncateInline(0);
    ClearFlag(InodeInfoFlag::kInlineData);
    ClearFlag(InodeInfoFlag::kDataExist);
  }
  return ZX_ERR_NOT_SUPPORTED;
}

}  // namespace f2fs
