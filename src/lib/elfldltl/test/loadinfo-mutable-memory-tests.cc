// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/load.h>
#include <lib/elfldltl/loadinfo-mutable-memory.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/testing/typed-test.h>

#include <cstdint>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "load-tests.h"

namespace {

using ::testing::AllOf;
using ::testing::Eq;
using ::testing::InSequence;
using ::testing::Optional;
using ::testing::Property;
using ::testing::Return;
using ::testing::SizeIs;
using ::testing::StrictMock;

NATIVE_FORMAT_TYPED_TEST_SUITE(ElfldltlLoadInfoMutableMemoryTests);

// This provides the underlying Memory object that the GetMutableMemory
// function returns.  The Memory object is a potentially-owning object that is
// returned by value.  But gmock classes can't be moved, so the Memory API
// object is a copyable proxy object that calls the mocked methods.
class MockMemory {
 public:
  MOCK_METHOD(bool, Store, (size_t size, uintptr_t ptr, uintmax_t value));
  MOCK_METHOD(bool, StoreAdd, (size_t size, uintptr_t ptr, uintmax_t value));
  MOCK_METHOD(std::optional<std::span<const std::byte>>, ReadArray,
              (size_t size, uintptr_t ptr, std::optional<size_t> count));

  class Memory;
};

using StrictMockMemory = StrictMock<MockMemory>;

template <typename T, bool Ok>
void ExpectStore(StrictMockMemory& mock, uintptr_t ptr, uintmax_t value) {
  EXPECT_CALL(mock, Store(sizeof(T), ptr, value)).WillOnce(Return(Ok));
}

template <typename T, bool Ok>
void ExpectStoreAdd(StrictMockMemory& mock, uintptr_t ptr, uintmax_t value) {
  EXPECT_CALL(mock, StoreAdd(sizeof(T), ptr, value)).WillOnce(Return(Ok));
}

template <typename T>
void ExpectReadArray(StrictMockMemory& mock, uintptr_t ptr, std::optional<size_t> count,
                     std::optional<std::span<const T>> result) {
  EXPECT_CALL(mock, ReadArray(sizeof(T), ptr, count))
      .WillOnce(Return(result ? std::make_optional(std::as_bytes(*result)) : std::nullopt));
}

class MockMemory::Memory {
 public:
  explicit Memory(StrictMockMemory& mock) : mock_(mock) {}

  template <typename T, typename U>
  bool Store(uintptr_t ptr, U value) {
    return mock_.Store(sizeof(T), ptr, value);
  }

  template <typename T, typename U>
  bool StoreAdd(uintptr_t ptr, U value) {
    return mock_.StoreAdd(sizeof(T), ptr, value);
  }

  template <typename T>
  std::optional<std::span<const T>> ReadArray(uintptr_t ptr,
                                              std::optional<size_t> count = std::nullopt) {
    if (auto bytes = mock_.ReadArray(sizeof(T), ptr, count)) {
      return std::span{
          reinterpret_cast<const T*>(bytes->data()),
          bytes->size() / sizeof(T),
      };
    }
    return std::nullopt;
  }

 private:
  StrictMockMemory& mock_;
};

// This holds the expectations set with ExpectGet(...) below.  MockGet (below)
// produces the GetMutableMemory callable object that uses it.
class MockGetMutableMemory {
 public:
  using Result = fit::result<bool, MockMemory::Memory>;

  MOCK_METHOD(Result, Get, (uintptr_t vaddr, size_t filesz));
};

using StrictMockGetMutableMemory = StrictMock<MockGetMutableMemory>;

void ExpectGet(StrictMockGetMutableMemory& mock_get, uintptr_t vaddr, size_t filesz,
               StrictMockMemory& mock_memory) {
  EXPECT_CALL(mock_get, Get(vaddr, filesz))
      .WillOnce(Return(MockGetMutableMemory::Result{fit::ok(MockMemory::Memory{mock_memory})}));
}

void ExpectGet(StrictMockGetMutableMemory& mock_get, uintptr_t vaddr, size_t filesz,
               bool keep_going, int repeat = 1) {
  EXPECT_CALL(mock_get, Get(vaddr, filesz))
      .Times(repeat)
      .WillRepeatedly(Return(MockGetMutableMemory::Result{fit::error{keep_going}}));
}

// This provides the GetMutableMemory callable signature and turns it into
// a call to the mockable method.
auto MockGet(StrictMockGetMutableMemory& mock) {
  return [&mock](auto& diag, const auto& segment) {
    auto get = [&mock](const auto& segment) -> MockGetMutableMemory::Result {
      return mock.Get(segment.vaddr(), segment.filesz());
    };
    return std::visit(get, segment);
  };
}

// Populate a LoadInfo with the common LLD layout.
template <class Elf>
auto MakeLoadInfo() {
  return elfldltl::testing::TestLoadInfo<  //
      Elf,
      elfldltl::testing::ConstantPhdr,          // page 0: RODATA
      elfldltl::testing::ConstantPhdr,          // page 1: CODE
      elfldltl::testing::DataPhdr,              // page 2: RELRO
      elfldltl::testing::DataWithZeroFillPhdr>  // page 3: DATA (+ bss)
                                                // Don't merge RELRO into DATA.
      (false);
}

constexpr uintptr_t kRodataAddr = elfldltl::testing::kPageSize / 2;
constexpr uintptr_t kCodeAddr = elfldltl::testing::kPageSize + (elfldltl::testing::kPageSize / 2);
constexpr uintptr_t kRelroStart = elfldltl::testing::kPageSize * 2;
constexpr uintptr_t kRelroMiddle = kRelroStart + (elfldltl::testing::kPageSize / 2);
constexpr uintptr_t kDataStart = elfldltl::testing::kPageSize * 3;
constexpr uintptr_t kDataMiddle = kDataStart + (elfldltl::testing::kPageSize / 2);

// Test cases where the get_mutable_memory callback should never be made.
TYPED_TEST(ElfldltlLoadInfoMutableMemoryTests, NoGet) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;

  auto load_info = MakeLoadInfo<Elf>();

  InSequence seq;

  StrictMockGetMutableMemory mock_get;
  auto get_mutable_memory = MockGet(mock_get);

  auto bad_addr_diag = [](uintptr_t addr) {
    return elfldltl::testing::ExpectedSingleError{
        "invalid relocation for ", static_cast<size_type>(sizeof(size_type)), " bytes",
        elfldltl::FileAddress{static_cast<size_type>(addr)}};
  };

  // A non-writable segment should fail with no callbacks at all.  Note that
  // the ExpectedSingleError diagnostics object always returns true to see more
  // errors, so that's the result we'll see from the adapter's methods.

  {
    auto bad_addr = bad_addr_diag(kRodataAddr);
    elfldltl::LoadInfoMutableMemory memory{bad_addr, load_info, get_mutable_memory};
    ASSERT_TRUE(memory.Init());
    EXPECT_TRUE(memory.template Store<size_type>(kRodataAddr, 0x1234));
  }
  {
    auto bad_addr = bad_addr_diag(kCodeAddr);
    elfldltl::LoadInfoMutableMemory memory{bad_addr, load_info, get_mutable_memory};
    ASSERT_TRUE(memory.Init());
    EXPECT_TRUE(memory.template Store<size_type>(kCodeAddr, 0x5678));
  }

  // Same for an address straddling the writable segment boundaries.

  {
    constexpr uintptr_t kBoundary = kRelroStart - (sizeof(size_type) / 2);
    auto bad_addr = bad_addr_diag(kBoundary);
    elfldltl::LoadInfoMutableMemory memory{bad_addr, load_info, get_mutable_memory};
    ASSERT_TRUE(memory.Init());
    EXPECT_TRUE(memory.template Store<size_type>(kBoundary, 0x1234));
  }
  {
    constexpr uintptr_t kBoundary = kDataStart - (sizeof(size_type) / 2);
    auto bad_addr = bad_addr_diag(kBoundary);
    elfldltl::LoadInfoMutableMemory memory{bad_addr, load_info, get_mutable_memory};
    ASSERT_TRUE(memory.Init());
    EXPECT_TRUE(memory.template Store<size_type>(kBoundary, 0x1234));
  }
}

// Test cases where the get_mutable_memory callback returns failure.
TYPED_TEST(ElfldltlLoadInfoMutableMemoryTests, GetFail) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;

  auto load_info = MakeLoadInfo<Elf>();

  InSequence seq;

  StrictMockGetMutableMemory mock_get;
  auto get_mutable_memory = MockGet(mock_get);

  // Note that the diag object always returns false when it's called, but also
  // registers failure if it's ever called.  The adapter should never report a
  // diagnostic for the get_mutable_memory call failing, it just returns its
  // bool value on the expectation that it has done its own diagnostics.  The
  // callback created by MockGet just ignores the diagnostics object and
  // returns the bool error value in the expectation.
  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  elfldltl::LoadInfoMutableMemory memory{diag, load_info, get_mutable_memory};
  ASSERT_TRUE(memory.Init());

  // The Boolean error value from a failing get_mutable_memory callback should
  // propagate back to the caller of Store / StoreAdd.

  ExpectGet(mock_get, kRelroStart, elfldltl::testing::kPageSize, true, 2);
  EXPECT_TRUE(memory.template Store<size_type>(kRelroMiddle, 0x1234));
  EXPECT_TRUE(memory.template StoreAdd<size_type>(kRelroMiddle, 0x5678));

  ExpectGet(mock_get, kDataStart, elfldltl::testing::kPageSize, false, 2);
  EXPECT_FALSE(memory.template Store<size_type>(kDataStart, 0xabcd));
  EXPECT_FALSE(memory.template StoreAdd<size_type>(kDataStart, 0xedf0));

  // The ReadArray methods have to return std::nullopt for any failure
  // regardless of the Diagnostics object's Boolean return value.
  ExpectGet(mock_get, kDataStart, elfldltl::testing::kPageSize, true, 2);
  EXPECT_EQ(memory.template ReadArray<size_type>(kDataMiddle, 1), std::nullopt);
  EXPECT_EQ(memory.template ReadArray<size_type>(kDataMiddle), std::nullopt);
}

// Test cases where the a Memory object is returned and used.
TYPED_TEST(ElfldltlLoadInfoMutableMemoryTests, Store) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;

  auto load_info = MakeLoadInfo<Elf>();

  InSequence seq;

  StrictMockGetMutableMemory mock_get;
  auto get_mutable_memory = MockGet(mock_get);

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  elfldltl::LoadInfoMutableMemory memory{diag, load_info, get_mutable_memory};
  ASSERT_TRUE(memory.Init());

  StrictMockMemory mock_memory;

  // The Boolean value from the underlying Memory method call should propagate
  // back to the caller of Store / StoreAdd.

  ExpectGet(mock_get, kRelroStart, elfldltl::testing::kPageSize, mock_memory);
  ExpectStore<size_type, true>(mock_memory, kRelroStart, 0x1234);
  ExpectStoreAdd<size_type, true>(mock_memory, kRelroMiddle, 0x5678);
  EXPECT_TRUE(memory.template Store<size_type>(kRelroStart, 0x1234));
  EXPECT_TRUE(memory.template StoreAdd<size_type>(kRelroMiddle, 0x5678));

  ExpectGet(mock_get, kDataStart, elfldltl::testing::kPageSize, mock_memory);
  ExpectStore<size_type, false>(mock_memory, kDataStart, 0xabcd);
  ExpectStoreAdd<size_type, false>(mock_memory, kDataMiddle, 0xedf0);
  EXPECT_FALSE(memory.template Store<size_type>(kDataStart, 0xabcd));
  EXPECT_FALSE(memory.template StoreAdd<size_type>(kDataMiddle, 0xedf0));
}

// Test the ReadArray methods.
TYPED_TEST(ElfldltlLoadInfoMutableMemoryTests, ReadArray) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;

  auto load_info = MakeLoadInfo<Elf>();

  InSequence seq;

  StrictMockGetMutableMemory mock_get;
  auto get_mutable_memory = MockGet(mock_get);

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  elfldltl::LoadInfoMutableMemory memory{diag, load_info, get_mutable_memory};
  ASSERT_TRUE(memory.Init());

  StrictMockMemory mock_memory;

  static constexpr size_type kData[] = {0x1234, 0x5678};
  constexpr std::span<const size_type> kDataSpan{kData};

  ExpectGet(mock_get, kDataStart, elfldltl::testing::kPageSize, mock_memory);
  ExpectReadArray<size_type>(mock_memory, kDataStart, 1, kDataSpan.first(1));
  ExpectReadArray<size_type>(mock_memory, kDataStart, std::nullopt, kDataSpan);
  ExpectReadArray<size_type>(mock_memory, kDataMiddle, 1, std::nullopt);
  ExpectReadArray<size_type>(mock_memory, kDataMiddle, std::nullopt, std::nullopt);
  EXPECT_THAT(memory.template ReadArray<size_type>(kDataStart, 1),
              Optional(AllOf(SizeIs(1),
                             Property(&std::span<const size_type>::data, Eq(kDataSpan.data())))));
  EXPECT_THAT(memory.template ReadArray<size_type>(kDataStart),
              Optional(AllOf(SizeIs(2),
                             Property(&std::span<const size_type>::data, Eq(kDataSpan.data())))));
  EXPECT_EQ(memory.template ReadArray<size_type>(kDataMiddle, 1), std::nullopt);
  EXPECT_EQ(memory.template ReadArray<size_type>(kDataMiddle), std::nullopt);
}

// Verify that memory accesses ending at the exact final byte of an ELF segment
// (vaddr + filesz) are treated as inclusive and accepted.
TYPED_TEST(ElfldltlLoadInfoMutableMemoryTests, StoreAtSegmentEnd) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;

  auto load_info = MakeLoadInfo<Elf>();

  InSequence seq;

  StrictMockGetMutableMemory mock_get;
  auto get_mutable_memory = MockGet(mock_get);

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  elfldltl::LoadInfoMutableMemory memory{diag, load_info, get_mutable_memory};
  ASSERT_TRUE(memory.Init());

  StrictMockMemory mock_memory;

  // Calculate the address of the very last word fitting inside the DATA segment.
  constexpr uintptr_t kDataSegmentLastWord =
      kDataStart + elfldltl::testing::kPageSize - sizeof(size_type);

  ExpectGet(mock_get, kDataStart, elfldltl::testing::kPageSize, mock_memory);
  ExpectStore<size_type, true>(mock_memory, kDataSegmentLastWord, 0x1234);

  // Storing at the exact end of the segment must succeed without out-of-bounds errors.
  EXPECT_TRUE(memory.template Store<size_type>(kDataSegmentLastWord, 0x1234));
}

}  // namespace
