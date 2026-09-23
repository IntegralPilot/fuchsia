// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_XATTR_H_
#define SRC_STORAGE_F2FS_XATTR_H_

#include <span>

#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/layout.h"

namespace f2fs {
class LockedPage;
constexpr uint32_t kXattrMagic = 0xF2F52011;
constexpr uint32_t kXattrMaxRefcount = 1024;

constexpr size_t kValidXattrBlockSize = kBlockSize - sizeof(NodeFooter);

// Xattr spaces use 4-byte alignment
using xattr_slot_t = uint32_t;
constexpr uint32_t kXattrAlign = sizeof(xattr_slot_t);

constexpr uint32_t XattrSlots(size_t size) {
  return ((safemath::CheckAdd(safemath::checked_cast<uint32_t>(size), kXattrAlign) - 1) /
          kXattrAlign)
      .ValueOrDie();
}

constexpr size_t kMaxXattrSlots = kInlineXattrAddrs + XattrSlots(kValidXattrBlockSize);

enum class XattrIndex {
  kUser = 1,
  kPosixAclAccess,
  kPosixAclDefault,
  kTrusted,
  kLustre,
  kSecurity,
  kAdvise,
  kEncryption = 9,
  kVerity = 11,
};

enum class XattrOption {
  kNone = 0x0,
  kCreate = 0x1,
  kReplace = 0x2,
};

struct XattrHeader {
  uint32_t magic = 0;
  uint32_t refcount = 0;
  uint32_t reserved[4];
} __PACKED;

constexpr size_t kXattrHeaderSlots = XattrSlots(sizeof(XattrHeader));
static_assert(kMaxXattrSlots >= kXattrHeaderSlots);

struct XattrEntryInfo {
  uint8_t name_index;
  uint8_t name_len;
  uint16_t value_size;

  bool IsLast() const { return name_index == 0 && name_len == 0 && value_size == 0; }
  uint32_t Slots() const {
    return XattrSlots(sizeof(XattrEntryInfo) + sizeof(char *) + name_len + value_size);
  }
  size_t Size() const { return safemath::checked_cast<size_t>(Slots()) * kXattrAlign; }
  size_t NameOffset() const { return sizeof(XattrEntryInfo); }
  size_t ValueOffset() const { return NameOffset() + name_len; }
} __PACKED;

constexpr size_t kMaxXattrValueLength =
    safemath::checked_cast<size_t>(XattrSlots(kValidXattrBlockSize) * kXattrAlign) -
    sizeof(XattrHeader) - sizeof(XattrEntryInfo) - sizeof(char *);

class XattrOperator {
 public:
  explicit XattrOperator(LockedPage &ipage, LockedPage &xattr_page);

  // Not copyable or moveable
  XattrOperator(const XattrOperator &) = delete;
  XattrOperator &operator=(const XattrOperator &) = delete;
  XattrOperator(XattrOperator &&) = delete;
  XattrOperator &operator=(XattrOperator &&) = delete;

  zx::result<uint32_t> FindSlotOffset(XattrIndex index, std::string_view name);
  zx_status_t Add(XattrIndex index, std::string_view name, std::span<const uint8_t> value);
  void Remove(uint32_t offset);
  zx::result<size_t> Lookup(XattrIndex index, std::string_view name, std::span<uint8_t> out);

  void WriteTo(LockedPage &ipage, LockedPage &xattr_page);

  uint32_t GetEndOffset(uint32_t from = kXattrHeaderSlots);

 private:
  std::unique_ptr<std::array<xattr_slot_t, kMaxXattrSlots>> buffer_;
  const uint32_t available_slots_;
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_XATTR_H_
