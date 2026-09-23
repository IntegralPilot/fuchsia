// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/markers.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/scheduler/role.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/process.h>
#include <zircon/processargs.h>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode_dir.h"  // IWYU pragma: keep

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  zx::result result = memfs::Memfs::Create(loop.dispatcher(), "<tmp>");
  if (result.is_error()) {
    FX_PLOGS(FATAL, result.status_value()) << "Memfs::Create failed";
  }
  auto& [tmp, tmp_vnode] = result.value();

  auto outgoing_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  outgoing_dir->AddEntry("root", tmp_vnode);

  tmp->ServeDirectory(outgoing_dir, fidl::ServerEnd<fuchsia_io::Directory>(
                                        zx::channel(zx_take_startup_handle(PA_DIRECTORY_REQUEST))));

  fuchsia_scheduler::SetRoleForThisThread("fuchsia.storage.memfs.main");

  loop.Run();

  return 0;
}
