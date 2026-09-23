// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/fs/hack_filesystem.h"

#include <gtest/gtest.h>

#ifdef __Fuchsia__
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/pseudo_file.h>
#include <string.h>

#include <memory>
#include <string>
#include <vector>
#endif

namespace {
using namespace escher;

TEST(HackFilesystem, Init) {
  auto fs = HackFilesystem::New();

  HackFileContents contents = fs->ReadFile("shaders/model_renderer/main.vert");
  EXPECT_GT(contents.size(), 0U);
  EXPECT_EQ(contents.substr(0, 12), "#version 450");
}

#ifdef __Fuchsia__
TEST(HackFilesystem, InitWithRealFilesInDir) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto root_dir = std::make_unique<vfs::PseudoDir>();
  auto subdir = std::make_unique<vfs::PseudoDir>();

  const std::string kContent1 = "something";
  const std::string kContent2 = "other thing";
  const std::string kDir = "subdir";
  const std::string kFile1 = "test_file1.txt";
  const std::string kFile2 = "test_file2.txt";
  const std::string kPath1 = kDir + "/" + kFile1;
  const std::string kPath2 = kFile2;
  const std::string kInvalidPath = kDir + "/" + kFile2;

  auto make_file = [](const std::string& content) {
    return std::make_unique<vfs::PseudoFile>(
        content.size(), [content](std::vector<uint8_t>* output, size_t max_file_size) {
          output->resize(content.size());
          memcpy(output->data(), content.data(), output->size());
          return ZX_OK;
        });
  };

  auto file1 = make_file(kContent1);
  auto file2 = make_file(kContent2);

  subdir->AddEntry(kFile1, std::move(file1));
  root_dir->AddEntry(kDir, std::move(subdir));
  root_dir->AddEntry(kFile2, std::move(file2));

  // HackFilesystem uses a SyncClient which blocks until the server (on our loop)
  // responds, so run the loop in a background thread.
  loop.StartThread("vfs-thread");

  {
    auto [client, server] = *fidl::CreateEndpoints<fuchsia_io::Directory>();
    root_dir->Serve(fuchsia_io::kPermReadable, std::move(server), loop.dispatcher());

    auto fs = HackFilesystem::New(std::move(client));
    EXPECT_EQ(fs->ReadFile(kPath1), kContent1);
    EXPECT_EQ(fs->ReadFile(kPath2), kContent2);
  }
  {
    auto [client, server] = *fidl::CreateEndpoints<fuchsia_io::Directory>();
    root_dir->Serve(fuchsia_io::kPermReadable, std::move(server), loop.dispatcher());

    auto fs = HackFilesystem::New(std::move(client));
    EXPECT_EQ(fs->ReadFile(kPath1), kContent1);
    EXPECT_EQ(fs->ReadFile(kPath2), kContent2);
  }

  loop.Shutdown();
}
#endif

}  // namespace
