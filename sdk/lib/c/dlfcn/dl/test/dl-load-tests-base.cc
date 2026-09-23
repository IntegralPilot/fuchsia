// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-load-tests-base.h"

#include <fcntl.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/ld/abi.h>

#include <filesystem>

#include <gtest/gtest.h>

#include "startup-symbols.h"

namespace dl::testing {
namespace {

const std::filesystem::path kTestDir =
    elfldltl::testing::GetTestDataPath("sdk/lib/ld/test/modules/dl-tests");

bool TestLibExists(std::string_view name) { return std::filesystem::exists(kTestDir / name); }

}  // namespace

void DlLoadTestsBase::ExpectRootModule(std::string_view name) {
  ASSERT_TRUE(TestLibExists(name)) << name;
}

void DlLoadTestsBase::ExpectMissing(std::string_view name) {
  ASSERT_FALSE(TestLibExists(name)) << name;
}

void DlLoadTestsBase::Needed(std::initializer_list<std::string_view> names) {
  // The POSIX dynamic linker will just do `open` system calls to find files.
  // It runs chdir'd to the directory where they're found.  Nothing else done
  // here in the test harness affects the lookups it does or verifies that it
  // does the expected set in the expected order.  So this just verifies that
  // each SONAME in the list is an existing test file.
  for (std::string_view name : names) {
    ASSERT_TRUE(TestLibExists(name)) << name;
  }
}

void DlLoadTestsBase::Needed(
    std::initializer_list<std::pair<std::string_view, bool>> name_found_pairs) {
  for (auto [name, found] : name_found_pairs) {
    if (found) {
      ASSERT_TRUE(TestLibExists(name)) << name;
    } else {
      ASSERT_FALSE(TestLibExists(name)) << name;
    }
  }
}

void DlLoadTestsBase::FileCheck(std::string_view filename) {
  EXPECT_NE(filename, ld::abi::Abi<>::kSoname.str());
}

fit::result<std::optional<DlLoadTestsBase::SystemError>, DlLoadTestsBase::File>
DlLoadTestsBase::RetrieveFile(Diagnostics& diag, std::string_view filename) {
  FileCheck(filename);
  const std::filesystem::path path = kTestDir / filename;
  if (fbl::unique_fd fd{open(path.c_str(), O_RDONLY)}) {
    return fit::ok(File{std::move(fd), diag});
  }
  // The only expected failure is a "not found" error.
  EXPECT_EQ(errno, ENOENT);
  return fit::error(std::nullopt);
}

void DlLoadTestsBase::TearDown() {
  for (auto& [ptr, info] : opened_modules()) {
    ADD_FAILURE() << info.refcnt << " calls to dlopen(\"" << info.name
                  << "\") without corresponding dlclose(" << ptr << ")";
    while (--info.refcnt) {
      UntrackModule(ptr);
      CleanUpOpenedFile(ptr);
    }
  }
}

void DlLoadTestsBase::TrackModule(void* module, const char* filename) {
  std::string file = filename ? filename : "<null>";
  if (auto it = opened_modules_.find(module); it != opened_modules_.end()) {
    // The file has already been opened in this test, just increment the refcount.
    it->second.refcnt++;
  } else {
    opened_modules_.insert({module, {.name = std::move(file), .refcnt = 1}});
  }
}

void DlLoadTestsBase::UntrackModule(void* file) {
  auto it = opened_modules_.find(file);
  ASSERT_NE(it, opened_modules_.end());
  ASSERT_GT(it->second.refcnt, 0u);
  // Decrement the refcount of the file. If it reaches 0, then erase the entry
  // from the map.
  if (!(--it->second.refcnt)) {
    opened_modules_.erase(it);
  }
}

}  // namespace dl::testing
