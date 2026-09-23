// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/testing/runtime_dir_helper.h"

#include <gtest/gtest.h>

#include "fbl/ref_ptr.h"
#include "fidl/fuchsia.io/cpp/fidl.h"
#include "fidl/fuchsia.io/cpp/markers.h"
#include "fidl/fuchsia.io/cpp/natural_types.h"
#include "lib/fidl/cpp/wire/channel.h"
#include "lib/fidl/cpp/wire/internal/transport_channel.h"
#include "lib/syslog/cpp/macros.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/pseudo_file.h"

namespace testing {

namespace {
fbl::RefPtr<fs::PseudoDir> MakeElfJobIdFile(zx_koid_t job) {
  auto job_id_file =
      fbl::MakeRefCounted<fs::UnbufferedPseudoFile>([job](fbl::String* output) -> zx_status_t {
        *output = std::to_string(job);
        return ZX_OK;
      });

  auto elf_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  FX_CHECK(elf_dir->AddEntry("job_id", std::move(job_id_file)) == ZX_OK);

  auto job_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  FX_CHECK(job_dir->AddEntry("elf", std::move(elf_dir)) == ZX_OK);

  return job_dir;
}

fuchsia_io::OpenableOpenRequest MakeDirOpenRequest(
    std::string_view dirname, fidl::ServerEnd<fuchsia_io::Directory> server_end) {
  fuchsia_io::OpenableOpenRequest r;
  r.path(std::string{dirname});
  r.flags(fuchsia_io::kPermReadable);
  r.object(server_end.TakeChannel());
  return r;
}
}  // namespace

RuntimeDirHelper::~RuntimeDirHelper() { Cleanup(); }

void RuntimeDirHelper::Start(async_dispatcher_t* client_dispatcher) {
  ASSERT_FALSE(root_dir_client_.is_valid());
  ASSERT_TRUE(root_dir_);

  auto [client_end, server_end] = *fidl::CreateEndpoints<fuchsia_io::Directory>();
  ASSERT_EQ(vfs_.ServeDirectory(std::move(root_dir_), std::move(server_end)), ZX_OK);
  ASSERT_EQ(loop_.StartThread(), ZX_OK);

  // |root_dir_| is now invalid.

  root_dir_client_.Bind(std::move(client_end), client_dispatcher);
}

void RuntimeDirHelper::Cleanup() {
  vfs_.Shutdown([loop = &loop_](zx_status_t status) {
    ASSERT_EQ(status, ZX_OK);
    loop->Quit();
  });

  loop_.JoinThreads();
}

void RuntimeDirHelper::AddJobIdFile(zx_koid_t job) {
  ASSERT_FALSE(root_dir_client_.is_valid());
  ASSERT_TRUE(root_dir_);

  auto job_dir = MakeElfJobIdFile(job);

  ASSERT_EQ(root_dir_->AddEntry(std::to_string(job), std::move(job_dir)), ZX_OK);
}

fidl::ClientEnd<fuchsia_io::Directory> RuntimeDirHelper::GetScopedDirectoryHandle(zx_koid_t job) {
  FX_CHECK(root_dir_client_.is_valid());
  FX_CHECK(!root_dir_);

  auto [client_end, server_end] = *fidl::CreateEndpoints<fuchsia_io::Directory>();
  FX_CHECK(root_dir_client_->Open(MakeDirOpenRequest(std::to_string(job), std::move(server_end)))
               .is_ok());

  return std::move(client_end);
}

}  // namespace testing
