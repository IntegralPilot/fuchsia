// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.paver/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <zxtest/zxtest.h>

namespace {

TEST(PaverSvcTest, PaverSvccEnumeratesSuccessfully) {
  zx::result client_end = component::Connect<fuchsia_paver::Paver>();
  ASSERT_OK(client_end);
  fidl::WireSyncClient paver(std::move(client_end.value()));

  auto [client, server] = fidl::Endpoints<fuchsia_paver::DataSink>::Create();

  const fidl::OneWayStatus result = paver->FindDataSink(std::move(server));
  ASSERT_OK(result.status());
}

}  // namespace
