// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/fs_test/crypt_service.h"

#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>

#include "sdk/lib/syslog/cpp/macros.h"

namespace fs_test {

namespace {

zx::result<> SetUpCryptWithRandomKeys() {
  fidl::WireSyncClient<fuchsia_fxfs::CryptManagement> client;
  {
    zx::result management_service = component::Connect<fuchsia_fxfs::CryptManagement>();
    if (management_service.is_error()) {
      FX_LOGS(ERROR) << "Unable to connect to crypt management service: "
                     << management_service.status_string();
      return management_service.take_error();
    }
    client = fidl::WireSyncClient(*std::move(management_service));
  }
  unsigned char key[32];
  zx_cprng_draw(key, sizeof(key));
  fidl::Array<uint8_t, 16> wrapping_key_id_0 = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
  if (auto result = client->AddWrappingKey(wrapping_key_id_0,
                                           fidl::VectorView<unsigned char>::FromExternal(key));
      !result.ok()) {
    FX_LOGS(ERROR) << "Failed to add wrapping key: " << result.status_string();
    return zx::error(result.status());
  }
  zx_cprng_draw(key, sizeof(key));
  fidl::Array<uint8_t, 16> wrapping_key_id_1 = {1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
  if (auto result = client->AddWrappingKey(wrapping_key_id_1,
                                           fidl::VectorView<unsigned char>::FromExternal(key));
      !result.ok()) {
    FX_LOGS(ERROR) << "Failed to add wrapping key: " << result.status_string();
    return zx::error(result.status());
  }
  if (auto result = client->SetActiveKey(fuchsia_fxfs::wire::KeyPurpose::kData, wrapping_key_id_0);
      !result.ok()) {
    FX_LOGS(ERROR) << "Failed to set active data key: " << result.status_string();
    return zx::error(result.status());
  }
  if (auto result =
          client->SetActiveKey(fuchsia_fxfs::wire::KeyPurpose::kMetadata, wrapping_key_id_1);
      !result.ok()) {
    FX_LOGS(ERROR) << "Failed to set active data key: " << result.status_string();
    return zx::error(result.status());
  }
  return zx::ok();
}

}  // namespace

zx::result<fidl::ClientEnd<fuchsia_fxfs::Crypt>> InitializeCryptService() {
  static bool initialized = false;
  if (!initialized) {
    if (auto status = SetUpCryptWithRandomKeys(); status.is_error()) {
      return status.take_error();
    }
    initialized = true;
  }
  zx::result crypt_service = component::Connect<fuchsia_fxfs::Crypt>();
  if (crypt_service.is_error()) {
    FX_LOGS(ERROR) << "Unable to connect to the crypt service: " << crypt_service.status_string();
  }
  return crypt_service;
}

}  // namespace fs_test

extern "C" {

// Exported for Rust
zx_status_t get_crypt_service(zx_handle_t* handle) {
  zx::result crypt_service = fs_test::InitializeCryptService();
  if (crypt_service.is_error()) {
    return crypt_service.error_value();
  }
  *handle = crypt_service->TakeChannel().release();
  return ZX_OK;
}

}  // extern
