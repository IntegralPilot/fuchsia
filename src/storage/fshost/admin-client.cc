// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/fshost/admin-client.h"

#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.sys2/cpp/common_types.h>
#include <fidl/fuchsia.sys2/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

namespace fshost {
zx::result<fidl::ClientEnd<fuchsia_fshost::Admin>> ConnectToAdmin() {
  constexpr char kRealmQueryServicePath[] = "/svc/fuchsia.sys2.RealmQuery.root";
  constexpr char kFshostMoniker[] = "./bootstrap/fshost";

  // Connect to the root RealmQuery and get instance directories of fshost
  zx::result<fidl::ClientEnd<fuchsia_sys2::RealmQuery>> query_client_end =
      component::Connect<fuchsia_sys2::RealmQuery>(kRealmQueryServicePath);
  if (query_client_end.is_error()) {
    return query_client_end.take_error();
  }

  auto [exposed_client, exposed_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto res = fidl::WireCall(query_client_end.value())
                 ->OpenDirectory(kFshostMoniker, fuchsia_sys2::OpenDirType::kExposedDir,
                                 std::move(exposed_server));

  if (!res.ok()) {
    return zx::error(res.status());
  }
  if (res.value().is_error()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  // Connect to Admin protocol from exposed dir of fshost
  return component::ConnectAt<fuchsia_fshost::Admin>(exposed_client.borrow());
}
}  // namespace fshost
