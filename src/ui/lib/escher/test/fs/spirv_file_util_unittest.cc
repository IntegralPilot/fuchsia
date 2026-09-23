// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/shaders/util/spirv_file_util.h"

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/pseudo_file.h>
#include <string.h>

#include <memory>
#include <string>
#include <vector>

#include <gtest/gtest.h>

namespace {
using namespace escher;

TEST(SpirvFileUtil, ReadSpirvFromDiskAtDir) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto root_dir = std::make_unique<vfs::PseudoDir>();

  const std::vector<uint32_t> kContent = {0x12345678, 0xABCDEF01};
  const std::string kShaderName = "test_shader";
  ShaderVariantArgs args;
  std::string expected_filename = kShaderName + std::to_string(args.hash().val) + ".spirv";

  auto file = std::make_unique<vfs::PseudoFile>(
      kContent.size() * sizeof(uint32_t),
      [kContent](std::vector<uint8_t>* output, size_t max_file_size) {
        output->resize(kContent.size() * sizeof(uint32_t));
        memcpy(output->data(), kContent.data(), output->size());
        return ZX_OK;
      });

  root_dir->AddEntry(expected_filename, std::move(file));

  auto [client, server] = *fidl::CreateEndpoints<fuchsia_io::Directory>();
  root_dir->Serve(fuchsia_io::kPermReadable, std::move(server), loop.dispatcher());

  // |ReadSpirvFromDiskAtDir| uses a SyncClient which blocks until the server (on our loop)
  // responds, so run the loop in a background thread.
  loop.StartThread("vfs-thread");

  fidl::SyncClient<fuchsia_io::Directory> sync_client(std::move(client));
  std::vector<uint32_t> out_spirv;
  EXPECT_TRUE(shader_util::ReadSpirvFromDiskAtDir(args, sync_client, kShaderName, &out_spirv));
  EXPECT_EQ(out_spirv, kContent);

  loop.Shutdown();
}

}  // namespace
