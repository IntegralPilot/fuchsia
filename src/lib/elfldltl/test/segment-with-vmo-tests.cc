// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/loadinfo-mutable-memory.h>
#include <lib/elfldltl/segment-with-vmo.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/zircon.h>
#include <lib/zx/vmo.h>

#include <vector>

#include <gtest/gtest.h>

#include "load-tests.h"

namespace {

using LoadInfo =
    elfldltl::LoadInfo<elfldltl::Elf<>, elfldltl::StdContainer<std::vector>::Container,
                       elfldltl::PhdrLoadPolicy::kBasic, elfldltl::SegmentWithVmo::Copy>;

using GetMutableMemory = elfldltl::SegmentWithVmo::GetMutableMemory<LoadInfo>;

using size_type = elfldltl::Elf<>::size_type;
using Phdr = elfldltl::Elf<>::Phdr;

static_assert(std::is_move_constructible_v<LoadInfo::Segment>);
static_assert(std::is_move_assignable_v<LoadInfo::Segment>);

TEST(ElfldltlSegmentWithVmoTests, GetMutableMemoryCtor) {
  // Default-constructible.
  GetMutableMemory default_constructed;

  // Copy-constructible.
  GetMutableMemory copy_constructed{default_constructed};

  // Copy-assignable.
  default_constructed = copy_constructed;

  // Using it in default-constructed state accesses the invalid VMO handle.
  {
    // Mock up a fake segment.  All that really matters is that segment.vmo()
    // is not set yet so GetMutableMemory will try to create a new child from
    // the VMO provided at construction (which is none, so invalid handle).
    constexpr size_type kSegmentAddr = 0x10000;
    constexpr size_type kSegmentSize = 0x20000;
    LoadInfo::Segment segment = LoadInfo::DataSegment{
        kSegmentAddr,  // offset
        kSegmentAddr,  // vaddr
        kSegmentSize,  // memsz
        kSegmentSize,  // filesz
    };
    ASSERT_FALSE(std::get<LoadInfo::DataSegment>(segment).vmo());
    elfldltl::testing::ExpectedSingleError expected_error{
        "cannot create copy-on-write VMO for segment contents",
        elfldltl::FileOffset{kSegmentAddr},
        ": ",
        elfldltl::ZirconError{ZX_ERR_BAD_HANDLE},
    };
    auto result = default_constructed(expected_error, segment);
    ASSERT_TRUE(result.is_error());
    // ExpectedSingleError always says keep going.
    EXPECT_TRUE(result.error_value());
  }
}

TEST(ElfldltlSegmentWithVmoTests, GetMutableMemoryCreatesVmo) {
  constexpr size_type kSegmentAddr = 0x10000;
  constexpr size_type kSegmentSize = 0x20000;
  constexpr uint64_t kFileSize = kSegmentAddr + kSegmentSize;

  zx::vmo file_vmo;
  ASSERT_EQ(zx::vmo::create(kFileSize, 0, &file_vmo), ZX_OK);

  zx_info_vmo_t file_vmo_info;
  ASSERT_EQ(file_vmo.get_info(ZX_INFO_VMO, &file_vmo_info, sizeof(file_vmo_info), nullptr, nullptr),
            ZX_OK);

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  GetMutableMemory get_mutable_memory{file_vmo.borrow()};

  LoadInfo::Segment segment = LoadInfo::DataSegment{
      kSegmentAddr,  // offset
      kSegmentAddr,  // vaddr
      kSegmentSize,  // memsz
      kSegmentSize,  // filesz
  };
  auto& data_segment = std::get<LoadInfo::DataSegment>(segment);
  ASSERT_FALSE(data_segment.vmo());

  auto result = get_mutable_memory(diag, segment);
  ASSERT_TRUE(result.is_ok()) << result.error_value();

  auto memory = std::move(result).value();
  EXPECT_EQ(memory.image().size_bytes(), kSegmentSize);
  EXPECT_EQ(memory.base(), kSegmentAddr);

  ASSERT_TRUE(data_segment.vmo());

  zx_info_vmo_t segment_vmo_info;
  ASSERT_EQ(data_segment.vmo().get_info(ZX_INFO_VMO, &segment_vmo_info, sizeof(segment_vmo_info),
                                        nullptr, nullptr),
            ZX_OK);
  EXPECT_EQ(segment_vmo_info.parent_koid, file_vmo_info.koid);
}

TEST(ElfldltlSegmentWithVmoTests, GetMutableMemoryReusesVmo) {
  constexpr size_type kSegmentAddr = 0x10000;
  constexpr size_type kSegmentSize = 0x20000;
  constexpr uint64_t kFileSize = kSegmentAddr + kSegmentSize;

  zx::vmo file_vmo;
  ASSERT_EQ(zx::vmo::create(kFileSize, 0, &file_vmo), ZX_OK);

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  GetMutableMemory get_mutable_memory{file_vmo.borrow()};

  LoadInfo::Segment segment = LoadInfo::DataSegment{
      kSegmentAddr,  // offset
      kSegmentAddr,  // vaddr
      kSegmentSize,  // memsz
      kSegmentSize,  // filesz
  };
  auto& data_segment = std::get<LoadInfo::DataSegment>(segment);
  ASSERT_FALSE(data_segment.vmo());

  ASSERT_TRUE(elfldltl::SegmentWithVmo::MakeMutable(diag, data_segment, file_vmo.borrow()));
  ASSERT_TRUE(data_segment.vmo());

  zx_info_vmo_t segment_vmo_info;
  ASSERT_EQ(data_segment.vmo().get_info(ZX_INFO_VMO, &segment_vmo_info, sizeof(segment_vmo_info),
                                        nullptr, nullptr),
            ZX_OK);
  const zx::unowned_vmo segment_vmo_handle{data_segment.vmo()};

  auto result = get_mutable_memory(diag, segment);
  ASSERT_TRUE(result.is_ok()) << result.error_value();

  auto memory = std::move(result).value();
  EXPECT_EQ(memory.image().size_bytes(), kSegmentSize);
  EXPECT_EQ(memory.base(), kSegmentAddr);

  // The handle in use should not have changed.
  EXPECT_EQ(data_segment.vmo().get(), segment_vmo_handle->get());

  ASSERT_TRUE(data_segment.vmo());

  // Make doubly sure the handle name still refers to the same VMO.
  zx_info_vmo_t after_vmo_info;
  ASSERT_EQ(data_segment.vmo().get_info(ZX_INFO_VMO, &after_vmo_info, sizeof(after_vmo_info),
                                        nullptr, nullptr),
            ZX_OK);
  EXPECT_EQ(segment_vmo_info.koid, after_vmo_info.koid);
}

TEST(ElfldltlSegmentWithVmoTests, MutableMemoryStore) {
  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  // Load up a whole simulated set of segments.
  constexpr size_type kPageSize = 0x1000;
  constexpr size_type kMutable1Addr = 0x1000;
  constexpr size_type kMutable1Size = 0x2000;
  constexpr size_type kMutable2Addr = 0x3000;
  constexpr size_type kMutable2Size = 0x4000;
  constexpr size_type kMutable3Addr = 0x7000;
  constexpr size_type kMutable3Size = 0x4000;
  constexpr Phdr kPhdrs[] = {{.flags = Phdr::kRead,
                              .offset = 0,
                              .vaddr = 0,
                              .filesz = kMutable1Addr,
                              .memsz = kMutable1Addr},
                             {.flags = Phdr::kRead | Phdr::kWrite,
                              .offset = kMutable1Addr,
                              .vaddr = kMutable1Addr,
                              .filesz = kMutable1Size,
                              .memsz = kMutable1Size},
                             {.flags = Phdr::kRead | Phdr::kWrite,
                              .offset = kMutable2Addr,
                              .vaddr = kMutable2Addr,
                              .filesz = kMutable2Size,
                              .memsz = kMutable2Size},
                             {.flags = Phdr::kRead | Phdr::kWrite,
                              .offset = kMutable3Addr,
                              .vaddr = kMutable3Addr,
                              .filesz = kMutable3Size,
                              .memsz = kMutable3Size}};
  constexpr size_type kFileSize = std::span{kPhdrs}.back().offset + std::span{kPhdrs}.back().memsz;

  LoadInfo info;
  for (const Phdr phdr : kPhdrs) {
    ASSERT_TRUE(info.AddSegment(diag, kPageSize, phdr, false));
  }
  ASSERT_EQ(info.segments().size(), 4u);

  // Fill up the file with values.
  zx::vmo file_vmo;
  ASSERT_EQ(zx::vmo::create(kFileSize, 0, &file_vmo), ZX_OK);
  constexpr auto unrelocated_value = [](size_type offset) -> size_type {
    return offset ^ static_cast<size_type>(0xdeadbeef);
  };
  for (size_type offset = 0; offset < kFileSize; offset += sizeof(size_type)) {
    const size_type value = unrelocated_value(offset);
    ASSERT_EQ(file_vmo.write(&value, offset, sizeof(value)), ZX_OK);
  }

  elfldltl::LoadInfoMutableMemory mutable_memory{
      diag,
      info,
      elfldltl::SegmentWithVmo::GetMutableMemory<LoadInfo>{file_vmo.borrow()},
  };
  ASSERT_TRUE(mutable_memory.Init());

  // Do some stores in the first mutable segment.
  constexpr size_type kMutable1StoreAt = kMutable1Addr + 0x100;
  constexpr size_type kMutable1StoreAddAt = kMutable1Addr + 0x200;
  ASSERT_TRUE(mutable_memory.Store<size_type>(kMutable1StoreAt, ~kMutable1StoreAt));
  ASSERT_TRUE(mutable_memory.StoreAdd<size_type>(kMutable1StoreAddAt, 0x1234));

  // Do some stores in the third mutable segment.
  constexpr size_type kMutable3StoreAt = kMutable3Addr + 0x100;
  constexpr size_type kMutable3StoreAddAt = kMutable3Addr + 0x200;
  ASSERT_TRUE(mutable_memory.Store<size_type>(kMutable3StoreAt, ~kMutable3StoreAt));
  ASSERT_TRUE(mutable_memory.StoreAdd<size_type>(kMutable3StoreAddAt, 0x5678));

  // Verify the file VMO was never changed.
  for (size_type offset = 0; offset < kFileSize; offset += sizeof(size_type)) {
    size_type value;
    ASSERT_EQ(file_vmo.read(&value, offset, sizeof(value)), ZX_OK);
    EXPECT_EQ(value, unrelocated_value(offset)) << " at offset " << offset;
  }

  // The second mutable segment wasn't touched, so should not have a VMO.
  EXPECT_FALSE(std::get<LoadInfo::DataSegment>(info.segments()[2]).vmo());

  // Verify the contents of the first mutable VMO: all the same as the file
  // VMO, except for the words at kMutable1StoreAt and kMutable1StoreAddAt.
  zx::unowned_vmo mutable1_vmo{
      std::get<LoadInfo::DataSegment>(info.segments()[1]).vmo(),
  };
  ASSERT_TRUE(mutable1_vmo->is_valid());
  for (size_type offset = 0; offset < kMutable1Size; offset += sizeof(size_type)) {
    const size_type addr = kMutable1Addr + offset;
    size_type value;
    ASSERT_EQ(mutable1_vmo->read(&value, offset, sizeof(value)), ZX_OK);
    switch (addr) {
      default:
        EXPECT_EQ(value, unrelocated_value(addr))
            << " at offset " << offset << " for address " << addr;
        break;
      case kMutable1StoreAt:
        EXPECT_EQ(value, ~kMutable1StoreAt);
        break;
      case kMutable1StoreAddAt:
        EXPECT_EQ(value, unrelocated_value(kMutable1StoreAddAt) + 0x1234);
        break;
    }
  }

  // Verify the contents of the third mutable VMO: all the same as the file
  // VMO, except for the words at kMutable1StoreAt and kMutable1StoreAddAt.
  zx::unowned_vmo mutable3_vmo{
      std::get<LoadInfo::DataSegment>(info.segments()[3]).vmo(),
  };
  ASSERT_TRUE(mutable3_vmo->is_valid());
  for (size_type offset = 0; offset < kMutable3Size; offset += sizeof(size_type)) {
    const size_type addr = kMutable3Addr + offset;
    size_type value;
    ASSERT_EQ(mutable3_vmo->read(&value, offset, sizeof(value)), ZX_OK);
    switch (addr) {
      default:
        EXPECT_EQ(value, unrelocated_value(addr))
            << " at offset " << offset << " for address " << addr;
        break;
      case kMutable3StoreAt:
        EXPECT_EQ(value, ~kMutable3StoreAt);
        break;
      case kMutable3StoreAddAt:
        EXPECT_EQ(value, unrelocated_value(kMutable3StoreAddAt) + 0x5678);
        break;
    }
  }
}

}  // namespace
