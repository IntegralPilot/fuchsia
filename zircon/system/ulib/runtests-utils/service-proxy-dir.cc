// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <zircon/status.h>

#include <string>
#include <string_view>

#include <runtests-utils/service-proxy-dir.h>

namespace fio = fuchsia_io;

namespace runtests {

ServiceProxyDir::ServiceProxyDir(fidl::ClientEnd<fio::Directory> proxy_dir)
    : proxy_dir_(std::move(proxy_dir)) {}

void ServiceProxyDir::AddEntry(std::string name, fbl::RefPtr<fs::Vnode> node) {
  std::lock_guard lock(lock_);
  entries_[std::move(name)] = std::move(node);
}

fuchsia_io::NodeProtocolKinds ServiceProxyDir::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kDirectory;
}

zx_status_t ServiceProxyDir::Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out) {
  auto entry_name = std::string(name.data(), name.length());

  std::lock_guard lock(lock_);
  auto entry = entries_.find(entry_name);
  if (entry != entries_.end()) {
    *out = entry->second;
    return ZX_OK;
  }
  entries_.emplace(entry_name,
                   *out = fbl::MakeRefCounted<fs::Service>([this, entry_name](zx::channel request) {
                     return fidl::WireCall(proxy_dir_)
                         ->Open(fidl::StringView::FromExternal(entry_name),
                                fio::Flags::kProtocolService, {}, std::move(request))
                         .status();
                   }));

  return ZX_OK;
}

}  // namespace runtests
