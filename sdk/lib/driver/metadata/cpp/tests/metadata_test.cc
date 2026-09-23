// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_metadata::test {

class FakeMetadataServer final : public fidl::Server<fuchsia_driver_metadata::Metadata> {
 public:
  void GetPersistedMetadata(GetPersistedMetadataCompleter::Sync& completer) override {
    if (!persisted_metadata_.has_value()) {
      completer.Reply(fit::error(ZX_ERR_NOT_FOUND));
      return;
    }
    completer.Reply(fit::ok(persisted_metadata_.value()));
  }

  void SetMetadata(const fuchsia_hardware_test::Metadata& metadata) {
    fit::result persisted_metadata = fidl::Persist(metadata);
    ASSERT_TRUE(persisted_metadata.is_ok());
    persisted_metadata_.emplace(std::move(persisted_metadata.value()));
  }

  void Serve(fdf::OutgoingDirectory& outgoing, async_dispatcher_t* dispatcher) {
    fuchsia_driver_metadata::Service::InstanceHandler handler{
        {.metadata = bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure)}};
    ASSERT_OK(outgoing.component().AddService(std::move(handler),
                                              fuchsia_hardware_test::Metadata::kSerializableName));
  }

 private:
  std::optional<std::vector<uint8_t>> persisted_metadata_;
  fidl::ServerBindingGroup<fuchsia_driver_metadata::Metadata> bindings_;
};

class IncomingNamespace {
 public:
  void Serve(fidl::ServerEnd<fuchsia_io::Directory> server) {
    ASSERT_OK(outgoing_.Serve(std::move(server)));
  }

  void SetMetadata(const fuchsia_hardware_test::Metadata& metadata) {
    ASSERT_TRUE(metadata_server_.has_value());
    metadata_server_->SetMetadata(metadata);
  }

  void StartMetadataServer() {
    ASSERT_FALSE(metadata_server_.has_value());
    metadata_server_.emplace();
    metadata_server_->Serve(outgoing_, fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

 private:
  std::optional<FakeMetadataServer> metadata_server_;
  fdf::OutgoingDirectory outgoing_;
};

class MetadataTest : public ::testing::Test {
 protected:
  void InitIncomingNamespace(bool start_metadata_server) {
    if (start_metadata_server) {
      incoming_namespace_.SyncCall(&IncomingNamespace::StartMetadataServer);
    }

    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    incoming_namespace_.SyncCall(&IncomingNamespace::Serve, std::move(server));

    std::vector<fuchsia_component_runner::ComponentNamespaceEntry> namespace_entries;
    namespace_entries.emplace_back(fuchsia_component_runner::ComponentNamespaceEntry{
        {.path = "/", .directory = std::move(client)}});
    zx::result incoming = fdf::Namespace::Create(namespace_entries);
    ASSERT_OK(incoming);
    incoming_.emplace(std::move(incoming.value()));
  }

  void SetIncomingMetadata(const fuchsia_hardware_test::Metadata& metadata) {
    incoming_namespace_.SyncCall(&IncomingNamespace::SetMetadata, std::move(metadata));
  }

  const fdf::Namespace& incoming() { return incoming_.value(); }

 private:
  fdf_testing::DriverRuntime runtime;
  fdf::UnownedSynchronizedDispatcher background_driver_dispatcher_ =
      runtime.StartBackgroundDispatcher();
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_namespace_{
      background_driver_dispatcher_->async_dispatcher(), std::in_place};
  std::optional<fdf::Namespace> incoming_;

  // Sets the global logger instance which is needed by functions within the `fdf_metadata`
  // namespace in order to make log statements.
  fdf_testing::ScopedGlobalLogger logger_;
};

// Verify that `fdf_metadata::GetMetadata()` can retrieve metadata.
TEST_F(MetadataTest, GetMetadata) {
  const fuchsia_hardware_test::Metadata kMetadata{{.test_property = "test value"}};

  InitIncomingNamespace(true);
  SetIncomingMetadata(kMetadata);

  zx::result metadata = fdf_metadata::GetMetadata<fuchsia_hardware_test::Metadata>(incoming());
  ASSERT_OK(metadata);
  ASSERT_EQ(metadata.value(), kMetadata);
}

// Verify that `fdf_metadata::GetMetadataIfExists()` can retrieve existing metadata.
TEST_F(MetadataTest, GetExistingMetadata) {
  const fuchsia_hardware_test::Metadata kMetadata{{.test_property = "test value"}};

  InitIncomingNamespace(true);
  SetIncomingMetadata(kMetadata);

  zx::result result =
      fdf_metadata::GetMetadataIfExists<fuchsia_hardware_test::Metadata>(incoming());
  ASSERT_OK(result);
  std::optional metadata = result.value();
  ASSERT_TRUE(metadata.has_value());
  ASSERT_EQ(metadata.value(), kMetadata);
}

// Verify that `fdf_metadata::GetMetadataIfExists()` returns nullopt if the incoming metadata server
// does not have metadata to provide.
TEST_F(MetadataTest, GetNonExistentMetadata) {
  InitIncomingNamespace(true);

  zx::result result =
      fdf_metadata::GetMetadataIfExists<fuchsia_hardware_test::Metadata>(incoming());
  ASSERT_OK(result);
  std::optional metadata = result.value();
  ASSERT_FALSE(metadata.has_value());
}

// Verify that `fdf_metadata::GetMetadataIfExists()` returns nullopt if the incoming metadata server
// does not exist.
TEST_F(MetadataTest, GetNonExistentMetadataServer) {
  InitIncomingNamespace(false);

  zx::result result =
      fdf_metadata::GetMetadataIfExists<fuchsia_hardware_test::Metadata>(incoming());
  ASSERT_OK(result);
  std::optional metadata = result.value();
  ASSERT_FALSE(metadata.has_value());
}

// Verify that `fdf_metadata::GetMetadataFromFidlServiceIfExists()` returns an error if unpersisting
// fails.
TEST_F(MetadataTest, GetMetadataUnpersistFails) {
  const fuchsia_hardware_test::Metadata kMetadata{{.test_property = "test value"}};

  InitIncomingNamespace(true);
  SetIncomingMetadata(kMetadata);

  // Attempt to retrieve the metadata as a Dictionary, which will fail to unpersist.
  zx::result result =
      fdf_metadata::GetMetadataFromFidlServiceIfExists<fuchsia_driver_metadata::Dictionary>(
          incoming().svc_dir(), fuchsia_hardware_test::Metadata::kSerializableName);
  ASSERT_TRUE(result.is_error());
}

}  // namespace fdf_metadata::test

#endif
