// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/stat.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <utility>

#include <fbl/ref_ptr.h>
#include <fbl/string.h>
#include <fbl/vector.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/lazy_dir.h"
#include "src/storage/lib/vfs/cpp/pseudo_file.h"
#include "src/storage/lib/vfs/cpp/tests/dir_test_util.h"
#include "src/storage/lib/vfs/cpp/vfs.h"

namespace {

namespace fio = fuchsia_io;

class TestLazyDirHelper : public fs::LazyDir {
 public:
  struct TestContent {
    uint64_t id;
    fbl::String name;
  };

  void GetContents(LazyEntryVector* out_vector) override {
    out_vector->reserve(contents_.size());
    for (const auto& content : contents_) {
      out_vector->push_back({content.id, content.name, S_IFREG});
    }
  }

  zx_status_t GetFile(fbl::RefPtr<fs::Vnode>* out, uint64_t id, fbl::String name) override {
    last_output_file = fbl::MakeRefCounted<fs::UnbufferedPseudoFile>();
    *out = last_output_file;
    last_id = id;
    last_name = name;
    return ZX_OK;
  }

  void AddContent(TestContent content) { contents_.push_back(std::move(content)); }

  fbl::RefPtr<fs::Vnode> last_output_file;
  uint64_t last_id;
  fbl::String last_name;

 private:
  fbl::Vector<TestContent> contents_;
};

TEST(LazyDir, ApiTest) {
  TestLazyDirHelper test;

  {
    fs::VdirCookie cookie;
    uint8_t buffer[4096];
    size_t len;

    EXPECT_EQ(test.Readdir(&cookie, buffer, sizeof(buffer), &len), ZX_OK);
    fs::DirentChecker dc(buffer, len);
    dc.ExpectEntry(".", fio::DirentType::kDirectory);
    dc.ExpectEnd();
  }

  test.AddContent({1, "test"});
  {
    fs::VdirCookie cookie;
    uint8_t buffer[4096];
    size_t len;

    EXPECT_EQ(test.Readdir(&cookie, buffer, sizeof(buffer), &len), ZX_OK);
    fs::DirentChecker dc(buffer, len);
    dc.ExpectEntry(".", fio::DirentType::kDirectory);
    dc.ExpectEntry("test", fio::DirentType::kFile);
    dc.ExpectEnd();

    fbl::RefPtr<fs::Vnode> out;
    EXPECT_EQ(test.Lookup("test", &out), ZX_OK);
    EXPECT_EQ(test.last_id, 1u);
    EXPECT_TRUE(strcmp("test", test.last_name.c_str()) == 0);
    EXPECT_EQ(out.get(), test.last_output_file.get());

    EXPECT_EQ(test.Lookup("test2", &out), ZX_ERR_NOT_FOUND);
  }
  test.AddContent({33, "aaaa"});
  {
    fs::VdirCookie cookie;
    uint8_t buffer[4096];
    size_t len;

    EXPECT_EQ(test.Readdir(&cookie, buffer, sizeof(buffer), &len), ZX_OK);
    fs::DirentChecker dc(buffer, len);
    dc.ExpectEntry(".", fio::DirentType::kDirectory);
    dc.ExpectEntry("test", fio::DirentType::kFile);
    dc.ExpectEntry("aaaa", fio::DirentType::kFile);
    dc.ExpectEnd();

    fbl::RefPtr<fs::Vnode> out;
    EXPECT_EQ(test.Lookup("aaaa", &out), ZX_OK);
    EXPECT_EQ(test.last_id, 33u);
    EXPECT_TRUE(strcmp("aaaa", test.last_name.c_str()) == 0);
    EXPECT_EQ(out.get(), test.last_output_file.get());
  }
  {
    // Ensure manually setting cookie past entries excludes them, but leaves "."
    fs::VdirCookie cookie;
    cookie.n = 30;
    uint8_t buffer[4096];
    size_t len;

    EXPECT_EQ(test.Readdir(&cookie, buffer, sizeof(buffer), &len), ZX_OK);
    fs::DirentChecker dc(buffer, len);
    dc.ExpectEntry(".", fio::DirentType::kDirectory);
    dc.ExpectEntry("aaaa", fio::DirentType::kFile);
    dc.ExpectEnd();

    // Expect that "." is missing when reusing the cookie.
    EXPECT_EQ(test.Readdir(&cookie, buffer, sizeof(buffer), &len), ZX_OK);
    dc = fs::DirentChecker(buffer, len);
    dc.ExpectEnd();
  }
}

}  // namespace
