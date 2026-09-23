// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_TESTS_DIR_TEST_UTIL_H_
#define SRC_STORAGE_LIB_VFS_CPP_TESTS_DIR_TEST_UTIL_H_

#include <cstddef>
#include <cstdint>
#include <cstring>

#include <gtest/gtest.h>
#include <src/storage/lib/vfs/cpp/vfs_types.h>

namespace fs {
// Helper class to check entries of a directory
// Usage example:
//    fs::PseudoDir* test; // Test directory has SampleDir and SampleFile.
//    uint8_t buffer[256];
//    size_t len;
//    EXPECT_EQ(test->Readdir(&cookie, buffer, sizeof(buffer), &len), ZX_OK);
//    fs::DirentChecker dc(buffer, len);
//    dc.ExpectEntry(".", fuchsia_io::DirentType::kDirectory);
//    dc.ExpectEntry("SampleDir", fuchsia_io::DirentType::kDirectory);
//    dc.ExpectEntry("SampleFile",fuchsia_io::DirentType::kFile);
//    dc.ExpectEnd();
//
class DirentChecker {
 public:
  DirentChecker(const uint8_t* buffer, size_t length) : current_(buffer), remaining_(length) {}

  void ExpectEnd() const { EXPECT_EQ(0u, remaining_); }

  void ExpectEntry(std::string_view name, fuchsia_io::DirentType type) {
    ASSERT_NE(0u, remaining_);
    auto entry = reinterpret_cast<const fs::DirectoryEntry*>(current_);
    size_t entry_size = entry->name_length + sizeof(fs::DirectoryEntry);
    ASSERT_GE(remaining_, entry_size);
    current_ += entry_size;
    remaining_ -= entry_size;
    EXPECT_EQ(name, std::string_view(entry->name, entry->name_length));
    EXPECT_EQ(type, entry->type);
  }

 private:
  const uint8_t* current_;
  size_t remaining_;
};
}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_TESTS_DIR_TEST_UTIL_H_
