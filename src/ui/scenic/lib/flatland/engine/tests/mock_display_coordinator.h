// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_TESTS_MOCK_DISPLAY_COORDINATOR_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_TESTS_MOCK_DISPLAY_COORDINATOR_H_

#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/wire_test_base.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <atomic>

#include <gmock/gmock.h>

namespace flatland {

class MockDisplayCoordinator
    : public fidl::testing::WireTestBase<fuchsia_hardware_display::Coordinator> {
 public:
  MockDisplayCoordinator() = default;

  // Must be called from the dispatcher thread.
  void Bind(fidl::ServerEnd<fuchsia_hardware_display::Coordinator> server_end,
            async_dispatcher_t* dispatcher) {
    is_bound_.store(true, std::memory_order_relaxed);
    binding_ = std::make_unique<fidl::ServerBinding<fuchsia_hardware_display::Coordinator>>(
        dispatcher, std::move(server_end), this, [this](fidl::UnbindInfo info) {
          if (!info.is_peer_closed()) {
            FX_LOGS(INFO) << "FIDL binding is closed: " << info.FormatDescription();
          }
          is_bound_.store(false, std::memory_order_relaxed);
        });
  }

  bool IsBound() const { return is_bound_.load(std::memory_order_relaxed); }

  // TODO(https://fxbug.dev/324689624): Do not use gMock to generate mocking
  // methods.

  MOCK_METHOD(void, ImportEvent,
              (fuchsia_hardware_display::wire::CoordinatorImportEventRequest*,
               ImportEventCompleter::Sync&),
              (override));

  MOCK_METHOD(
      void, SetLayerColorConfig,
      (fidl::WireServer<fuchsia_hardware_display::Coordinator>::SetLayerColorConfigRequestView,
       SetLayerColorConfigCompleter::Sync&),
      (override));

  MOCK_METHOD(void, SetLayerImage2,
              (fuchsia_hardware_display::wire::CoordinatorSetLayerImage2Request*,
               SetLayerImage2Completer::Sync&),
              (override));

  MOCK_METHOD(void, CommitConfig,
              (fuchsia_hardware_display::wire::CoordinatorCommitConfigRequest*,
               CommitConfigCompleter::Sync&),
              (override));

  MOCK_METHOD(void, GetLatestCommittedConfigStamp, (GetLatestCommittedConfigStampCompleter::Sync&),
              (override));

  MOCK_METHOD(void, CheckConfig, (CheckConfigCompleter::Sync&), (override));

  MOCK_METHOD(void, DiscardConfig, (DiscardConfigCompleter::Sync&), (override));

  MOCK_METHOD(void, ImportBufferCollection,
              (fuchsia_hardware_display::wire::CoordinatorImportBufferCollectionRequest*,
               ImportBufferCollectionCompleter::Sync&),
              (override));

  MOCK_METHOD(void, SetBufferCollectionConstraints,
              (fuchsia_hardware_display::wire::CoordinatorSetBufferCollectionConstraintsRequest*,
               SetBufferCollectionConstraintsCompleter::Sync&),
              (override));

  MOCK_METHOD(void, ReleaseBufferCollection,
              (fuchsia_hardware_display::wire::CoordinatorReleaseBufferCollectionRequest*,
               ReleaseBufferCollectionCompleter::Sync&),
              (override));

  MOCK_METHOD(void, ImportImage,
              (fuchsia_hardware_display::wire::CoordinatorImportImageRequest*,
               ImportImageCompleter::Sync&),
              (override));

  MOCK_METHOD(void, ReleaseImage,
              (fuchsia_hardware_display::wire::CoordinatorReleaseImageRequest*,
               ReleaseImageCompleter::Sync&),
              (override));

  MOCK_METHOD(void, SetLayerPrimaryConfig,
              (fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryConfigRequest*,
               SetLayerPrimaryConfigCompleter::Sync&),
              (override));

  MOCK_METHOD(void, SetLayerPrimaryPosition,
              (fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryPositionRequest*,
               SetLayerPrimaryPositionCompleter::Sync&),
              (override));

  MOCK_METHOD(void, SetLayerPrimaryAlpha,
              (fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryAlphaRequest*,
               SetLayerPrimaryAlphaCompleter::Sync&),
              (override));

  MOCK_METHOD(void, CreateLayer,
              (CreateLayerRequestView request, CreateLayerCompleter::Sync& completer), (override));

  MOCK_METHOD(void, DestroyLayer,
              (fuchsia_hardware_display::wire::CoordinatorDestroyLayerRequest*,
               DestroyLayerCompleter::Sync&),
              (override));

  MOCK_METHOD(void, SetDisplayMode,
              (fuchsia_hardware_display::wire::CoordinatorSetDisplayModeRequest*,
               SetDisplayModeCompleter::Sync&),
              (override));

  MOCK_METHOD(void, SetDisplayLayers,
              (fuchsia_hardware_display::wire::CoordinatorSetDisplayLayersRequest*,
               SetDisplayLayersCompleter::Sync&),
              (override));

  MOCK_METHOD(void, SetDisplayColorConversion,
              (fuchsia_hardware_display::wire::CoordinatorSetDisplayColorConversionRequest*,
               SetDisplayColorConversionCompleter::Sync&),
              (override));

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    FX_LOGS(ERROR) << "MockDisplayCoordinator::NotImplemented_(): " << name
                   << " is not implemented, closing connection.";

    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  std::unique_ptr<fidl::ServerBinding<fuchsia_hardware_display::Coordinator>> binding_;
  std::atomic<bool> is_bound_ = false;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_TESTS_MOCK_DISPLAY_COORDINATOR_H_
