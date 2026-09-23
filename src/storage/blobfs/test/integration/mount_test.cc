// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <unistd.h>

#include <cerrno>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"
#include "src/storage/blobfs/test/integration/fdio_test.h"

namespace blobfs {
namespace {

// Uses the default layout of kDataRootOnly.
using DataMountTest = BlobfsTest;

// Variant that sets the layout to kExportDirectory.
using OutgoingMountTest = FdioTest;

// merkle root for a file. in order to create a file on blobfs we need the filename to be a valid
// merkle root whether or not we ever write the content.
//
// This is valid enough to create files but it is unknown what content this was generated
// from. Previously this comment said it was "test content" but that seems to be incorrect.
constexpr char kFileName[] = "be901a14ec42ee0a8ee220eb119294cdd40d26d573139ee3d51e4430e7d08c28";

TEST_F(DataMountTest, DataRootHasNoRootDirectoryInIt) {
  errno = 0;
  fbl::unique_fd no_fd(openat(root_fd(), kOutgoingDataRoot, O_RDONLY));
  ASSERT_FALSE(no_fd.is_valid());
}

TEST_F(OutgoingMountTest, OutgoingDirectoryHasRootDirectoryInIt) {
  fbl::unique_fd foo_fd(openat(outgoing_dir_fd(), kOutgoingDataRoot, O_DIRECTORY));
  ASSERT_TRUE(foo_fd.is_valid());
}

TEST_F(OutgoingMountTest, OutgoingDirectoryIsReadOnly) {
  fbl::unique_fd no_fd(openat(outgoing_dir_fd(), kFileName, O_CREAT, S_IRUSR | S_IWUSR));
  ASSERT_FALSE(no_fd.is_valid());
}

TEST_F(DataMountTest, OutgoingDirectoryCanListAndUnlinkBlobs) {
  auto blob = TestDeliveryBlob::CreateUncompressed(20);
  ASSERT_OK(blob_creator().CreateAndWriteBlob(blob).status_value());
  ASSERT_THAT(ListBlobs(), testing::ElementsAre(blob.digest()));
  ASSERT_OK(Unlink(blob.digest()));
}

}  // namespace
}  // namespace blobfs
