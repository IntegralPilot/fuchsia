// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/xattr.h"

#include "src/storage/f2fs/file_cache.h"

namespace f2fs {

XattrOperator::XattrOperator(LockedPage &ipage, LockedPage &xattr_page)
    : available_slots_(ipage ? kMaxXattrSlots : XattrSlots(kValidXattrBlockSize)) {
  buffer_ = std::make_unique<std::array<xattr_slot_t, kMaxXattrSlots>>();
  buffer_->fill(0);

  if (ipage) {
    Inode &inode = ipage->GetAddress<Node>()->i;
    std::memcpy(buffer_->data(), &(inode.i_addr[kAddrsPerInode - kInlineXattrAddrs]),
                safemath::checked_cast<size_t>(kXattrAlign * kInlineXattrAddrs));
  }

  if (xattr_page) {
    ZX_DEBUG_ASSERT(xattr_page->Size() >= kValidXattrBlockSize);
    std::memcpy(buffer_->data() + (ipage ? kInlineXattrAddrs : 0),
                xattr_page->GetAddress<uint8_t>(), kValidXattrBlockSize);
  }

  if (buffer_->at(0) != kXattrMagic) {
    XattrHeader header{
        .magic = kXattrMagic,
        .refcount = 1,
    };

    std::memcpy(buffer_->data(), &header, sizeof(XattrHeader));
  }
}

zx::result<uint32_t> XattrOperator::FindSlotOffset(XattrIndex index, std::string_view name) {
  uint32_t slot_offset = kXattrHeaderSlots;

  while (slot_offset < available_slots_) {
    XattrEntryInfo entry_info;
    std::memcpy(&entry_info, &buffer_->at(slot_offset), sizeof(XattrEntryInfo));
    if (entry_info.IsLast()) {
      break;
    }

    if (static_cast<uint8_t>(index) == entry_info.name_index) {
      ZX_ASSERT(slot_offset + entry_info.Slots() <= buffer_->size());
      std::vector<char> entry(entry_info.Size());
      std::memcpy(entry.data(), &buffer_->at(slot_offset), entry.size());

      if (std::string(&entry.at(entry_info.NameOffset()), entry_info.name_len) == name) {
        return zx::ok(slot_offset);
      }
    }

    slot_offset += entry_info.Slots();
  }

  return zx::error(ZX_ERR_NOT_FOUND);
}

zx_status_t XattrOperator::Add(XattrIndex index, std::string_view name,
                               std::span<const uint8_t> value) {
  uint32_t slot_offset = GetEndOffset();
  if (slot_offset >= available_slots_) {
    return ZX_ERR_NO_SPACE;
  }

  XattrEntryInfo new_info = {.name_index = static_cast<uint8_t>(index),
                             .name_len = safemath::checked_cast<uint8_t>(name.length()),
                             .value_size = safemath::checked_cast<uint16_t>(value.size())};

  if (slot_offset + new_info.Slots() > available_slots_) {
    return ZX_ERR_NO_SPACE;
  }

  std::vector<char> entry(new_info.Size());
  std::memcpy(entry.data(), &new_info, sizeof(XattrEntryInfo));
  std::memcpy(&entry.at(new_info.NameOffset()), name.data(), name.length());
  std::memcpy(&entry.at(new_info.ValueOffset()), value.data(), value.size());

  std::memcpy(&buffer_->at(slot_offset), entry.data(), entry.size());

  return ZX_OK;
}

void XattrOperator::Remove(uint32_t offset) {
  XattrEntryInfo entry_info;
  std::memcpy(&entry_info, &buffer_->at(offset), sizeof(XattrEntryInfo));
  uint32_t entry_slots = entry_info.Slots();
  uint32_t next_entry_offset = offset + entry_slots;

  if (next_entry_offset >= available_slots_) {
    ZX_ASSERT(offset + entry_slots <= available_slots_);
    std::fill(buffer_->begin() + offset, buffer_->begin() + offset + entry_slots, 0);
    return;
  }

  uint32_t end_offset = GetEndOffset(next_entry_offset);
  std::move(buffer_->begin() + next_entry_offset, buffer_->begin() + end_offset,
            buffer_->begin() + offset);
  std::fill(buffer_->begin() + end_offset - entry_slots, buffer_->begin() + end_offset, 0);
}

zx::result<size_t> XattrOperator::Lookup(XattrIndex index, std::string_view name,
                                         std::span<uint8_t> out) {
  zx::result<uint32_t> offset = FindSlotOffset(index, name);
  if (offset.is_error()) {
    return offset.take_error();
  }

  if (out.empty()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  XattrEntryInfo entry_info;
  std::memcpy(&entry_info, &buffer_->at(*offset), sizeof(XattrEntryInfo));

  ZX_ASSERT(*offset + entry_info.Slots() <= buffer_->size());
  std::vector<char> entry(entry_info.Size());
  std::memcpy(entry.data(), &buffer_->at(*offset), entry.size());
  std::memcpy(out.data(), &entry.at(entry_info.ValueOffset()), entry_info.value_size);
  return zx::ok(entry_info.value_size);
}

void XattrOperator::WriteTo(LockedPage &ipage, LockedPage &xattr_page) {
  if (ipage) {
    Inode &inode = ipage->GetAddress<Node>()->i;
    std::memcpy(&(inode.i_addr[kAddrsPerInode - kInlineXattrAddrs]), buffer_->data(),
                safemath::checked_cast<size_t>(kXattrAlign * kInlineXattrAddrs));
    ipage.SetDirty();
  }

  if (xattr_page) {
    ZX_DEBUG_ASSERT(xattr_page->Size() >= kValidXattrBlockSize);
    std::memcpy(xattr_page->GetAddress<uint8_t>(),
                buffer_->data() + (ipage ? kInlineXattrAddrs : 0), kValidXattrBlockSize);
    xattr_page.SetDirty();
  }
}

uint32_t XattrOperator::GetEndOffset(uint32_t from) {
  from = std::max(from, safemath::checked_cast<uint32_t>(kXattrHeaderSlots));

  while (from < available_slots_) {
    XattrEntryInfo entry_info;
    std::memcpy(&entry_info, &buffer_->at(from), sizeof(XattrEntryInfo));
    if (entry_info.IsLast()) {
      break;
    }

    from += entry_info.Slots();
  }

  return from;
}

}  // namespace f2fs
