// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/storage/storage.h>

#include <gtest/gtest.h>

#include "test_utils.h"

namespace {

struct FuchsiaFirmwareStorageTestCase {
  size_t offset;
  size_t size;
  size_t buffer_offset;
  size_t block_size;
  size_t storage_size;

  std::string Name() const {
    static char buffer[512];
    snprintf(buffer, sizeof(buffer),
             "offset%zu_size%zu_buffer_offset%zu_block_size%zu_storage_size%zu", offset, size,
             buffer_offset, block_size, storage_size);
    return std::string(buffer);
  }
};

using FuchsiaFirmwareStorageTest = ::testing::TestWithParam<FuchsiaFirmwareStorageTestCase>;

template <size_t block_size>
struct FuchsiaFirmwareStorageTestCases {
  static constexpr size_t kStorageSize = 3 * block_size;
  static constexpr FuchsiaFirmwareStorageTestCase kTestCases[] = {
      // offset
      //   |~~~~~~~~~~~~~size~~~~~~~~~~~~|
      //   |---------|---------|---------|
      {0, kStorageSize, 0, block_size, kStorageSize},
      // offset
      //   |~~~~~~~~~size~~~~~~~~~|
      //   |---------|---------|---------|
      {0, kStorageSize - 1, 0, block_size, kStorageSize},
      // offset
      //   |~~size~~|
      //   |---------|---------|---------|
      {0, block_size - 1, 0, block_size, kStorageSize},
      //     offset
      //       |~~~~~~~~~~~size~~~~~~~~~~|
      //   |---------|---------|---------|
      {1, kStorageSize - 1, 0, block_size, kStorageSize},
      //     offset
      //       |~~~~~~~~~size~~~~~~~~|
      //   |---------|---------|---------|
      {1, kStorageSize - 2, 0, block_size, kStorageSize},
      //     offset
      //       |~~~size~~~|
      //   |---------|---------|---------|
      {1, block_size, 0, block_size, kStorageSize},
      //   offset
      //     |~size~|
      //   |---------|---------|---------|
      {1, block_size - 2, 0, block_size, kStorageSize},

      // Run the same with an additional offset of one block
      {block_size, kStorageSize, 0, block_size, kStorageSize + block_size},
      {block_size, kStorageSize - 1, 0, block_size, kStorageSize + block_size},
      {block_size, block_size - 1, 0, block_size, kStorageSize + block_size},
      {block_size + 1, kStorageSize - 1, 0, block_size, kStorageSize + block_size},
      {block_size + 1, kStorageSize - 2, 0, block_size, kStorageSize + block_size},
      {block_size + 1, block_size, 0, block_size, kStorageSize + block_size},
      {block_size + 1, block_size - 2, 0, block_size, kStorageSize + block_size},

      // Run the same with unaligned output address
      {0, kStorageSize, 1, block_size, kStorageSize},
      {0, kStorageSize - 1, 1, block_size, kStorageSize},
      {0, block_size - 1, 1, block_size, kStorageSize},
      {1, kStorageSize - 1, 1, block_size, kStorageSize},
      {1, kStorageSize - 2, 1, block_size, kStorageSize},
      {1, block_size, 1, block_size, kStorageSize},
      {1, block_size - 2, 1, block_size, kStorageSize},

      // A special case where the value of `offset` is not block aligned but DMA aligned. This can
      // trigger some internal optimization code path.
      {FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT,
       kStorageSize - FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT, 0, block_size, kStorageSize},
      {FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT,
       kStorageSize - FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT - 1, 0, block_size, kStorageSize},
      {FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT, block_size, 0, block_size, kStorageSize},
      {FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT,
       block_size - FUCHSIA_FIRMWARE_STORAGE_BUFFER_ALIGNMENT - 1, 0, block_size, kStorageSize},
  };
};

INSTANTIATE_TEST_SUITE_P(
    FuchsiaFirmwareStorageTestsBlockSize512, FuchsiaFirmwareStorageTest,
    testing::ValuesIn<FuchsiaFirmwareStorageTestCase>(
        FuchsiaFirmwareStorageTestCases<512>::kTestCases),
    [](const testing::TestParamInfo<FuchsiaFirmwareStorageTest::ParamType>& info) {
      return info.param.Name();
    });

INSTANTIATE_TEST_SUITE_P(
    FuchsiaFirmwareStorageTestsBlockSize4096, FuchsiaFirmwareStorageTest,
    testing::ValuesIn<FuchsiaFirmwareStorageTestCase>(
        FuchsiaFirmwareStorageTestCases<4096>::kTestCases),
    [](const testing::TestParamInfo<FuchsiaFirmwareStorageTest::ParamType>& info) {
      return info.param.Name();
    });

TEST_P(FuchsiaFirmwareStorageTest, FuchsiaFirmwareStorageReadTestParameterized) {
  const FuchsiaFirmwareStorageTestCase& test_case = GetParam();
  TestFuchsiaFirmwareStorage storage(test_case.storage_size, test_case.block_size);
  FuchsiaFirmwareStorage ops = storage.GetFuchsiaFirmwareStorage();
  AlignedBuffer read_buffer(test_case.size + test_case.buffer_offset);
  ASSERT_TRUE(FuchsiaFirmwareStorageRead(&ops, test_case.offset, test_case.size,
                                         read_buffer.get() + test_case.buffer_offset));
  ASSERT_EQ(memcmp(storage.buffer().data() + test_case.offset,
                   read_buffer.get() + test_case.buffer_offset, test_case.size),
            0);
}

template <typename F>
void FuchsiaFirmwareStorageWriteTest(const FuchsiaFirmwareStorageTestCase& test_case, F func) {
  TestFuchsiaFirmwareStorage storage(test_case.storage_size, test_case.block_size);
  FuchsiaFirmwareStorage ops = storage.GetFuchsiaFirmwareStorage();

  size_t total_buffer_size = test_case.size + test_case.buffer_offset;
  AlignedBuffer write_buffer(total_buffer_size);
  auto write_start = write_buffer.get() + test_case.buffer_offset;

  // Initialize data to write. Take the content on storage and reverse it.
  memcpy(write_start, storage.buffer().data() + test_case.offset, test_case.size);
  std::reverse(write_start, write_start + test_case.size);

  // Make a copy of the data to write. This is for checking API does not modify input.
  std::vector<uint8_t> original(write_buffer.get(), write_buffer.get() + total_buffer_size);

  // Construct expected data on storage
  std::vector<uint8_t> expected = storage.buffer();
  memcpy(expected.data() + test_case.offset, write_start, test_case.size);
  ASSERT_TRUE(func(&ops, test_case.offset, test_case.size, write_start));
  ASSERT_EQ(expected, storage.buffer());

  // Input buffer should not be changed.
  ASSERT_EQ(memcmp(write_buffer.get(), original.data(), total_buffer_size), 0);
}

TEST_P(FuchsiaFirmwareStorageTest, FuchsiaFirmwareStorageWriteConstTestParameterized) {
  const FuchsiaFirmwareStorageTestCase& test_case = GetParam();
  auto function = [](FuchsiaFirmwareStorage* ops, size_t offset, size_t size, const void* src) {
    return FuchsiaFirmwareStorageWriteConst(ops, offset, size, src);
  };
  FuchsiaFirmwareStorageWriteTest<>(test_case, function);
}

TEST_P(FuchsiaFirmwareStorageTest, FuchsiaFirmwareStorageWriteTestParameterized) {
  const FuchsiaFirmwareStorageTestCase& test_case = GetParam();
  auto function = [](FuchsiaFirmwareStorage* ops, size_t offset, size_t size, void* src) {
    return FuchsiaFirmwareStorageWrite(ops, offset, size, src);
  };
  FuchsiaFirmwareStorageWriteTest<>(test_case, function);
}

}  // namespace
