// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/loadinfo-mapped-memory.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/testing/typed-test.h>

#include <gmock/gmock.h>

#include "load-tests.h"

namespace {

using elfldltl::testing::ConstantPhdr;
using elfldltl::testing::DataPhdr;
using elfldltl::testing::ExpectOkDiagnostics;
using ::testing::Eq;
using ::testing::Pointer;
using ::testing::Property;
using ::testing::Return;
using ::testing::StrictMock;

constexpr size_t kPageSize = 4096;

constexpr std::array<std::byte, 8> kData = {std::byte{0xa1}, std::byte{0xb1}, std::byte{0xc1},
                                            std::byte{0xd1}, std::byte{0xe1}, std::byte{0xf1},
                                            std::byte{0xa2}, std::byte{0xb2}};

FORMAT_TYPED_TEST_SUITE(ElfldltlLoadInfoMappedMemoryTests);

class MockDirectMemory {
 public:
  MockDirectMemory() { ON_CALL(*this, ReadBytes).WillByDefault(Return(std::span(kData))); }

  MOCK_METHOD(std::optional<std::span<const std::byte>>, ReadBytes, (uintptr_t ptr, size_t count));

  // This is a workaround for MOCK_METHOD's incompatibility with templated functions.
  // LoadInfoMappedMemory calls into this ReadArray function, which in turn calls
  // the ReadBytes MOCK_METHOD above in tests.
  template <typename T>
  std::optional<std::span<const T>> ReadArray(uintptr_t ptr, size_t count) {
    if (std::optional result = ReadBytes(ptr, count * sizeof(T))) {
      return std::span{reinterpret_cast<const T*>(result->data()),
                       result->size_bytes() / sizeof(T)};
    }
    return std::nullopt;
  }
};

// A helper that verifies the return value of MockDirectMemory::ReadBytes
// matches kData properties.
template <typename T>
constexpr auto IsExpectedData() {
  return Optional(
      AllOf(Property(&std::span<const T>::data, Pointer(reinterpret_cast<const T*>(kData.data()))),
            Property(&std::span<const T>::size, Eq(kData.size() / sizeof(T)))));
}

TYPED_TEST(ElfldltlLoadInfoMappedMemoryTests, ReadInBounds) {
  using Elf = typename TestFixture::Elf;
  using ElfLoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();
  ElfLoadInfo load_info;

  // Load three non-mergeable segments.
  size_type offset = 0;
  for (const auto& phdr :
       {ConstantPhdr<Elf>{}(offset), DataPhdr<Elf>{}(offset), ConstantPhdr<Elf>{}(offset)}) {
    ASSERT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));
  }

  StrictMock<MockDirectMemory> mock_mem;

  elfldltl::LoadInfoMappedMemory mapped_mem(load_info, mock_mem);

  // Read from first segment..., .
  EXPECT_CALL(mock_mem, ReadBytes(0, 4096)).Times(1);
  EXPECT_CALL(mock_mem, ReadBytes(2048, 2048)).Times(1);
  EXPECT_CALL(mock_mem, ReadBytes(4095, 1)).Times(1);

  // Read from second segment.
  EXPECT_CALL(mock_mem, ReadBytes(4096, 4096)).Times(1);
  EXPECT_CALL(mock_mem, ReadBytes(6144, 2048)).Times(1);
  EXPECT_CALL(mock_mem, ReadBytes(8191, 1)).Times(1);

  // Read from third segment.
  EXPECT_CALL(mock_mem, ReadBytes(8192, 4096)).Times(1);
  EXPECT_CALL(mock_mem, ReadBytes(10240, 2048)).Times(1);
  EXPECT_CALL(mock_mem, ReadBytes(12287, 1)).Times(1);

  // Test read count is correctly inferred.
  // The tests use `IsExpectedData<T>()` to test the span (adjusted for sizeof(T))
  // returned by the underlying mock memory object is passed straight through by
  // the adaptor. The actual kData (a small array of bytes) that the mock memory
  // object returns does not matter for testing the adaptor, so these tests do not
  // differentiate the data returned by the memory object when offset and count values vary.
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(0), IsExpectedData<std::byte>());
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(2048), IsExpectedData<std::byte>());
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(4095), IsExpectedData<std::byte>());

  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(4096), IsExpectedData<std::byte>());
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(6144), IsExpectedData<std::byte>());
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(8191), IsExpectedData<std::byte>());

  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(8192), IsExpectedData<std::byte>());
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(10240), IsExpectedData<std::byte>());
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(12287), IsExpectedData<std::byte>());

  // Test the explicit read count is passed through.
  EXPECT_CALL(mock_mem, ReadBytes(0, 4096)).Times(1);
  EXPECT_CALL(mock_mem, ReadBytes(4096, 4096)).Times(1);
  EXPECT_CALL(mock_mem, ReadBytes(8192, 4096)).Times(1);

  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(0, 4096), IsExpectedData<std::byte>());
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(4096, 4096),
              IsExpectedData<std::byte>());
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(8192, 4096),
              IsExpectedData<std::byte>());
}

TYPED_TEST(ElfldltlLoadInfoMappedMemoryTests, ReadSizedType) {
  using Elf = typename TestFixture::Elf;
  using ElfLoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();
  ElfLoadInfo load_info;

  // Load three non-mergeable segments with 4096 byte vaddr ranges.
  size_type offset = 0;
  for (const auto& phdr :
       {ConstantPhdr<Elf>{}(offset), DataPhdr<Elf>{}(offset), ConstantPhdr<Elf>{}(offset)}) {
    ASSERT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));
  }

  StrictMock<MockDirectMemory> mock_mem;

  elfldltl::LoadInfoMappedMemory mapped_mem(const_cast<const ElfLoadInfo&>(load_info), mock_mem);

  // Test that the available read count is correctly adjusted for 8B-sized read.
  EXPECT_CALL(mock_mem, ReadBytes(0, 4096)).Times(1);
  EXPECT_THAT(mapped_mem.template ReadArray<const uint64_t>(0), IsExpectedData<uint64_t>());

  EXPECT_CALL(mock_mem, ReadBytes(32, 4064)).Times(1);
  EXPECT_THAT(mapped_mem.template ReadArray<const uint64_t>(32), IsExpectedData<uint64_t>());

  // Test that the request count is passed through from the caller.
  EXPECT_CALL(mock_mem, ReadBytes(0, 32)).Times(1);
  EXPECT_THAT(mapped_mem.template ReadArray<const uint64_t>(0, 4), IsExpectedData<uint64_t>());

  // Expect that the request count cannot exceed availability, adjusted for sizeof(T).
  EXPECT_EQ(mapped_mem.template ReadArray<const uint64_t>(0, 1024), std::nullopt);
}

TYPED_TEST(ElfldltlLoadInfoMappedMemoryTests, ReadOutOfBounds) {
  using Elf = typename TestFixture::Elf;
  using ElfLoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();
  ElfLoadInfo load_info;

  // Load three non-mergeable segments.
  size_type offset = 0;
  for (const auto& phdr :
       {ConstantPhdr<Elf>{}(offset), DataPhdr<Elf>{}(offset), ConstantPhdr<Elf>{}(offset)}) {
    ASSERT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));
  }

  // StrictMock will fail if any method on the mock is called that is not expected.
  StrictMock<MockDirectMemory> mock_mem;

  elfldltl::LoadInfoMappedMemory mapped_mem(const_cast<const ElfLoadInfo&>(load_info), mock_mem);

  // Test vaddr that is out of bounds.
  EXPECT_EQ(mapped_mem.template ReadArray<const std::byte>(12288), std::nullopt);

  // Test size count that is out of bounds.
  EXPECT_EQ(mapped_mem.template ReadArray<const std::byte>(12287, 2), std::nullopt);

  // Test size count that would carry into the next segment.
  EXPECT_EQ(mapped_mem.template ReadArray<const std::byte>(0, 4097), std::nullopt);
}

TYPED_TEST(ElfldltlLoadInfoMappedMemoryTests, ReadSegmentGap) {
  using Elf = typename TestFixture::Elf;
  using ElfLoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();
  ElfLoadInfo load_info;

  // Load segments with a gap between vaddr ranges.
  size_type offset1 = 0;
  size_type offset2 = 8192;
  for (const auto& phdr : {ConstantPhdr<Elf>{}(offset1), ConstantPhdr<Elf>{}(offset2)}) {
    ASSERT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));
  }

  StrictMock<MockDirectMemory> mock_mem;

  elfldltl::LoadInfoMappedMemory mapped_mem(const_cast<const ElfLoadInfo&>(load_info), mock_mem);

  // Test that read from first segment addr is bounded correctly.
  EXPECT_CALL(mock_mem, ReadBytes(0, 4096)).Times(1);
  // Test that read from last segment addr is bounded correctly.
  EXPECT_CALL(mock_mem, ReadBytes(8192, 4096)).Times(1);

  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(0), IsExpectedData<std::byte>());
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(8192), IsExpectedData<std::byte>());

  // Test a vaddr that falls within the gap in the segments' vaddr ranges.
  EXPECT_EQ(mapped_mem.template ReadArray<const std::byte>(4096), std::nullopt);
  EXPECT_EQ(mapped_mem.template ReadArray<const std::byte>(6144), std::nullopt);
}

// Test that reads will not surpass the segment.filesz(), regardless of segment.memsz().
TYPED_TEST(ElfldltlLoadInfoMappedMemoryTests, FileSzBoundary) {
  using Elf = typename TestFixture::Elf;
  using ElfLoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();
  ElfLoadInfo load_info;

  // Load DataWithZeroFill segment with a filesz < memsz
  Phdr phdr{
      .type = elfldltl::ElfPhdrType::kLoad, .offset = 0, .vaddr = 0, .filesz = 2048, .memsz = 4096};
  phdr.flags = elfldltl::PhdrBase::kWrite;
  ASSERT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));

  StrictMock<MockDirectMemory> mock_mem;

  elfldltl::LoadInfoMappedMemory mapped_mem(const_cast<const ElfLoadInfo&>(load_info), mock_mem);

  // Test that read from the segment is bounded to filesz, not memsz.
  EXPECT_CALL(mock_mem, ReadBytes(0, 2048)).Times(1);
  EXPECT_THAT(mapped_mem.template ReadArray<const std::byte>(0), IsExpectedData<std::byte>());

  // Test that reading from a segment offset > filesz, but < memsz returns null.
  EXPECT_EQ(mapped_mem.template ReadArray<const std::byte>(2050), std::nullopt);
}

}  // namespace
