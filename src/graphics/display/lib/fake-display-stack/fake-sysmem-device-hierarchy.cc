// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/fake-display-stack/fake-sysmem-device-hierarchy.h"

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <memory>
#include <utility>

#include "src/sysmem/server/allocator.h"
#include "src/sysmem/server/sysmem.h"

namespace fake_display {

zx::result<std::unique_ptr<FakeSysmemDeviceHierarchy>> FakeSysmemDeviceHierarchy::Create() {
  return zx::ok(std::make_unique<FakeSysmemDeviceHierarchy>());
}

FakeSysmemDeviceHierarchy::FakeSysmemDeviceHierarchy()
    : sysmem_client_loop_(&kAsyncLoopConfigNeverAttachToThread) {
  zx_status_t start_status =
      sysmem_client_loop_.StartThread("FakeSysmemDeviceHierarchy.SysmemClientDispatcher");
  ZX_ASSERT_MSG(start_status == ZX_OK, "Failed to start the Sysmem client loop: %s",
                zx_status_get_string(start_status));

  // sysmem_service::Sysmem::Create() must be called on the client loop's dispatcher.
  //
  // `sysmem_service` stores the created instance on the client loop dispatcher
  // thread, and `sysmem_service_mutex` ensures that the created instance is
  // seen by the constructor thread.
  //
  // `sysmem_service_` is written on the constructor thread, as stated in the data
  // member comment. This results in an unsurprising threading model. For example,
  // a call to ConnectAllocator2() immediately after the constructor completes is
  // guaranteed to see the written value.
  std::mutex sysmem_service_mutex;
  std::unique_ptr<sysmem_service::Sysmem> sysmem_service;

  libsync::Completion done;
  zx_status_t post_status = async::PostTask(sysmem_client_loop_.dispatcher(), [&] {
    sysmem_service::Sysmem::CreateArgs create_args;
    zx::result<std::unique_ptr<sysmem_service::Sysmem>> create_result =
        sysmem_service::Sysmem::Create(sysmem_client_loop_.dispatcher(), create_args);
    ZX_ASSERT_MSG(create_result.is_ok(), "sysmem_service::Sysmem::Create() failed: %s",
                  create_result.status_string());

    {
      std::lock_guard lock(sysmem_service_mutex);
      sysmem_service = std::move(create_result.value());
    }
    done.Signal();
  });

  ZX_ASSERT(post_status == ZX_OK);
  done.Wait();

  {
    std::lock_guard lock(sysmem_service_mutex);
    sysmem_service_ = std::move(sysmem_service);
  }
}

zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>>
FakeSysmemDeviceHierarchy::ConnectAllocator2() {
  auto [sysmem_client, sysmem_server] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
  sysmem_service_->SyncCall([this, sysmem_server = std::move(sysmem_server)]() mutable {
    sysmem_service::Allocator::CreateOwnedV2(std::move(sysmem_server), sysmem_service_.get(),
                                             sysmem_service_->v2_allocators());
  });
  return zx::ok(std::move(sysmem_client));
}

FakeSysmemDeviceHierarchy::~FakeSysmemDeviceHierarchy() {
  // The sysmem_service::Sysmem instance must be destroyed on the client loop's dispatcher.
  libsync::Completion done;
  zx_status_t post_status = async::PostTask(sysmem_client_loop_.dispatcher(), [this, &done] {
    sysmem_service_.reset();
    done.Signal();
  });
  ZX_ASSERT(post_status == ZX_OK);
  done.Wait();
}

}  // namespace fake_display
