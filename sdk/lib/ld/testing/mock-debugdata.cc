// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/ld/testing/mock-debugdata.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/defer.h>
#include <zircon/status.h>

#include <gmock/gmock.h>

#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

namespace ld::testing {

class MockSvcDirectory::Impl {
 public:
  void Init() {}

  void AddEntry(std::string_view name, MetaConnector metaconnector) {
    Connector connector = std::move(metaconnector)(loop_.dispatcher());
    auto node = fbl::MakeRefCounted<fs::Service>(std::move(connector));
    zx_status_t status = dir_->AddEntry(name, std::move(node));
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }

  void Serve(fidl::ServerEnd<fuchsia_io::Directory> server_end) {
    zx_status_t status = vfs_.ServeDirectory(dir_, std::move(server_end), fuchsia_io::kRwStarDir);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }

  async::Loop& loop() { return loop_; }

 private:
  async::Loop loop_{&kAsyncLoopConfigAttachToCurrentThread};
  fbl::RefPtr<fs::PseudoDir> dir_ = fbl::MakeRefCounted<fs::PseudoDir>();
  fs::SynchronousVfs vfs_{loop_.dispatcher()};
};

void MockDebugdata::Publish(PublishRequest& request, PublishCompleter::Sync&) {
  Publish(request.data_sink(), std::move(request.data()), std::move(request.vmo_token()));
}

MockSvcDirectory::MockSvcDirectory() = default;

MockSvcDirectory::MockSvcDirectory(MockSvcDirectory&&) = default;

MockSvcDirectory::~MockSvcDirectory() = default;

void MockSvcDirectory::Init() {
  ASSERT_FALSE(impl_) << "ld::testing::MockSvcDirectory::Init() called twice";
  impl_ = std::make_unique<Impl>();
  impl_->Init();
}

void MockSvcDirectory::AddEntry(std::string_view name, MetaConnector metaconnector) {
  ASSERT_TRUE(impl_) << "ld::testing::MockSvcDirectory::Init() not called";
  impl_->AddEntry(name, std::move(metaconnector));
}

void MockSvcDirectory::Serve(fidl::ServerEnd<fuchsia_io::Directory> server_end) {
  ASSERT_TRUE(impl_) << "ld::testing::MockSvcDirectory::Init() not called";
  impl_->Serve(std::move(server_end));
}

void MockSvcDirectory::Serve(fidl::ClientEnd<fuchsia_io::Directory>& client_end) {
  ASSERT_TRUE(impl_) << "ld::testing::MockSvcDirectory::Init() not called";
  zx::result server_end = fidl::CreateEndpoints(&client_end);
  ASSERT_TRUE(server_end.is_ok()) << server_end.status_string();
  impl_->Serve(*std::move(server_end));
}

async::Loop& MockSvcDirectory::loop() { return impl_->loop(); }

}  // namespace ld::testing
