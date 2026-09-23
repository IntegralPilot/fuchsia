// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/storage/gpt_utils.h>

#include <gtest/gtest.h>

#include "src/firmware/lib/zircon_boot/test/test_data/test_images.h"
#include "test_utils.h"

namespace {
constexpr size_t kBlockSize = 512;

TEST(FuchsiaFirmwareStorageGptUtilsTest, FuchsiaFirmwareStorageSyncGptPrimary) {
  TestFuchsiaFirmwareStorage test_storage(sizeof(kTestGptDisk), kBlockSize);
  memcpy(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk));
  auto storage = test_storage.GetFuchsiaFirmwareStorage();
  // Corrupt primary header.
  test_storage.buffer().data()[kBlockSize]++;
  GptData gpt_data;
  ASSERT_TRUE(FuchsiaFirmwareStorageSyncGpt(&storage, &gpt_data));
  ASSERT_TRUE(FuchsiaFirmwareStorageFreeGptData(&storage, &gpt_data));
  ASSERT_EQ(memcmp(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk)), 0);
}

TEST(FuchsiaFirmwareStorageGptUtilsTest, FuchsiaFirmwareStorageSyncGptBackup) {
  TestFuchsiaFirmwareStorage test_storage(sizeof(kTestGptDisk), kBlockSize);
  memcpy(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk));
  auto storage = test_storage.GetFuchsiaFirmwareStorage();
  // Corrupt backup header.
  test_storage.buffer().data()[sizeof(kTestGptDisk) - kBlockSize]++;
  GptData gpt_data;
  ASSERT_TRUE(FuchsiaFirmwareStorageSyncGpt(&storage, &gpt_data));
  ASSERT_TRUE(FuchsiaFirmwareStorageFreeGptData(&storage, &gpt_data));
  ASSERT_EQ(memcmp(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk)), 0);
}

// See src/firmware/lib/zircon_boot/test/test_data/generate_test_data.py for how kTestGptDisk is
// generated.
constexpr size_t kTestGptZirconAOffset = 34 * kBlockSize;
constexpr size_t kTestGptZirconSize = 4 * kBlockSize;
constexpr size_t kTestGptZirconBOffset = 38 * kBlockSize;

void FuchsiaFirmwareStorageGptTestHelper(const char* part_name, size_t offset, size_t size,
                                         size_t partition_offset) {
  std::string payload(kBlockSize, 'a');
  TestFuchsiaFirmwareStorage test_storage(sizeof(kTestGptDisk), kBlockSize);
  memcpy(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk));
  auto storage = test_storage.GetFuchsiaFirmwareStorage();

  GptData gpt_data;
  ASSERT_TRUE(FuchsiaFirmwareStorageSyncGpt(&storage, &gpt_data));
  auto cleanup = fit::defer([&gpt_data, &storage]() {
    ASSERT_TRUE(FuchsiaFirmwareStorageFreeGptData(&storage, &gpt_data));
  });

  ASSERT_TRUE(
      FuchsiaFirmwareStorageGptWrite(&storage, &gpt_data, part_name, offset, size, payload.data()));

  std::vector<uint8_t> expected = test_storage.buffer();
  memcpy(expected.data() + partition_offset + offset, payload.data(), size);
  ASSERT_EQ(expected, test_storage.buffer());

  // Read back the dtaa for test.
  std::string read_content(payload.size(), 0);
  ASSERT_TRUE(FuchsiaFirmwareStorageGptRead(&storage, &gpt_data, part_name, offset, size,
                                            read_content.data()));
  ASSERT_EQ(read_content, payload);
}

TEST(FuchsiaFirmwareStorageGptTest, ReadWriteZirconA) {
  // Pass offset/size in the middle of the partition to test that parameter forwarding is correctly.
  FuchsiaFirmwareStorageGptTestHelper("zircon_a", kBlockSize, kBlockSize, kTestGptZirconAOffset);
}

TEST(FuchsiaFirmwareStorageGptTest, ReadWriteZirconB) {
  FuchsiaFirmwareStorageGptTestHelper("zircon_b", kBlockSize, kBlockSize, kTestGptZirconBOffset);
}

TEST(FuchsiaFirmwareStorageGptTest, Nonexist) {
  TestFuchsiaFirmwareStorage test_storage(sizeof(kTestGptDisk), kBlockSize);
  memcpy(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk));
  auto storage = test_storage.GetFuchsiaFirmwareStorage();

  GptData gpt_data;
  ASSERT_TRUE(FuchsiaFirmwareStorageSyncGpt(&storage, &gpt_data));
  auto cleanup = fit::defer([&gpt_data, &storage]() {
    ASSERT_TRUE(FuchsiaFirmwareStorageFreeGptData(&storage, &gpt_data));
  });

  std::string payload(kBlockSize, 'a');
  ASSERT_FALSE(FuchsiaFirmwareStorageGptWrite(&storage, &gpt_data, "non-exist", kBlockSize,
                                              kBlockSize, payload.data()));
}

TEST(FuchsiaFirmwareStorageGptTest, Overflow) {
  TestFuchsiaFirmwareStorage test_storage(sizeof(kTestGptDisk), kBlockSize);
  memcpy(test_storage.buffer().data(), kTestGptDisk, sizeof(kTestGptDisk));
  auto storage = test_storage.GetFuchsiaFirmwareStorage();

  GptData gpt_data;
  ASSERT_TRUE(FuchsiaFirmwareStorageSyncGpt(&storage, &gpt_data));
  auto cleanup = fit::defer([&gpt_data, &storage]() {
    ASSERT_TRUE(FuchsiaFirmwareStorageFreeGptData(&storage, &gpt_data));
  });

  std::string payload(kBlockSize, 'a');
  ASSERT_FALSE(FuchsiaFirmwareStorageGptWrite(&storage, &gpt_data, "zircon_a",
                                              kTestGptZirconSize - kBlockSize + 1, kBlockSize,
                                              payload.data()));
}

}  // namespace
