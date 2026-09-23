// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/sys/service/cpp/service_handler.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/service.h>

namespace sys {

ServiceHandler::ServiceHandler() noexcept : dir_(std::make_unique<vfs::PseudoDir>()) {}
ServiceHandler::ServiceHandler(ServiceHandler&&) = default;

ServiceHandler::~ServiceHandler() = default;

zx_status_t ServiceHandler::AddMember(std::string member, MemberHandler handler) const {
  return dir_->AddEntry(std::move(member), std::make_unique<vfs::Service>(std::move(handler)));
}

}  // namespace sys
