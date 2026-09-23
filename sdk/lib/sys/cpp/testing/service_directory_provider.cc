// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/sys/cpp/testing/service_directory_provider.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/service.h>

#include <utility>

namespace sys {
namespace testing {

ServiceDirectoryProvider::ServiceDirectoryProvider(async_dispatcher_t* dispatcher)
    : svc_dir_(std::make_unique<vfs::PseudoDir>()) {
  auto [svc_client, svc_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  svc_dir_->Serve(fuchsia_io::wire::kPermReadable, std::move(svc_server), dispatcher);
  service_directory_ = std::make_shared<sys::ServiceDirectory>(svc_client.TakeChannel());
}

ServiceDirectoryProvider::~ServiceDirectoryProvider() = default;

zx_status_t ServiceDirectoryProvider::AddService(std::unique_ptr<vfs::Service> service,
                                                 std::string name) const {
  return svc_dir_->AddEntry(std::move(name), std::move(service));
}

zx_status_t ServiceDirectoryProvider::AddService(Connector connector,
                                                 std::string service_name) const {
  return AddService(std::make_unique<vfs::Service>(std::move(connector)), std::move(service_name));
}

}  // namespace testing
}  // namespace sys
