// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <cstddef>
#include <cstdint>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/pseudo_file.h"
#include "src/storage/lib/vfs/cpp/tests/dir_test_util.h"
#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"

namespace {

namespace fio = fuchsia_io;

TEST(PseudoDir, ApiTest) {
  auto dir = fbl::MakeRefCounted<fs::PseudoDir>();
  auto subdir = fbl::MakeRefCounted<fs::PseudoDir>();
  auto file1 = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>();
  auto file2 = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>();

  // add entries
  EXPECT_EQ(ZX_OK, dir->AddEntry("subdir", subdir));
  EXPECT_EQ(ZX_OK, dir->AddEntry("file1", file1));
  EXPECT_EQ(ZX_OK, dir->AddEntry("file2", file2));
  EXPECT_EQ(ZX_OK, dir->AddEntry("file2b", file2));

  // try to add duplicates
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, dir->AddEntry("subdir", subdir));
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, dir->AddEntry("file1", subdir));

  // remove entries
  EXPECT_EQ(ZX_OK, dir->RemoveEntry("file2"));
  EXPECT_EQ(ZX_ERR_NOT_FOUND, dir->RemoveEntry("file2"));

  // verify node protocol type and rights (directories should support all rights)
  EXPECT_EQ(dir->GetProtocols(), fuchsia_io::NodeProtocolKinds::kDirectory);
  EXPECT_TRUE(dir->ValidateRights(fuchsia_io::Rights{uint64_t{fuchsia_io::kMaskKnownPermissions}}));

  // verify node attributes
  zx::result attr = dir->GetAttributes();
  ASSERT_TRUE(attr.is_ok());
  EXPECT_EQ(*attr, fs::VnodeAttributes{});

  // lookup entries
  fbl::RefPtr<fs::Vnode> node;
  EXPECT_EQ(ZX_OK, dir->Lookup("subdir", &node));
  EXPECT_EQ(subdir.get(), node.get());
  EXPECT_EQ(ZX_OK, dir->Lookup("file1", &node));
  EXPECT_EQ(file1.get(), node.get());
  EXPECT_EQ(ZX_ERR_NOT_FOUND, dir->Lookup("file2", &node));
  EXPECT_EQ(ZX_OK, dir->Lookup("file2b", &node));
  EXPECT_EQ(file2.get(), node.get());

  // readdir
  {
    fs::VdirCookie cookie;
    uint8_t buffer[4096];
    size_t length;
    EXPECT_EQ(dir->Readdir(&cookie, buffer, sizeof(buffer), &length), ZX_OK);
    fs::DirentChecker dc(buffer, length);
    dc.ExpectEntry(".", fio::DirentType::kDirectory);
    dc.ExpectEntry("subdir", fio::DirentType::kDirectory);
    dc.ExpectEntry("file1", fio::DirentType::kFile);
    dc.ExpectEntry("file2b", fio::DirentType::kFile);
    dc.ExpectEnd();
  }

  // readdir with small buffer
  {
    fs::VdirCookie cookie;
    uint8_t buffer[(2 * sizeof(fs::DirectoryEntry)) + 13];
    size_t length;
    EXPECT_EQ(dir->Readdir(&cookie, buffer, sizeof(buffer), &length), ZX_OK);
    fs::DirentChecker dc(buffer, length);
    dc.ExpectEntry(".", fio::DirentType::kDirectory);
    dc.ExpectEntry("subdir", fio::DirentType::kDirectory);
    dc.ExpectEnd();

    // readdir again
    EXPECT_EQ(dir->Readdir(&cookie, buffer, sizeof(buffer), &length), ZX_OK);
    fs::DirentChecker dc1(buffer, length);
    dc1.ExpectEntry("file1", fio::DirentType::kFile);
    dc1.ExpectEntry("file2b", fio::DirentType::kFile);
    dc1.ExpectEnd();
  }

  // test removed entries do not appear in readdir or lookup
  dir->RemoveEntry("file1");
  {
    fs::VdirCookie cookie;
    uint8_t buffer[4096];
    size_t length;
    EXPECT_EQ(dir->Readdir(&cookie, buffer, sizeof(buffer), &length), ZX_OK);
    fs::DirentChecker dc(buffer, length);
    dc.ExpectEntry(".", fio::DirentType::kDirectory);
    dc.ExpectEntry("subdir", fio::DirentType::kDirectory);
    dc.ExpectEntry("file2b", fio::DirentType::kFile);
    dc.ExpectEnd();
  }
  EXPECT_EQ(ZX_ERR_NOT_FOUND, dir->Lookup("file1", &node));

  // remove all entries
  dir->RemoveAllEntries();

  // readdir again
  {
    fs::VdirCookie cookie;
    uint8_t buffer[4096];
    size_t length;
    EXPECT_EQ(dir->Readdir(&cookie, buffer, sizeof(buffer), &length), ZX_OK);
    fs::DirentChecker dc(buffer, length);
    dc.ExpectEntry(".", fio::DirentType::kDirectory);
    dc.ExpectEnd();
  }
}

}  // namespace
