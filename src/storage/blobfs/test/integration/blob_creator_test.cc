// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <unistd.h>
#include <zircon/errors.h>

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <utility>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/delivery_blob_private.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"

namespace blobfs {
namespace {

class BlobCreatorTest : public BlobfsTest {
 public:
  void Barrier() const {
    // This is just a barrier to reduce the risk of a race. There is no way for the caller to wait
    // and guarantee that the vmo close has gotten to the port on the server. So we send some
    // other message that will be handled by the server to ensure that we do some kind of waiting on
    // it. Since that server is single-threaded it is unlikely that the close has not made it back
    // to start handling before it gets the next message after this one.
    auto info = fs().GetFsInfo();
    ASSERT_OK(info);
  }
};

TEST_F(BlobCreatorTest, CreateNewBlobSucceeds) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  EXPECT_OK(blob_creator().CreateAndWriteBlob(blob));
}

TEST_F(BlobCreatorTest, CreateExistingBlobFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  EXPECT_OK(blob_creator().CreateAndWriteBlob(blob));

  auto writer = blob_creator().Create(blob.digest());
  EXPECT_STATUS(writer, ZX_ERR_ALREADY_EXISTS);
}

TEST_F(BlobCreatorTest, TwoWritesToOneDigestFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);

  // Can't start two writers for the same digest at once.
  EXPECT_STATUS(blob_creator().CreateExisting(blob.digest()), ZX_ERR_ALREADY_EXISTS);

  ASSERT_OK(writer->WriteBlob(blob));

  // Open the blob under the old version.
  auto vmo = blob_reader().GetVmo(blob.digest());
  EXPECT_OK(vmo);

  // Other write completed. This can be replaced now. Note that the other connection isn't even
  // closed yet, but the blob has become readable.
  auto writer2 = blob_creator().CreateExisting(blob.digest());
  EXPECT_OK(writer2);

  // Can't overwrite while the other overwrite is in progress.
  EXPECT_STATUS(blob_creator().CreateExisting(blob.digest()), ZX_ERR_ALREADY_EXISTS);
  ASSERT_OK(writer2->WriteBlob(blob));

  // Try to read with the new info.
  char a;
  EXPECT_OK(vmo->read(&a, 0, 1));
}

TEST_F(BlobCreatorTest, AbandonOverwriteAllowsRestart) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(blob));

  {
    auto writer = blob_creator().CreateExisting(blob.digest());
    EXPECT_OK(writer);

    // Can't overwrite while the other overwrite is in progress.
    EXPECT_STATUS(blob_creator().CreateExisting(blob.digest()), ZX_ERR_ALREADY_EXISTS);
  }

  // First writer went away, so now we can try again.
  auto writer = blob_creator().CreateExisting(blob.digest());
  EXPECT_OK(writer);
  EXPECT_OK(writer->WriteBlob(blob));
}

TEST_F(BlobCreatorTest, FailOverwriteForUnlinked) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(blob));
  {
    // Grab the vmo to survive unlink.
    auto vmo = blob_reader().GetVmo(blob.digest());
    EXPECT_OK(vmo);

    ASSERT_OK(Unlink(blob.digest()));

    EXPECT_STATUS(blob_creator().CreateExisting(blob.digest()), ZX_ERR_ALREADY_EXISTS);
  }
}

TEST_F(BlobCreatorTest, UnlinkPreventedByOverwrite) {
  uint64_t old_used_bytes;
  {
    auto info = fs().GetFsInfo();
    ASSERT_OK(info);
    old_used_bytes = info->used_bytes;
  }

  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(blob));

  // Should have grown.
  {
    auto info = fs().GetFsInfo();
    ASSERT_OK(info);
    ASSERT_GT(info->used_bytes, old_used_bytes);
  }

  auto fd = fs().GetRootFd();
  {
    // Start overwrite.
    auto writer = blob_creator().CreateExisting(blob.digest());
    EXPECT_OK(writer);

    ASSERT_OK(Unlink(blob.digest()));

    // Finish overwrite. This will let the purge complete.
    ASSERT_OK(writer);
  }

  // Should be back where we started.
  {
    auto info = fs().GetFsInfo();
    ASSERT_OK(info);
    ASSERT_EQ(info->used_bytes, old_used_bytes);
  }
}

TEST_F(BlobCreatorTest, UnlinkDuringOverwrite) {
  uint64_t old_used_bytes;
  {
    auto info = fs().GetFsInfo();
    ASSERT_OK(info);
    old_used_bytes = info->used_bytes;
  }

  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(blob));

  // Should have grown.
  {
    auto info = fs().GetFsInfo();
    ASSERT_OK(info);
    ASSERT_GT(info->used_bytes, old_used_bytes);
  }

  auto fd = fs().GetRootFd();
  {
    auto vmo = blob_reader().GetVmo(blob.digest());
    EXPECT_OK(vmo);

    // Start overwrite.
    auto writer = blob_creator().CreateExisting(blob.digest());
    EXPECT_OK(writer);

    ASSERT_OK(Unlink(blob.digest()));

    // Finish overwrite.
    ASSERT_OK(writer->WriteBlob(blob));

    // Try to read with the new info.
    char a;
    EXPECT_OK(vmo->read(&a, 0, 1));
  }

  ASSERT_NO_FATAL_FAILURE(Barrier());

  // Should be back where we started.
  {
    auto info = fs().GetFsInfo();
    ASSERT_OK(info);
    ASSERT_EQ(info->used_bytes, old_used_bytes);
  }
}

TEST_F(BlobCreatorTest, AllowExistingNullBlob) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  EXPECT_OK(blob_creator().CreateAndWriteBlob(blob));

  auto writer = blob_creator().CreateExisting(blob.digest());
  EXPECT_OK(writer);
  EXPECT_OK(writer->WriteBlob(blob));

  EXPECT_OK(blob_reader().GetVmo(blob.digest()));
}

using BlobWriterTest = BlobCreatorTest;

TEST_F(BlobWriterTest, ValidateRingBufferSize) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(vmo);
  ASSERT_EQ(GetVmoSize(*vmo), kRingBufferSize);
}

TEST_F(BlobWriterTest, MultipleGetVmoCallsFail) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);

  EXPECT_STATUS(writer->GetVmo(blob.data().size()), ZX_ERR_BAD_STATE);
}

TEST_F(BlobWriterTest, GetVmoWithTooSmallOfPayloadFails) {
  // BlobWriter only accepts delivery blobs and GetVmo should validate that the payload is at least
  // the size of the delivery blob header.
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(MetadataType1::kHeaderSize - 1);
  EXPECT_STATUS(writer_vmo, ZX_ERR_BAD_STATE);
}

TEST_F(BlobWriterTest, BytesReadyBeforeGetVmoFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  ASSERT_STATUS(writer->BytesReady(5), ZX_ERR_BAD_STATE);
}

TEST_F(BlobWriterTest, WritingMoreThanTheRingBufferFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);
  ASSERT_STATUS(writer->BytesReady(kRingBufferSize + 1), ZX_ERR_OUT_OF_RANGE);
}

TEST_F(BlobWriterTest, WritingMoreBytesThanExpectedFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(vmo);
  ASSERT_GT(GetVmoSize(*vmo), blob.data().size());

  ASSERT_STATUS(writer->BytesReady(blob.data().size() + 1), ZX_ERR_BUFFER_TOO_SMALL);
}

TEST_F(BlobWriterTest, WrittenBlobIsReadable) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(blob));

  auto reader_vmo = blob_reader().GetVmo(blob.digest());
  ASSERT_OK(reader_vmo);
}

TEST_F(BlobWriterTest, ZeroBytesReadyIsValid) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);
  uint64_t vmo_size = GetVmoSize(*writer_vmo);
  ASSERT_GT(vmo_size, blob.data().size());
  ASSERT_GT(blob.data().size(), 20lu);

  ASSERT_OK(writer->BytesReady(0));
  ASSERT_OK(writer_vmo->write(blob.data().data(), 0, blob.data().size()));
  ASSERT_OK(writer->BytesReady(0));
  ASSERT_OK(writer->BytesReady(blob.data().size()));
  ASSERT_OK(writer->BytesReady(0));

  auto reader_vmo = blob_reader().GetVmo(blob.digest());
  ASSERT_OK(reader_vmo);
}

TEST_F(BlobWriterTest, MultipleWrites) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);
  uint64_t vmo_size = GetVmoSize(*writer_vmo);
  ASSERT_GT(vmo_size, blob.data().size());
  ASSERT_GT(blob.data().size(), 20lu);
  ASSERT_OK(writer_vmo->write(blob.data().data(), 0, blob.data().size()));
  ASSERT_OK(writer->BytesReady(10));
  ASSERT_OK(writer->BytesReady(10));
  ASSERT_OK(writer->BytesReady(blob.data().size() - 20));

  auto reader_vmo = blob_reader().GetVmo(blob.digest());
  ASSERT_OK(reader_vmo);
}

TEST_F(BlobWriterTest, BytesReadyAfterBlobWrittenFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);
  ASSERT_GT(GetVmoSize(*writer_vmo), blob.data().size());
  ASSERT_OK(writer_vmo->write(blob.data().data(), 0, blob.data().size()));
  ASSERT_OK(writer->BytesReady(blob.data().size()));

  // Continuing to write fails.
  ASSERT_STATUS(writer->BytesReady(1), ZX_ERR_BUFFER_TOO_SMALL);

  // The blob was still written.
  auto reader_vmo = blob_reader().GetVmo(blob.digest());
  ASSERT_OK(reader_vmo);
}

TEST_F(BlobWriterTest, WriteNullBlob) {
  auto blob = TestDeliveryBlob::CreateUncompressed(0);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(blob));

  auto reader_vmo = blob_reader().GetVmo(blob.digest());
  ASSERT_OK(reader_vmo);
  EXPECT_EQ(GetVmoSize(*reader_vmo), 0lu);
}

TEST_F(BlobWriterTest, BytesReadySpanningEndOfVmo) {
  auto blob = TestDeliveryBlob::CreateUncompressed(kRingBufferSize + 50);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);
  uint64_t vmo_size = GetVmoSize(*writer_vmo);
  ASSERT_EQ(vmo_size, kRingBufferSize);
  ASSERT_LT(vmo_size, blob.data().size());
  ASSERT_GT(vmo_size * 2, blob.data().size());

  // Write out the number of bytes that will wrap around the end of the VMO first. This allows up to
  // write the entire ring buffer second.
  uint64_t wrapped_byte_count = blob.data().size() - kRingBufferSize;
  ASSERT_OK(writer_vmo->write(blob.data().data(), 0, wrapped_byte_count));
  ASSERT_OK(writer->BytesReady(wrapped_byte_count));

  ASSERT_OK(writer_vmo->write(blob.data().data() + wrapped_byte_count, wrapped_byte_count,
                              kRingBufferSize - wrapped_byte_count));
  ASSERT_OK(writer_vmo->write(blob.data().data() + kRingBufferSize, 0, wrapped_byte_count));
  ASSERT_OK(writer->BytesReady(kRingBufferSize));

  auto reader_vmo = blob_reader().GetVmo(blob.digest());
  ASSERT_OK(reader_vmo);
  EXPECT_EQ(GetVmoStreamSize(*reader_vmo), kRingBufferSize + 50);
}

TEST_F(BlobWriterTest, CorruptBlobFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);

  // Corrupt the last byte of the blob.
  std::vector<uint8_t> blob_copy(blob.data().begin(), blob.data().end());
  ASSERT_NE(blob_copy.back(), 0xCD);
  blob_copy.back() = 0xCD;

  ASSERT_OK(writer_vmo->write(blob_copy.data(), 0, blob_copy.size()));
  ASSERT_STATUS(writer->BytesReady(blob_copy.size()), ZX_ERR_IO_DATA_INTEGRITY);
}

TEST_F(BlobWriterTest, CorruptDeliveryBlobHeaderFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);

  // Corrupt the delivery blob header.
  std::vector<uint8_t> blob_copy(blob.data().begin(), blob.data().end());
  ASSERT_NE(blob_copy[0], 0xCD);
  blob_copy[0] = 0xCD;

  ASSERT_OK(writer_vmo->write(blob_copy.data(), 0, blob_copy.size()));
  ASSERT_STATUS(writer->BytesReady(blob_copy.size()), ZX_ERR_IO_DATA_INTEGRITY);
}

TEST_F(BlobWriterTest, CreateWithAllowExistingDeletesOld) {
  constexpr uint64_t kBlobSize = 9000;  // Big enough for compression to make a difference.
  auto blob = TestBlobData::Create(kBlobSize);
  auto uncompressed_blob = TestDeliveryBlob::CreateUncompressed(blob);
  ASSERT_STATUS(blob_reader().GetVmo(blob.digest()), ZX_ERR_NOT_FOUND);

  {
    // Doing CreateExisting despite the blob not being there. Succeeds normally.
    auto writer = blob_creator().CreateExisting(blob.digest());
    EXPECT_OK(writer);
    ASSERT_OK(writer->WriteBlob(uncompressed_blob));
  }

  // Get the current space usage.
  uint64_t old_used_bytes;
  {
    auto info = fs().GetFsInfo();
    ASSERT_OK(info);
    old_used_bytes = info->used_bytes;
    EXPECT_NE(old_used_bytes, 0ul);
  }

  auto compressed_blob = TestDeliveryBlob::CreateCompressed(blob);
  {
    auto writer = blob_creator().CreateExisting(blob.digest());
    EXPECT_OK(writer);
    EXPECT_OK(writer->WriteBlob(compressed_blob));
  }

  // Check that it can be verified.
  auto vmo = blob_reader().GetVmo(blob.digest());
  EXPECT_OK(vmo);
  char a;
  EXPECT_OK(vmo->read(&a, 0, 1));

  // Check that space usage has been recovered by replacing with a compressed blob.
  // In part to verify that the old blob has actually been deleted.
  auto info = fs().GetFsInfo();
  ASSERT_OK(info);
  EXPECT_LT(info->used_bytes, old_used_bytes);
}

TEST_F(BlobWriterTest, FailedWriteMarksBlobCorruptRecoversOnRemount) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  EXPECT_OK(blob_creator().CreateAndWriteBlob(blob));
  auto fd = fs().GetRootFd();
  // Sync to ensure that the data makes it disk.
  ASSERT_EQ(fsync(fd.get()), 0);

  {
    auto vmo = blob_reader().GetVmo(blob.digest());
    ASSERT_OK(vmo);

    auto writer = blob_creator().CreateExisting(blob.digest());

    // Sleep the disk during the write to generate a failure.
    ASSERT_OK(fs().GetRamDisk()->SleepAfter(0));
    EXPECT_TRUE(writer->WriteBlob(blob).is_error());
    ASSERT_OK(fs().GetRamDisk()->Wake());

    // The original blob is partially deleted. It cannot respond.
    char a;
    EXPECT_STATUS(vmo->read(&a, 0, 1), ZX_ERR_BAD_STATE);
  }

  // Remount to revert changes to the ondisk version.
  ASSERT_OK(Remount());

  auto vmo = blob_reader().GetVmo(blob.digest());
  ASSERT_OK(vmo);

  // Now reading works.
  char a;
  EXPECT_OK(vmo->read(&a, 0, 1));
}

TEST_F(BlobWriterTest, FailedOverwriteWithBadData) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  EXPECT_OK(blob_creator().CreateAndWriteBlob(blob));

  auto writer = blob_creator().CreateExisting(blob.digest());
  ASSERT_OK(writer);

  auto vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(vmo);

  uint64_t payload_size = blob.data().size() - 1;
  uint64_t bytes_written = 0;
  while (bytes_written < payload_size) {
    uint64_t bytes_to_write = std::min(kRingBufferSize, payload_size - bytes_written);
    ASSERT_OK(vmo->write(blob.data().data() + bytes_written, 0, bytes_to_write));
    ASSERT_OK(writer->BytesReady(bytes_to_write));
    bytes_written += bytes_to_write;
  }
  // Write a bad byte at the end of the delivery blob.
  uint8_t bad_byte = blob.data()[payload_size - 1] ^ 0xFF;
  ASSERT_OK(vmo->write(&bad_byte, 0, 1));
  EXPECT_STATUS(writer->BytesReady(1), ZX_ERR_IO_DATA_INTEGRITY);
}

// Ensure that dropping a handle and reopening a blob do not race.
TEST_F(BlobWriterTest, CloseRaceTest) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  for (int i = 0; i < 1000; ++i) {
    auto writer_or = blob_creator().Create(blob.digest());
    ASSERT_OK(writer_or);
    auto writer = std::move(writer_or.value());
    ASSERT_OK(writer.GetVmo(blob.data().size()));
  }
}

TEST_F(BlobWriterTest, CloseRaceAllowExistingTest) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  for (int i = 0; i < 1000; ++i) {
    auto writer_or = blob_creator().CreateExisting(blob.digest());
    ASSERT_OK(writer_or);
    auto writer = std::move(writer_or.value());
    ASSERT_OK(writer.GetVmo(blob.data().size()));
  }
}

TEST_F(BlobWriterTest, OverwriteCloseRaceTest) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(blob));
  for (int i = 0; i < 1000; ++i) {
    auto writer_or = blob_creator().CreateExisting(blob.digest());
    ASSERT_OK(writer_or);
    auto writer = std::move(writer_or.value());
    ASSERT_OK(writer.GetVmo(blob.data().size()));
  }
}

using BlobReaderTest = BlobCreatorTest;

TEST_F(BlobReaderTest, GetVmoForMissingBlobFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto vmo = blob_reader().GetVmo(blob.digest());
  EXPECT_STATUS(vmo, ZX_ERR_NOT_FOUND);
}

TEST_F(BlobCreatorTest, NeedsOverwriteForMissingBlobFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto result = blob_creator().NeedsOverwrite(blob.digest());
  EXPECT_STATUS(result.status_value(), ZX_ERR_NOT_FOUND);
}

TEST_F(BlobReaderTest, GetVmoForPartiallyWrittenBlobFails) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  auto writer = blob_creator().Create(blob.digest());
  ASSERT_OK(writer);
  auto writer_vmo = writer->GetVmo(blob.data().size());
  ASSERT_OK(writer_vmo);
  ASSERT_OK(writer_vmo->write(blob.data().data(), 0, blob.data().size() - 1));
  ASSERT_OK(writer->BytesReady(blob.data().size() - 1));

  auto vmo = blob_reader().GetVmo(blob.digest());
  EXPECT_STATUS(vmo, ZX_ERR_NOT_FOUND);
}

TEST_F(BlobCreatorTest, NeedsOverwriteForBlobRespondsFalse) {
  auto blob = TestDeliveryBlob::CreateUncompressed(10);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(blob));
  auto result = blob_creator().NeedsOverwrite(blob.digest());
  ASSERT_OK(result);
  ASSERT_FALSE(*result);
}

}  // namespace
}  // namespace blobfs
