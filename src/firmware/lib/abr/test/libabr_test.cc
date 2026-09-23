// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/abr/abr.h>
#include <lib/abr/data.h>
#include <lib/abr/util.h>
#include <lib/cksum.h>

#include <string>

#include <zxtest/zxtest.h>

namespace {

extern "C" {
// A CRC32 implementation for the test environment.
uint32_t AbrCrc32(const void* buf, size_t buf_size) {
  return crc32(0UL, reinterpret_cast<const uint8_t*>(buf), buf_size);
}

// AVBOps implementations which forward to a LibabrTest instance.
bool FakeReadAbrMetadata(void* context, size_t size, uint8_t* buffer);
bool FakeWriteAbrMetadata(void* context, const uint8_t* buffer, size_t size);
bool FakeReadAbrMetadataCustom(void* context, AbrSlotData* a, AbrSlotData* b,
                               uint8_t* one_shot_flags);
bool FakeWriteAbrMetadataCustom(void* context, const AbrSlotData* a, const AbrSlotData* b,
                                uint8_t one_shot_flags);
}  // extern "C"

// Call this after messing with metadata (if you want the CRC to match).
void UpdateMetadataCRC(AbrData* metadata) {
  metadata->crc32 = AbrHostToBigEndian(AbrCrc32(metadata, sizeof(*metadata) - sizeof(uint32_t)));
}

// Initializes metadata to a valid state where both slots are unbootable.
void InitializeMetadata(AbrData* metadata) {
  memset(metadata, 0, sizeof(*metadata));
  memcpy(metadata->magic, kAbrMagic, kAbrMagicLen);
  metadata->version_major = kAbrMajorVersion;
  metadata->version_minor = kAbrMinorVersion;
  metadata->one_shot_flags = kAbrDataOneShotFlagNone;
  UpdateMetadataCRC(metadata);
}

// Checks that metadata is valid and normalized. These conditions should always be true after
// libabr has updated the metadata, even if previous metadata was invalid.
void ValidateMetadata(const AbrData& metadata) {
  EXPECT_EQ(0, memcmp(metadata.magic, kAbrMagic, kAbrMagicLen));
  EXPECT_EQ(AbrBigEndianToHost(metadata.crc32),
            AbrCrc32(&metadata, sizeof(metadata) - sizeof(uint32_t)));
  EXPECT_EQ(kAbrMajorVersion, metadata.version_major);
  EXPECT_EQ(kAbrMinorVersion, metadata.version_minor);

  for (auto slot_index : {kAbrSlotIndexA, kAbrSlotIndexB}) {
    const AbrSlotData& slot = metadata.slot_data[slot_index];

    // Priority and tries_remaining must be in range.
    EXPECT_LE(slot.priority, kAbrMaxPriority);
    EXPECT_LE(slot.tries_remaining, kAbrMaxTriesRemaining);

    // A bootable slot should never have an unbootable reason, but an unbootable slot may have a
    // None reason (e.g. when coming from earlier A/B/R versions that didn't track the reason).
    if (slot.successful_boot || slot.tries_remaining) {
      EXPECT_EQ(slot.unbootable_reason, kAbrUnbootableReasonNone);
    }
  }
}

struct FakeOps {
  operator const AbrOps*() const { return &ops_; }

  // AbrOps calls to |read_abr_metadata| forward here.
  bool ReadMetadata(size_t size, uint8_t* buffer) {
    read_metadata_count_++;
    EXPECT_EQ(size, sizeof(AbrData));
    if (size != sizeof(AbrData)) {
      return false;
    }
    memcpy(buffer, &metadata_, sizeof(metadata_));
    return read_metadata_result_;
  }

  // AbrOps calls to |write_abr_metadata| forward here.
  bool WriteMetadata(const uint8_t* buffer, size_t size) {
    write_metadata_count_++;
    EXPECT_EQ(size, sizeof(AbrData));
    if (size != sizeof(AbrData)) {
      return false;
    }
    memcpy(&metadata_, buffer, sizeof(metadata_));
    return write_metadata_result_;
  }

  bool ReadMetadataCustom(AbrSlotData* a, AbrSlotData* b, uint8_t* one_shot_flags) {
    *a = metadata_.slot_data[0];
    *b = metadata_.slot_data[1];
    *one_shot_flags = metadata_.one_shot_flags;
    read_metadata_custom_count_++;
    return read_metadata_result_;
  }

  bool WriteMetadataCustom(const AbrSlotData* a, const AbrSlotData* b, uint8_t one_shot_flags) {
    metadata_.slot_data[0] = *a;
    metadata_.slot_data[1] = *b;
    metadata_.one_shot_flags = one_shot_flags;
    write_metadata_custom_count_++;
    return write_metadata_result_;
  }

  // Set these to false in a test to induce I/O errors.
  bool read_metadata_result_ = true;
  bool write_metadata_result_ = true;
  // These will be incremented on every AbrOps call from libabr.
  int read_metadata_count_ = 0;
  int write_metadata_count_ = 0;
  int read_metadata_custom_count_ = 0;
  int write_metadata_custom_count_ = 0;
  // This will be used as the 'stored' metadata for all AbrOps callbacks.
  AbrData metadata_{};
  // This will be used as the AbrOps argument for libabr calls.
  AbrOps ops_ = {this, FakeReadAbrMetadata, FakeWriteAbrMetadata, nullptr, nullptr};
};

FakeOps FakeOpsWithInitializedMetadata() {
  FakeOps ops;
  InitializeMetadata(&ops.metadata_);
  return ops;
}

FakeOps FakeOpsCustom() {
  FakeOps ops;
  ops.ops_.read_abr_metadata = nullptr;
  ops.ops_.write_abr_metadata = nullptr;
  ops.ops_.read_abr_metadata_custom = FakeReadAbrMetadataCustom;
  ops.ops_.write_abr_metadata_custom = FakeWriteAbrMetadataCustom;
  return ops;
}

AbrSlotIndex OtherSlot(AbrSlotIndex slot_index) {
  EXPECT_NE(kAbrSlotIndexR, slot_index);
  return (slot_index == kAbrSlotIndexA) ? kAbrSlotIndexB : kAbrSlotIndexA;
}

// These callbacks forward to a FakeOps instance.
bool FakeReadAbrMetadata(void* context, size_t size, uint8_t* buffer) {
  return reinterpret_cast<FakeOps*>(context)->ReadMetadata(size, buffer);
}

bool FakeWriteAbrMetadata(void* context, const uint8_t* buffer, size_t size) {
  return reinterpret_cast<FakeOps*>(context)->WriteMetadata(buffer, size);
}

bool FakeReadAbrMetadataCustom(void* context, AbrSlotData* a, AbrSlotData* b,
                               uint8_t* one_shot_flags) {
  return reinterpret_cast<FakeOps*>(context)->ReadMetadataCustom(a, b, one_shot_flags);
}

bool FakeWriteAbrMetadataCustom(void* context, const AbrSlotData* a, const AbrSlotData* b,
                                uint8_t one_shot_flags) {
  return reinterpret_cast<FakeOps*>(context)->WriteMetadataCustom(a, b, one_shot_flags);
}

TEST(LibabrTest, GetBootSlotNotInitialized) {
  FakeOps ops;
  memset(&ops.metadata_, 0, sizeof(ops.metadata_));
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, true, nullptr));
  ValidateMetadata(ops.metadata_);
}

void GetBootSlotActiveNotSuccessful(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  bool is_slot_marked_successful = true;
  EXPECT_EQ(slot_index, AbrGetBootSlot(ops, true, &is_slot_marked_successful));
  EXPECT_FALSE(is_slot_marked_successful);
  ValidateMetadata(ops.metadata_);
}
TEST(LibabrTest, GetBootSlotActiveNotSuccessfulA) {
  GetBootSlotActiveNotSuccessful(kAbrSlotIndexA);
}
TEST(LibabrTest, GetBootSlotActiveNotSuccessfulB) {
  GetBootSlotActiveNotSuccessful(kAbrSlotIndexB);
}

void GetBootSlotActiveSuccessful(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  bool is_slot_marked_successful = false;
  EXPECT_EQ(slot_index, AbrGetBootSlot(ops, true, &is_slot_marked_successful));
  EXPECT_TRUE(is_slot_marked_successful);
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotActiveSuccessfulA) { GetBootSlotActiveSuccessful(kAbrSlotIndexA); }

TEST(LibabrTest, GetBootSlotActiveSuccessfulB) { GetBootSlotActiveSuccessful(kAbrSlotIndexB); }

void GetBootSlotNeitherSuccessful(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  bool is_slot_marked_successful = true;
  EXPECT_EQ(slot_index, AbrGetBootSlot(ops, true, &is_slot_marked_successful));
  EXPECT_FALSE(is_slot_marked_successful);
  ValidateMetadata(ops.metadata_);

  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority, 0);
  EXPECT_GT(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
}
TEST(LibabrTest, GetBootSlotNeitherSuccessfulA) { GetBootSlotNeitherSuccessful(kAbrSlotIndexA); }
TEST(LibabrTest, GetBootSlotNeitherSuccessfulB) { GetBootSlotNeitherSuccessful(kAbrSlotIndexB); }

void GetBootSlotOnlyActiveSuccessful(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  bool is_slot_marked_successful = false;
  EXPECT_EQ(slot_index, AbrGetBootSlot(ops, true, &is_slot_marked_successful));
  EXPECT_TRUE(is_slot_marked_successful);
  ValidateMetadata(ops.metadata_);

  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_NE(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
}
TEST(LibabrTest, GetBootSlotOnlyActiveSuccessfulA) {
  GetBootSlotOnlyActiveSuccessful(kAbrSlotIndexA);
}
TEST(LibabrTest, GetBootSlotOnlyActiveSuccessfulB) {
  GetBootSlotOnlyActiveSuccessful(kAbrSlotIndexB);
}

void GetBootSlotOnlyInactiveSuccessful(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, other_slot_index, false));
  bool is_slot_marked_successful = true;
  EXPECT_EQ(slot_index, AbrGetBootSlot(ops, true, &is_slot_marked_successful));
  EXPECT_FALSE(is_slot_marked_successful);
  ValidateMetadata(ops.metadata_);

  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority, 0);
  EXPECT_GT(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  // Success should be removed from the inactive slot.
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].tries_remaining, kAbrMaxTriesRemaining);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
}
TEST(LibabrTest, GetBootSlotOnlyInactiveSuccessfulA) {
  GetBootSlotOnlyInactiveSuccessful(kAbrSlotIndexA);
}
TEST(LibabrTest, GetBootSlotOnlyInactiveSuccessfulB) {
  GetBootSlotOnlyInactiveSuccessful(kAbrSlotIndexB);
}

void GetBootSlotUnbootable(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotUnbootable(ops, slot_index, kAbrUnbootableReasonOsRequested));
  // |slot_index| has higher prio but is unbootable, AbrGetBootSlot() should ignore it and select
  // |other_slot_index| instead.
  bool is_slot_marked_successful = true;
  EXPECT_EQ(other_slot_index, AbrGetBootSlot(ops, true, &is_slot_marked_successful));
  EXPECT_FALSE(is_slot_marked_successful);
  ValidateMetadata(ops.metadata_);

  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].unbootable_reason, kAbrUnbootableReasonOsRequested);
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].tries_remaining, kAbrMaxTriesRemaining - 1);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].unbootable_reason, kAbrUnbootableReasonNone);
}
TEST(LibabrTest, GetBootSlotUnbootableA) { GetBootSlotUnbootable(kAbrSlotIndexA); }
TEST(LibabrTest, GetBootSlotUnbootableB) { GetBootSlotUnbootable(kAbrSlotIndexB); }

void GetBootSlotLastBootDetection(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));

  // Use up all our boot attempts.
  for (int i = 0; i < kAbrMaxTriesRemaining; ++i) {
    ASSERT_EQ(slot_index, AbrGetBootSlot(ops, true, nullptr));
    ValidateMetadata(ops.metadata_);
  }

  // The slot should now be unbootable due to no tries remaining.
  AbrSlotInfo info;
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, slot_index, &info));
  EXPECT_FALSE(info.is_bootable);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNoMoreTries);

  // If we subsequently explicitly marks the slot unbootable, it should update the reason.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotUnbootable(ops, slot_index, kAbrUnbootableReasonOsRequested));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, slot_index, &info));
  EXPECT_FALSE(info.is_bootable);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonOsRequested);
}
TEST(LibabrTest, GetBootSlotLastBootDetectionA) { GetBootSlotLastBootDetection(kAbrSlotIndexA); }
TEST(LibabrTest, GetBootSlotLastBootDetectionB) { GetBootSlotLastBootDetection(kAbrSlotIndexB); }

// This shouldn't happen in the wild, but we'll test it for completeness.
void GetBootSlotBothSuccessful(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, other_slot_index, false));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  bool is_slot_marked_successful = false;
  EXPECT_EQ(slot_index, AbrGetBootSlot(ops, true, &is_slot_marked_successful));
  EXPECT_TRUE(is_slot_marked_successful);
  ValidateMetadata(ops.metadata_);

  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_NE(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  // Success should be removed from the inactive slot.
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].tries_remaining, kAbrMaxTriesRemaining);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
}
TEST(LibabrTest, GetBootSlotBothSuccessfulA) { GetBootSlotBothSuccessful(kAbrSlotIndexA); }
TEST(LibabrTest, GetBootSlotBothSuccessfulB) { GetBootSlotBothSuccessful(kAbrSlotIndexB); }

TEST(LibabrTest, GetBootSlotNoBootableSlot) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, false, nullptr));
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotNullReadOp) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.ops_.read_abr_metadata = nullptr;
  // The expectation is a fallback to recovery.
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, true, nullptr));
}

TEST(LibabrTest, GetBootSlotNullWriteOpNoUpdate) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.ops_.write_abr_metadata = nullptr;
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, false, nullptr));
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotNullWriteOpUpdate) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.ops_.write_abr_metadata = nullptr;
  // The expectation is to ignore the write error.
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, true, nullptr));
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotReadIOError) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.read_metadata_result_ = false;
  // The expectation is a fallback to recovery.
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, true, nullptr));
}

TEST(LibabrTest, GetBootSlotWriteIOErrorNoUpdate) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.write_metadata_result_ = false;
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, false, nullptr));
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotWriteIOErrorUpdate) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.write_metadata_result_ = false;
  // The expectation is to ignore the write error.
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, true, nullptr));
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotInvalidMetadataBadMagic) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  ops.metadata_.magic[0] = 'a';
  UpdateMetadataCRC(&ops.metadata_);
  // The expectation is that metadata is reinitialized, with A active.
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, true, nullptr));
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotInvalidMetadataBadCRC) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  ops.metadata_.crc32 = 0;
  // The expectation is that metadata is reinitialized, with A active.
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, true, nullptr));
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotInvalidMetadataUnsupportedVersion) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  ops.metadata_.version_major = 27;
  UpdateMetadataCRC(&ops.metadata_);
  // The expectation is a fallback to recovery without clobbering metadata.
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, true, nullptr));
  EXPECT_EQ(ops.metadata_.version_major, 27);
}

TEST(LibabrTest, GetBootSlotInvalidMetadataLittleEndianCRC) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.metadata_.crc32 = AbrCrc32(&ops.metadata_, sizeof(ops.metadata_) - sizeof(uint32_t));
  // The expectation is that metadata is reinitialized, with A active.
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, true, nullptr));
  ValidateMetadata(ops.metadata_);
}

void GetBootSlotNormalizeUnexpectedTries(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  // Set the metadata to a state where priority is zero, but tries remain.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ops.metadata_.slot_data[slot_index].priority = 0;
  UpdateMetadataCRC(&ops.metadata_);
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, true, nullptr));
  // The expectation is that the metadata has been normalized and updated.
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotNormalizeUnexpectedTriesA) {
  GetBootSlotNormalizeUnexpectedTries(kAbrSlotIndexA);
}

TEST(LibabrTest, GetBootSlotNormalizeUnexpectedTriesB) {
  GetBootSlotNormalizeUnexpectedTries(kAbrSlotIndexB);
}

void GetBootSlotNormalizeUnexpectedSuccessMark(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  // Set the metadata to a state where priority is zero, but marked successful.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  ops.metadata_.slot_data[slot_index].priority = 0;
  UpdateMetadataCRC(&ops.metadata_);
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, true, nullptr));
  // The expectation is that the metadata has been normalized and updated.
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotNormalizeUnexpectedSuccessMarkA) {
  GetBootSlotNormalizeUnexpectedSuccessMark(kAbrSlotIndexA);
}

TEST(LibabrTest, GetBootSlotNormalizeUnexpectedSuccessMarkB) {
  GetBootSlotNormalizeUnexpectedSuccessMark(kAbrSlotIndexB);
}

void GetBootSlotNormalizeTriesExhausted(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  // Set the metadata to a state where tries are exhausted and no successful mark, with an
  // arbitrary unbootable reason.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ops.metadata_.slot_data[slot_index].tries_remaining = 0;
  ops.metadata_.slot_data[slot_index].unbootable_reason = 0x42;
  UpdateMetadataCRC(&ops.metadata_);
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, true, nullptr));
  // The expectation is that the metadata has been normalized and updated.
  ValidateMetadata(ops.metadata_);
  // Whatever version of libabr wrote this metadata had already set the unbootable reason, we must
  // preserve it when normalizing metadata.
  ASSERT_EQ(ops.metadata_.slot_data[slot_index].unbootable_reason, 0x42);
}

TEST(LibabrTest, GetBootSlotNormalizeTriesExhaustedA) {
  GetBootSlotNormalizeTriesExhausted(kAbrSlotIndexA);
}

TEST(LibabrTest, GetBootSlotNormalizeTriesExhaustedB) {
  GetBootSlotNormalizeTriesExhausted(kAbrSlotIndexB);
}

void GetBootSlotNormalizeSuccessfulWithUnexpectedTries(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  // Set the metadata to a state where tries remain alongside a successful mark.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  ops.metadata_.slot_data[slot_index].tries_remaining = 3;
  UpdateMetadataCRC(&ops.metadata_);
  // Expect that the slot is reset to newly active state.
  bool is_slot_marked_successful = true;
  EXPECT_EQ(slot_index, AbrGetBootSlot(ops, true, &is_slot_marked_successful));
  EXPECT_FALSE(is_slot_marked_successful);
  // The expectation is that the metadata has been normalized and updated.
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, GetBootSlotNormalizeSuccessfulWithUnexpectedTriesA) {
  GetBootSlotNormalizeSuccessfulWithUnexpectedTries(kAbrSlotIndexA);
}

TEST(LibabrTest, GetBootSlotNormalizeSuccessfulWithUnexpectedTriesB) {
  GetBootSlotNormalizeSuccessfulWithUnexpectedTries(kAbrSlotIndexB);
}

void GetBootSlotNormalizePriorityOutOfRange(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  // Set the metadata to an active state where priority is higher than max.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ops.metadata_.slot_data[slot_index].priority = kAbrMaxPriority + 1;
  UpdateMetadataCRC(&ops.metadata_);
  EXPECT_EQ(slot_index, AbrGetBootSlot(ops, true, nullptr));
  // The expectation is that the metadata has been normalized and updated.
  ValidateMetadata(ops.metadata_);

  // When at max, should not change.
  ops.metadata_.slot_data[slot_index].priority = kAbrMaxPriority;
  UpdateMetadataCRC(&ops.metadata_);
  AbrGetBootSlot(ops, true, nullptr);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].priority, kAbrMaxPriority);
}

TEST(LibabrTest, GetBootSlotNormalizePriorityOutOfRangeA) {
  GetBootSlotNormalizePriorityOutOfRange(kAbrSlotIndexA);
}

TEST(LibabrTest, GetBootSlotNormalizePriorityOutOfRangeB) {
  GetBootSlotNormalizePriorityOutOfRange(kAbrSlotIndexB);
}

void GetBootSlotNormalizeTriesOutOfRange(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  // Set the metadata to an active state where tries_remaining is higher than max.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ops.metadata_.slot_data[slot_index].tries_remaining = kAbrMaxTriesRemaining + 1;
  UpdateMetadataCRC(&ops.metadata_);
  EXPECT_EQ(slot_index, AbrGetBootSlot(ops, true, nullptr));
  // The expectation is that the metadata has been normalized first and then the usual decrement.
  ValidateMetadata(ops.metadata_);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].tries_remaining, kAbrMaxTriesRemaining - 1);

  // When at max, should not change except for the usual decrement.
  ops.metadata_.slot_data[slot_index].tries_remaining = kAbrMaxTriesRemaining;
  UpdateMetadataCRC(&ops.metadata_);
  AbrGetBootSlot(ops, true, nullptr);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].tries_remaining, kAbrMaxTriesRemaining - 1);
}

TEST(LibabrTest, GetBootSlotNormalizeTriesOutOfRangeA) {
  GetBootSlotNormalizeTriesOutOfRange(kAbrSlotIndexA);
}

TEST(LibabrTest, GetBootSlotNormalizeTriesOutOfRangeB) {
  GetBootSlotNormalizeTriesOutOfRange(kAbrSlotIndexB);
}

TEST(LibabrTest, GetBootSlotOneShotRecovery) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  ASSERT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, true, nullptr));
  ValidateMetadata(ops.metadata_);
  // The setting should be automatically reset.
  AbrDataOneShotFlags one_shot_flags;
  ASSERT_EQ(AbrGetAndClearOneShotFlags(ops, &one_shot_flags), kAbrResultOk);
  EXPECT_FALSE(AbrIsOneShotRecoveryBootSet(one_shot_flags));
}

TEST(LibabrTest, GetBootSlotOneShotRecoveryNoUpdate) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  ASSERT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  EXPECT_EQ(kAbrSlotIndexB, AbrGetBootSlot(ops, false, nullptr));
  ValidateMetadata(ops.metadata_);
  // The setting was ignored so should persist.
  AbrDataOneShotFlags one_shot_flags;
  ASSERT_EQ(AbrGetAndClearOneShotFlags(ops, &one_shot_flags), kAbrResultOk);
  EXPECT_TRUE(AbrIsOneShotRecoveryBootSet(one_shot_flags));
}

TEST(LibabrTest, GetBootSlotUpdateTryCount) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  ops.metadata_.slot_data[kAbrSlotIndexB].tries_remaining = 3;
  UpdateMetadataCRC(&ops.metadata_);
  EXPECT_EQ(kAbrSlotIndexB, AbrGetBootSlot(ops, true, nullptr));
  ValidateMetadata(ops.metadata_);
  // Should be decremented by exactly one: 3 -> 2.
  EXPECT_EQ(2, ops.metadata_.slot_data[kAbrSlotIndexB].tries_remaining);
}

TEST(LibabrTest, GetBootSlotNoUpdates) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrSlotIndexB, AbrGetBootSlot(ops, false, nullptr));
  ValidateMetadata(ops.metadata_);
  EXPECT_EQ(0, ops.write_metadata_count_);
}

TEST(LibabrTest, GetBootSlotNoUpdatesFromNotInit) {
  FakeOps ops;
  memset(&ops.metadata_, 0, sizeof(ops.metadata_));
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, false, nullptr));
  EXPECT_EQ(0, ops.write_metadata_count_);
}

TEST(LibabrTest, GetBootSlotNoUpdatesFromNotNormalized) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  ops.metadata_.slot_data[kAbrSlotIndexB].priority = 0;
  UpdateMetadataCRC(&ops.metadata_);
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, false, nullptr));
  EXPECT_EQ(0, ops.write_metadata_count_);
}

TEST(LibabrTest, GetBootSlotNoExtraneousReads) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrSlotIndexR, AbrGetBootSlot(ops, false, nullptr));
  EXPECT_EQ(1, ops.read_metadata_count_);
}

TEST(LibabrTest, GetBootSlotNoExtraneousWrites) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, kAbrSlotIndexA, false));
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, true, nullptr));
  EXPECT_EQ(0, ops.write_metadata_count_);
}

TEST(LibabrTest, GetBootSlotNoExtraneousWritesOneUpdate) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrSlotIndexA, AbrGetBootSlot(ops, true, nullptr));
  // Expecting an update because of the tries_remaining decrement, but should be just one.
  EXPECT_EQ(1, ops.write_metadata_count_);
}

TEST(LibabrTest, GetSlotSuffix) {
  EXPECT_EQ(std::string("_a"), AbrGetSlotSuffix(kAbrSlotIndexA));
  EXPECT_EQ(std::string("_b"), AbrGetSlotSuffix(kAbrSlotIndexB));
  EXPECT_EQ(std::string("_r"), AbrGetSlotSuffix(kAbrSlotIndexR));
}

TEST(LibabrTest, GetSlotSuffixInvalidIndex) {
  EXPECT_EQ(std::string(""), AbrGetSlotSuffix((AbrSlotIndex)3));
}

void MarkSlotActive(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority, 0);
  EXPECT_GT(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].unbootable_reason, kAbrUnbootableReasonNone);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, MarkSlotActiveA) { MarkSlotActive(kAbrSlotIndexA); }

TEST(LibabrTest, MarkSlotActiveB) { MarkSlotActive(kAbrSlotIndexB); }

void MarkSlotActiveOverOther(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, other_slot_index, false));
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority,
            ops.metadata_.slot_data[other_slot_index].priority);
  EXPECT_GT(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].successful_boot, 1);
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, MarkSlotActiveOverOtherA) { MarkSlotActiveOverOther(kAbrSlotIndexA); }

TEST(LibabrTest, MarkSlotActiveOverOtherB) { MarkSlotActiveOverOther(kAbrSlotIndexB); }

void MarkSlotActiveClearsUnbootable(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.metadata_.slot_data[slot_index].unbootable_reason = kAbrUnbootableReasonNoMoreTries;
  UpdateMetadataCRC(&ops.metadata_);
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].unbootable_reason, kAbrUnbootableReasonNone);
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, MarkSlotActiveClearsUnbootableA) {
  MarkSlotActiveClearsUnbootable(kAbrSlotIndexA);
}

TEST(LibabrTest, MarkSlotActiveClearsUnbootableB) {
  MarkSlotActiveClearsUnbootable(kAbrSlotIndexB);
}

TEST(LibabrTest, MarkSlotActiveR) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultErrorInvalidData, AbrMarkSlotActive(ops, kAbrSlotIndexR));
}

TEST(LibabrTest, MarkSlotActiveInvalidIndex) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultErrorInvalidData, AbrMarkSlotActive(ops, (AbrSlotIndex)-1));
}

TEST(LibabrTest, MarkSlotActiveReadFailure) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.read_metadata_result_ = false;
  EXPECT_EQ(kAbrResultErrorIo, AbrMarkSlotActive(ops, kAbrSlotIndexA));
}

TEST(LibabrTest, MarkSlotActiveWriteFailure) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.write_metadata_result_ = false;
  EXPECT_EQ(kAbrResultErrorIo, AbrMarkSlotActive(ops, kAbrSlotIndexA));
}

TEST(LibabrTest, MarkSlotActiveNoExtraneousReads) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  EXPECT_EQ(1, ops.read_metadata_count_);
}

TEST(LibabrTest, MarkSlotActiveNoExtraneousWrites) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  EXPECT_EQ(1, ops.write_metadata_count_);
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  EXPECT_EQ(0, ops.write_metadata_count_);
}

void MarkSlotUnbootable(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, other_slot_index, false));
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotUnbootable(ops, slot_index, kAbrUnbootableReasonOsRequested));
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].priority, kAbrMaxPriority - 1);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].unbootable_reason, kAbrUnbootableReasonOsRequested);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].priority, kAbrMaxPriority);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].tries_remaining, 0);
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, MarkSlotUnbootableA) { MarkSlotUnbootable(kAbrSlotIndexA); }

TEST(LibabrTest, MarkSlotUnbootableB) { MarkSlotUnbootable(kAbrSlotIndexB); }

TEST(LibabrTest, MarkSlotUnbootableReason) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk,
            AbrMarkSlotUnbootable(ops, kAbrSlotIndexA, kAbrUnbootableReasonVerificationFailure));
  ASSERT_EQ(ops.metadata_.slot_data[kAbrSlotIndexA].unbootable_reason,
            kAbrUnbootableReasonVerificationFailure);
}

TEST(LibabrTest, MarkSlotUnbootableR) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultErrorInvalidData,
            AbrMarkSlotUnbootable(ops, kAbrSlotIndexR, kAbrUnbootableReasonOsRequested));
}

TEST(LibabrTest, MarkSlotUnbootableInvalidIndex) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultErrorInvalidData,
            AbrMarkSlotUnbootable(ops, (AbrSlotIndex)-1, kAbrUnbootableReasonOsRequested));
}

TEST(LibabrTest, MarkSlotUnbootableReadFailure) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.read_metadata_result_ = false;
  EXPECT_EQ(kAbrResultErrorIo,
            AbrMarkSlotUnbootable(ops, kAbrSlotIndexA, kAbrUnbootableReasonOsRequested));
}

TEST(LibabrTest, MarkSlotUnbootableWriteFailure) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.write_metadata_result_ = false;
  EXPECT_EQ(kAbrResultErrorIo,
            AbrMarkSlotUnbootable(ops, kAbrSlotIndexA, kAbrUnbootableReasonOsRequested));
}

TEST(LibabrTest, MarkSlotUnbootableNoExtraneousReads) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.read_metadata_count_ = 0;
  EXPECT_EQ(kAbrResultOk,
            AbrMarkSlotUnbootable(ops, kAbrSlotIndexA, kAbrUnbootableReasonOsRequested));
  EXPECT_EQ(1, ops.read_metadata_count_);
}

TEST(LibabrTest, MarkSlotUnbootableNoExtraneousWrites) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrResultOk,
            AbrMarkSlotUnbootable(ops, kAbrSlotIndexA, kAbrUnbootableReasonOsRequested));
  EXPECT_EQ(1, ops.write_metadata_count_);
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrResultOk,
            AbrMarkSlotUnbootable(ops, kAbrSlotIndexA, kAbrUnbootableReasonOsRequested));
  EXPECT_EQ(0, ops.write_metadata_count_);
}

void MarkSlotSuccessful(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_GT(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].tries_remaining, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, MarkSlotSuccessfulA) { MarkSlotSuccessful(kAbrSlotIndexA); }

TEST(LibabrTest, MarkSlotSuccessfulB) { MarkSlotSuccessful(kAbrSlotIndexB); }

TEST(LibabrTest, MarkSlotSuccessfulR) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultErrorInvalidData, AbrMarkSlotSuccessful(ops, kAbrSlotIndexR, false));
}

TEST(LibabrTest, MarkSlotSuccessfulInvalidIndex) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultErrorInvalidData, AbrMarkSlotSuccessful(ops, (AbrSlotIndex)-1, false));
}

TEST(LibabrTest, MarkSlotSuccessfulUnbootable) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultErrorInvalidData, AbrMarkSlotSuccessful(ops, kAbrSlotIndexA, false));
}

void MarkSlotSuccessfulFromUnbootableOk(AbrSlotIndex slot_index) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotUnbootable(ops, slot_index, kAbrUnbootableReasonOsRequested));
  // Marking successful should fail unless the |from_unbootable_ok| arg is set.
  ASSERT_EQ(kAbrResultErrorInvalidData, AbrMarkSlotSuccessful(ops, slot_index, false));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, true));

  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_GT(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].unbootable_reason, kAbrUnbootableReasonNone);
  ValidateMetadata(ops.metadata_);
}
TEST(LibabrTest, MarkSlotSuccessfulFromUnbootableOkA) {
  MarkSlotSuccessfulFromUnbootableOk(kAbrSlotIndexA);
}
TEST(LibabrTest, MarkSlotSuccessfulFromUnbootableOkB) {
  MarkSlotSuccessfulFromUnbootableOk(kAbrSlotIndexB);
}

TEST(LibabrTest, MarkSlotSuccessfulReadFailure) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.read_metadata_result_ = false;
  EXPECT_EQ(kAbrResultErrorIo, AbrMarkSlotSuccessful(ops, kAbrSlotIndexA, false));
}

TEST(LibabrTest, MarkSlotSuccessfulWriteFailure) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.write_metadata_result_ = false;
  EXPECT_EQ(kAbrResultErrorIo, AbrMarkSlotSuccessful(ops, kAbrSlotIndexA, false));
}

TEST(LibabrTest, MarkSlotSuccessfulNoExtraneousReads) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.read_metadata_count_ = 0;
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, kAbrSlotIndexA, false));
  EXPECT_EQ(1, ops.read_metadata_count_);
}

TEST(LibabrTest, MarkSlotSuccessfulNoExtraneousWrites) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, kAbrSlotIndexA, false));
  EXPECT_EQ(1, ops.write_metadata_count_);
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, kAbrSlotIndexA, false));
  EXPECT_EQ(0, ops.write_metadata_count_);
}

void MarkSlotSuccessfulWithOtherSuccessful(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  // First set up the other slot as successful.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, other_slot_index, false));
  EXPECT_GT(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].tries_remaining, 0);
  EXPECT_NE(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  EXPECT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  EXPECT_GT(ops.metadata_.slot_data[slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[slot_index].tries_remaining, 0);
  EXPECT_NE(ops.metadata_.slot_data[slot_index].successful_boot, 0);
  // Expect that the other slot has been marked not successful.
  EXPECT_NE(ops.metadata_.slot_data[other_slot_index].priority, 0);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].tries_remaining, kAbrMaxTriesRemaining);
  EXPECT_EQ(ops.metadata_.slot_data[other_slot_index].successful_boot, 0);
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, MarkSlotSuccessfulWithOtherSuccessfulA) {
  MarkSlotSuccessfulWithOtherSuccessful(kAbrSlotIndexA);
}

TEST(LibabrTest, MarkSlotSuccessfulWithOtherSuccessfulB) {
  MarkSlotSuccessfulWithOtherSuccessful(kAbrSlotIndexB);
}

void GetSlotInfo(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);
  FakeOps ops = FakeOpsWithInitializedMetadata();
  AbrSlotInfo info;
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, slot_index, &info));
  EXPECT_FALSE(info.is_bootable);
  EXPECT_FALSE(info.is_active);
  EXPECT_FALSE(info.is_marked_successful);
  EXPECT_EQ(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNone);
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, slot_index, &info));
  EXPECT_TRUE(info.is_bootable);
  EXPECT_TRUE(info.is_active);
  EXPECT_FALSE(info.is_marked_successful);
  EXPECT_GT(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNone);
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, slot_index, &info));
  EXPECT_TRUE(info.is_bootable);
  EXPECT_TRUE(info.is_active);
  EXPECT_TRUE(info.is_marked_successful);
  EXPECT_EQ(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNone);
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, slot_index, &info));
  EXPECT_TRUE(info.is_bootable);
  EXPECT_FALSE(info.is_active);
  EXPECT_TRUE(info.is_marked_successful);
  EXPECT_EQ(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNone);
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotUnbootable(ops, slot_index, kAbrUnbootableReasonOsRequested));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, slot_index, &info));
  EXPECT_FALSE(info.is_bootable);
  EXPECT_FALSE(info.is_active);
  EXPECT_FALSE(info.is_marked_successful);
  EXPECT_EQ(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonOsRequested);
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotUnbootable(ops, slot_index, kAbrUnbootableReasonNone));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, slot_index, &info));
  EXPECT_FALSE(info.is_bootable);
  EXPECT_FALSE(info.is_active);
  EXPECT_FALSE(info.is_marked_successful);
  EXPECT_EQ(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNone);
}

TEST(LibabrTest, GetSlotInfoA) { GetSlotInfo(kAbrSlotIndexA); }

TEST(LibabrTest, GetSlotInfoB) { GetSlotInfo(kAbrSlotIndexB); }

TEST(LibabrTest, GetSlotInfoR) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  AbrSlotInfo info;
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, kAbrSlotIndexR, &info));
  EXPECT_TRUE(info.is_bootable);
  EXPECT_TRUE(info.is_active);
  EXPECT_TRUE(info.is_marked_successful);
  EXPECT_EQ(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNone);
  // When any other slot is bootable, R is not active.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, kAbrSlotIndexR, &info));
  EXPECT_TRUE(info.is_bootable);
  EXPECT_FALSE(info.is_active);
  EXPECT_TRUE(info.is_marked_successful);
  EXPECT_EQ(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNone);
  ASSERT_EQ(kAbrResultOk,
            AbrMarkSlotUnbootable(ops, kAbrSlotIndexB, kAbrUnbootableReasonOsRequested));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexA));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, kAbrSlotIndexR, &info));
  EXPECT_TRUE(info.is_bootable);
  EXPECT_FALSE(info.is_active);
  EXPECT_TRUE(info.is_marked_successful);
  EXPECT_EQ(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNone);
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, kAbrSlotIndexB));
  EXPECT_TRUE(info.is_bootable);
  EXPECT_FALSE(info.is_active);
  EXPECT_TRUE(info.is_marked_successful);
  EXPECT_EQ(info.num_tries_remaining, 0);
  EXPECT_EQ(info.unbootable_reason, kAbrUnbootableReasonNone);
}

TEST(LibabrTest, GetSlotInfoReadFailure) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.read_metadata_result_ = false;
  AbrSlotInfo info;
  EXPECT_EQ(kAbrResultErrorIo, AbrGetSlotInfo(ops, kAbrSlotIndexB, &info));
}

TEST(LibabrTest, GetSlotInfoNoExtraneousReads) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.read_metadata_count_ = 0;
  AbrSlotInfo info;
  EXPECT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, kAbrSlotIndexB, &info));
  EXPECT_EQ(1, ops.read_metadata_count_);
}

TEST(LibabrTest, GetSlotInfoNoWrites) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  AbrSlotInfo info;
  EXPECT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, kAbrSlotIndexB, &info));
  EXPECT_EQ(0, ops.write_metadata_count_);
}

TEST(LibabrTest, SetOneShotRecovery) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  EXPECT_EQ(kAbrDataOneShotFlagRecoveryBoot, ops.metadata_.one_shot_flags);
  ValidateMetadata(ops.metadata_);
  EXPECT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, false));
  EXPECT_EQ(kAbrDataOneShotFlagNone, ops.metadata_.one_shot_flags);
  ValidateMetadata(ops.metadata_);
}

TEST(LibabrTest, SetOneShotRecoveryReadFailure) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.read_metadata_result_ = false;
  EXPECT_EQ(kAbrResultErrorIo, AbrSetOneShotRecovery(ops, true));
}

TEST(LibabrTest, SetOneShotRecoveryWriteFailure) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.write_metadata_result_ = false;
  EXPECT_EQ(kAbrResultErrorIo, AbrSetOneShotRecovery(ops, true));
}

TEST(LibabrTest, SetOneShotRecoveryNoExtraneousReads) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  EXPECT_EQ(1, ops.read_metadata_count_);
}

TEST(LibabrTest, SetOneShotRecoveryNoExtraneousWrites) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  EXPECT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  EXPECT_EQ(1, ops.write_metadata_count_);
  ops.write_metadata_count_ = 0;
  EXPECT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  EXPECT_EQ(0, ops.write_metadata_count_);
}

TEST(LibabrTest, UsesCustomMetadata) {
  FakeOps ops = FakeOpsCustom();
  EXPECT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  EXPECT_EQ(1, ops.write_metadata_custom_count_);
  EXPECT_EQ(1, ops.read_metadata_custom_count_);
  ops.read_metadata_custom_count_ = 0;
  ops.write_metadata_custom_count_ = 0;
  AbrSlotInfo info;
  EXPECT_EQ(kAbrResultOk, AbrGetSlotInfo(ops, kAbrSlotIndexA, &info));
  EXPECT_EQ(0, ops.write_metadata_custom_count_);
  EXPECT_EQ(1, ops.read_metadata_custom_count_);
}

void GetSlotLastMarkedActiveTest(AbrSlotIndex slot_index) {
  AbrSlotIndex other_slot_index = OtherSlot(slot_index);

  // Set both slots to active, with |slot_index| being the most recent one
  // marked active. This is a tyical state after an update to |slot_index|.
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, slot_index));

  AbrSlotIndex out;
  ASSERT_EQ(kAbrResultOk, AbrGetSlotLastMarkedActive(ops, &out));
  EXPECT_EQ(slot_index, out);

  // Marking the slot successful shall not change the result.
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, slot_index, false));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotLastMarkedActive(ops, &out));
  EXPECT_EQ(slot_index, out);

  // Marking the slot unbootable shall not change the result
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotUnbootable(ops, slot_index, kAbrUnbootableReasonOsRequested));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotLastMarkedActive(ops, &out));
  EXPECT_EQ(slot_index, out);

  // Marking the other slot successful shall not change the result
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotSuccessful(ops, other_slot_index, false));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotLastMarkedActive(ops, &out));
  EXPECT_EQ(slot_index, out);

  // Marking the other slot unbootable shall not change the result.
  ASSERT_EQ(kAbrResultOk,
            AbrMarkSlotUnbootable(ops, other_slot_index, kAbrUnbootableReasonOsRequested));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotLastMarkedActive(ops, &out));
  EXPECT_EQ(slot_index, out);

  // Setting one shot recovery shall not change the result
  ASSERT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotLastMarkedActive(ops, &out));
  EXPECT_EQ(slot_index, out);

  // Resetting one shot recovery shall not change the result
  ASSERT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, false));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotLastMarkedActive(ops, &out));
  EXPECT_EQ(slot_index, out);

  // Marking the other slot active does change the result
  ASSERT_EQ(kAbrResultOk, AbrMarkSlotActive(ops, other_slot_index));
  ASSERT_EQ(kAbrResultOk, AbrGetSlotLastMarkedActive(ops, &out));
  EXPECT_EQ(other_slot_index, out);
}

TEST(LibabrTest, GetSlotLastMarkedActiveTestA) { GetSlotLastMarkedActiveTest(kAbrSlotIndexA); }

TEST(LibabrTest, GetSlotLastMarkedActiveTestB) { GetSlotLastMarkedActiveTest(kAbrSlotIndexB); }

TEST(LibabrTest, IsOneShotRecoveryBootSetTrue) {
  EXPECT_TRUE(AbrIsOneShotRecoveryBootSet(kAbrDataOneShotFlagRecoveryBoot));
}
TEST(LibabrTest, IsOneShotRecoveryBootSetFalse) {
  EXPECT_FALSE(AbrIsOneShotRecoveryBootSet(kAbrDataOneShotFlagNone));
  EXPECT_FALSE(AbrIsOneShotRecoveryBootSet(kAbrDataOneShotFlagBootloaderBoot));
}

TEST(LibabrTest, IsOneShotBootloaderBootSetTrue) {
  EXPECT_TRUE(AbrIsOneShotBootloaderBootSet(kAbrDataOneShotFlagBootloaderBoot));
}

TEST(LibabrTest, IsOneShotBootloaderBootSetFalse) {
  EXPECT_FALSE(AbrIsOneShotBootloaderBootSet(kAbrDataOneShotFlagNone));
  EXPECT_FALSE(AbrIsOneShotBootloaderBootSet(kAbrDataOneShotFlagRecoveryBoot));
}

TEST(LibabrTest, IsOneShotRecoveryBootTrue) {
  AbrData abr_data{};
  abr_data.one_shot_flags = kAbrDataOneShotFlagRecoveryBoot;
  EXPECT_TRUE(AbrIsOneShotRecoveryBoot(&abr_data));
}

TEST(LibabrTest, IsOneShotRecoveryBootFalse) {
  AbrData abr_data{};
  abr_data.one_shot_flags = kAbrDataOneShotFlagNone;
  EXPECT_FALSE(AbrIsOneShotRecoveryBoot(&abr_data));
  abr_data.one_shot_flags = kAbrDataOneShotFlagBootloaderBoot;
  EXPECT_FALSE(AbrIsOneShotRecoveryBoot(&abr_data));
}

TEST(LibabrTest, IsOneShotBootloaderBootTrue) {
  AbrData abr_data{};
  abr_data.one_shot_flags = kAbrDataOneShotFlagBootloaderBoot;
  EXPECT_TRUE(AbrIsOneShotBootloaderBoot(&abr_data));
}

TEST(LibabrTest, IsOneShotBootloaderBootFalse) {
  AbrData abr_data{};
  abr_data.one_shot_flags = kAbrDataOneShotFlagNone;
  EXPECT_FALSE(AbrIsOneShotBootloaderBoot(&abr_data));
  abr_data.one_shot_flags = kAbrDataOneShotFlagRecoveryBoot;
  EXPECT_FALSE(AbrIsOneShotBootloaderBoot(&abr_data));
}

TEST(LibabrTest, SetOneShotRecoveryBoot) {
  AbrData abr_data{};
  abr_data.one_shot_flags = kAbrDataOneShotFlagNone;
  EXPECT_FALSE(AbrIsOneShotRecoveryBoot(&abr_data));

  AbrSetOneShotRecoveryBoot(&abr_data, true);
  EXPECT_TRUE(AbrIsOneShotRecoveryBoot(&abr_data));

  AbrSetOneShotRecoveryBoot(&abr_data, false);
  EXPECT_FALSE(AbrIsOneShotRecoveryBoot(&abr_data));
}

TEST(LibabrTest, SetOneShotRecoveryBootDontChangeOther) {
  AbrData abr_data{};

  abr_data.one_shot_flags = static_cast<uint8_t>(~kAbrDataOneShotFlagRecoveryBoot);
  EXPECT_EQ(abr_data.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagRecoveryBoot));

  AbrSetOneShotRecoveryBoot(&abr_data, true);
  EXPECT_EQ(abr_data.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagRecoveryBoot |
                                                          kAbrDataOneShotFlagRecoveryBoot));

  AbrSetOneShotRecoveryBoot(&abr_data, false);
  EXPECT_EQ(abr_data.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagRecoveryBoot));
}

TEST(LibabrTest, SetOneShotBootloaderBoot) {
  AbrData abr_data{};
  abr_data.one_shot_flags = kAbrDataOneShotFlagNone;
  EXPECT_FALSE(AbrIsOneShotBootloaderBoot(&abr_data));

  AbrSetOneShotBootloaderBoot(&abr_data, true);
  EXPECT_TRUE(AbrIsOneShotBootloaderBoot(&abr_data));

  AbrSetOneShotBootloaderBoot(&abr_data, false);
  EXPECT_FALSE(AbrIsOneShotBootloaderBoot(&abr_data));
}

TEST(LibabrTest, SetOneShotBootloaderBootDontChangeOther) {
  AbrData abr_data{};

  abr_data.one_shot_flags = static_cast<uint8_t>(~kAbrDataOneShotFlagBootloaderBoot);
  EXPECT_EQ(abr_data.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagBootloaderBoot));

  AbrSetOneShotBootloaderBoot(&abr_data, true);
  EXPECT_EQ(abr_data.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagBootloaderBoot |
                                                          kAbrDataOneShotFlagBootloaderBoot));

  AbrSetOneShotBootloaderBoot(&abr_data, false);
  EXPECT_EQ(abr_data.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagBootloaderBoot));
}

TEST(LibabrTest, AbrSetOneShotRecovery) {
  FakeOps ops = FakeOpsWithInitializedMetadata();

  ASSERT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  EXPECT_TRUE(AbrIsOneShotRecoveryBoot(&ops.metadata_));

  ASSERT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, false));
  EXPECT_FALSE(AbrIsOneShotRecoveryBoot(&ops.metadata_));
}

TEST(LibabrTest, AbrSetOneShotRecoveryDontChangeOther) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.metadata_.one_shot_flags = static_cast<uint8_t>(~kAbrDataOneShotFlagRecoveryBoot);
  UpdateMetadataCRC(&ops.metadata_);

  ASSERT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, true));
  EXPECT_EQ(ops.metadata_.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagRecoveryBoot) |
                                              kAbrDataOneShotFlagRecoveryBoot);

  ASSERT_EQ(kAbrResultOk, AbrSetOneShotRecovery(ops, false));
  EXPECT_EQ(ops.metadata_.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagRecoveryBoot));
}

TEST(LibabrTest, AbrSetOneShotBootloader) {
  FakeOps ops = FakeOpsWithInitializedMetadata();

  ASSERT_EQ(kAbrResultOk, AbrSetOneShotBootloader(ops, true));
  EXPECT_TRUE(AbrIsOneShotBootloaderBoot(&ops.metadata_));

  ASSERT_EQ(kAbrResultOk, AbrSetOneShotBootloader(ops, false));
  EXPECT_FALSE(AbrIsOneShotBootloaderBoot(&ops.metadata_));
}

TEST(LibabrTest, AbrSetOneShotBootloaderDontChangeOther) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  ops.metadata_.one_shot_flags = static_cast<uint8_t>(~kAbrDataOneShotFlagBootloaderBoot);
  UpdateMetadataCRC(&ops.metadata_);

  ASSERT_EQ(kAbrResultOk, AbrSetOneShotBootloader(ops, true));
  EXPECT_EQ(ops.metadata_.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagBootloaderBoot) |
                                              kAbrDataOneShotFlagBootloaderBoot);

  ASSERT_EQ(kAbrResultOk, AbrSetOneShotBootloader(ops, false));
  EXPECT_EQ(ops.metadata_.one_shot_flags, static_cast<uint8_t>(~kAbrDataOneShotFlagBootloaderBoot));
}

class LibabrTestOneShotFlagsFixture : public ::zxtest::TestWithParam<uint8_t> {};

#if defined(__clang__)
// Disabling UB sanitizer because we test values not defined in the enumeration.
[[clang::no_sanitize("undefined")]]
#endif
bool IsEqual(const AbrDataOneShotFlags& flags, uint8_t input_flags) {
  return static_cast<uint8_t>(flags) == input_flags;
}

TEST_P(LibabrTestOneShotFlagsFixture, AbrGetAndClearOneShotFlags) {
  FakeOps ops = FakeOpsWithInitializedMetadata();
  const uint8_t input_flags = GetParam();
  AbrDataOneShotFlags flags;

  // Check initial state
  ASSERT_EQ(kAbrResultOk, AbrGetAndClearOneShotFlags(ops, &flags));
  EXPECT_EQ(flags, kAbrDataOneShotFlagNone);
  EXPECT_EQ(ops.metadata_.one_shot_flags, kAbrDataOneShotFlagNone);

  // Check if correct flags retrieved
  ops.metadata_.one_shot_flags = input_flags;
  UpdateMetadataCRC(&ops.metadata_);
  ASSERT_EQ(kAbrResultOk, AbrGetAndClearOneShotFlags(ops, &flags));
  EXPECT_TRUE(IsEqual(flags, input_flags));

  // Check if flags were cleared
  EXPECT_EQ(ops.metadata_.one_shot_flags, kAbrDataOneShotFlagNone);
}

INSTANTIATE_TEST_SUITE_P(
    AbrGetOneShotFlags, LibabrTestOneShotFlagsFixture,
    ::zxtest::Values(0, kAbrDataOneShotFlagNone, kAbrDataOneShotFlagRecoveryBoot,
                     ~kAbrDataOneShotFlagRecoveryBoot, kAbrDataOneShotFlagBootloaderBoot,
                     kAbrDataOneShotFlagBootloaderBoot | kAbrDataOneShotFlagRecoveryBoot,
                     ~kAbrDataOneShotFlagBootloaderBoot, 0xff));

}  // namespace
