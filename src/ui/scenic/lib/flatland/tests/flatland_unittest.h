// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_TESTS_FLATLAND_UNITTEST_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_TESTS_FLATLAND_UNITTEST_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/default.h>
#include <lib/async/time.h>
#include <lib/fpromise/bridge.h>
#include <lib/stdcompat/source_location.h>
#include <lib/sync/cpp/completion.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>

#include <map>
#include <memory>
#include <unordered_map>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/scenic/lib/allocation/allocator.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/allocation/mock_buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/flatland_display.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/global_matrix_data.h"
#include "src/ui/scenic/lib/flatland/global_resolved_layers.h"
#include "src/ui/scenic/lib/flatland/global_topology_data.h"
#include "src/ui/scenic/lib/flatland/tests/logging_event_loop.h"
#include "src/ui/scenic/lib/flatland/tests/mock_flatland_presenter.h"
#include "src/ui/scenic/lib/flatland/uber_struct_system.h"
#include "src/ui/scenic/lib/scenic/util/error_reporter.h"
#include "src/ui/scenic/lib/scheduling/id.h"
#include "src/ui/scenic/lib/utils/dispatcher_holder.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/promise.h"
#include "zircon/errors.h"

#include <glm/gtx/matrix_transform_2d.hpp>

namespace flatland {

// Convenience struct for the PresentWithArgs method to avoid having to update it every time
// a new argument is added to Flatland::Present(). This struct also includes additional flags
// to PresentWithArgs itself for testing timing-related Present() functionality.
struct PresentArgs {
  // Arguments to Flatland::Present().
  zx::time requested_presentation_time;
  std::vector<zx::event> acquire_fences;
  std::vector<zx::event> release_fences;
  std::vector<zx::counter> present_fences;
  bool unsquashable = false;

  // Arguments to the PresentWithArgs method.

  // If true, skips the session update associated with the Present(), meaning the new UberStruct
  // will not be in the snapshot and the release fences will not be signaled.
  bool skip_session_update_and_release_fences = false;

  // The number of present tokens that should be returned to the client.
  uint32_t present_credits_returned = 1;

  // The future presentation infos that should be returned to the client.
  flatland::Flatland::FuturePresentationInfos presentation_infos = {};

  // If PresentWithArgs is called with |expect_success| = false, the error that should be
  // expected as the return value from Present().
  fuchsia_ui_composition::FlatlandError expected_error =
      fuchsia_ui_composition::FlatlandError::kBadOperation;
};

struct GlobalIdPair {
  allocation::GlobalBufferCollectionId collection_id;
  allocation::GlobalImageId image_id;
};

// Searches for a local matrix associated with a specific TransformHandle.
//
// |uber_struct| is the UberStruct to search to find the matrix. |target_handle| is the
// TransformHandle of the matrix to compare. |expected_matrix| is the expected value of that
// matrix.
inline void ExpectMatrix(const UberStruct* uber_struct, TransformHandle target_handle,
                         const glm::mat3& expected_matrix,
                         cpp20::source_location location = cpp20::source_location::current()) {
  SCOPED_TRACE(::testing::Message() << location.file_name() << ":" << location.line());
  glm::mat3 matrix = glm::mat3();
  auto matrix_kv = uber_struct->local_matrices.find(target_handle);
  if (matrix_kv != uber_struct->local_matrices.end()) {
    matrix = matrix_kv->second;
  }
  for (size_t i = 0; i < 3; ++i) {
    for (size_t j = 0; j < 3; ++j) {
      EXPECT_FLOAT_EQ(matrix[i][j], expected_matrix[i][j]) << " row " << j << " column " << i;
    }
  }
}

inline void ExpectMatrix(const std::shared_ptr<const UberStruct>& uber_struct,
                         TransformHandle target_handle, const glm::mat3& expected_matrix,
                         cpp20::source_location location = cpp20::source_location::current()) {
  ExpectMatrix(uber_struct.get(), target_handle, expected_matrix, location);
}

inline void ExpectMatrix(const std::shared_ptr<UberStruct>& uber_struct,
                         TransformHandle target_handle, const glm::mat3& expected_matrix,
                         cpp20::source_location location = cpp20::source_location::current()) {
  ExpectMatrix(uber_struct.get(), target_handle, expected_matrix, location);
}

const uint32_t kDefaultSize = 1;
const glm::vec2 kDefaultDisplayPixelRatio = {1.0f, 1.0f};
const int32_t kDefaultInset = 0;

inline fuchsia_ui_composition::wire::ViewBoundProtocols NoViewProtocols() { return {}; }

inline fuchsia_ui_views::wire::ViewIdentityOnCreation NewWireViewIdentityOnCreation() {
  auto view_identity = scenic::cpp::NewViewIdentityOnCreation();
  return fuchsia_ui_views::wire::ViewIdentityOnCreation{
      .view_ref = fuchsia_ui_views::wire::ViewRef{.reference = std::move(
                                                      view_identity.view_ref().reference())},
      .view_ref_control =
          fuchsia_ui_views::wire::ViewRefControl{
              .reference = std::move(view_identity.view_ref_control().reference())},
  };
}

inline fuchsia_ui_composition::wire::BufferCollectionImportToken ToWire(
    fuchsia_ui_composition::BufferCollectionImportToken token) {
  return {.value = std::move(token.value())};
}

inline fuchsia_ui_views::wire::ViewportCreationToken ToWire(
    fuchsia_ui_views::ViewportCreationToken token) {
  return {.value = std::move(token.value())};
}

inline fuchsia_ui_views::wire::ViewCreationToken ToWire(fuchsia_ui_views::ViewCreationToken token) {
  return {.value = std::move(token.value())};
}

class EventHandler : public fidl::AsyncEventHandler<fuchsia_ui_composition::Flatland> {
 public:
  EventHandler(scheduling::SessionId session_id,
               std::unordered_map<scheduling::SessionId, fuchsia_ui_composition::FlatlandError>&
                   flatland_errors)
      : session_id_(session_id), flatland_errors_(flatland_errors) {}

  // Handler for |OnError| events sent from the server.
  void OnError(fidl::Event<fuchsia_ui_composition::Flatland::OnError>& event) override {
    flatland_errors_[session_id_] = event.error();
  }

  // Notification that server end of channel was closed.
  void on_fidl_error(fidl::UnbindInfo unbind_info) override {
    FX_LOGS(INFO) << "Flatland test EventHandler unbound: " << unbind_info;
  }

 private:
  scheduling::SessionId session_id_;
  std::unordered_map<scheduling::SessionId, fuchsia_ui_composition::FlatlandError>&
      flatland_errors_;
};

class TestErrorReporter : public scenic_impl::ErrorReporter {
 public:
  explicit TestErrorReporter(std::optional<std::string>& last_error_log)
      : reported_error_(last_error_log) {}

 private:
  // |scenic_impl::ErrorReporter|
  void ReportError(fuchsia_logging::LogSeverity severity, std::string error_string) override {
    reported_error_ = error_string;
  }

  std::optional<std::string>& reported_error_;
};

class FlatlandTest : public LoggingEventLoop, public ::testing::Test {
 public:
  FlatlandTest()
      : uber_struct_system_(std::make_shared<UberStructSystem>()),
        link_system_(std::make_shared<LinkSystem>(uber_struct_system_->GetNextInstanceId())) {}

  void SetUp() override {
    mock_flatland_presenter_ = new ::testing::StrictMock<MockFlatlandPresenter>();

    ON_CALL(*mock_flatland_presenter_,
            ScheduleUpdateForSession(::testing::_, ::testing::_, ::testing::_, ::testing::_,
                                     ::testing::_, ::testing::_, ::testing::_))
        .WillByDefault(::testing::Invoke(
            [&](zx::time requested_presentation_time, scheduling::SchedulingIdPair id_pair,
                bool unsquashable, std::vector<zx::event> release_fences,
                std::vector<zx::counter> release_counters, std::vector<zx::counter> present_fences,
                bool schedule_asap) {
              // The ID must not already be registered.
              EXPECT_FALSE(pending_release_fences_.contains(id_pair));
              pending_release_fences_[id_pair] = std::move(release_fences);
              pending_release_counters_[id_pair] = std::move(release_counters);

              // Ensure IDs are strictly increasing.
              auto current_id_kv = pending_instance_updates_.find(id_pair.session_id);
              EXPECT_TRUE(current_id_kv == pending_instance_updates_.end() ||
                          current_id_kv->second < id_pair.present_id);

              // Only save the latest PresentId: the UberStructSystem will flush all Presents prior
              // to it.
              pending_instance_updates_[id_pair.session_id] = id_pair.present_id;

              // Store all requested presentation times to verify in test.
              requested_presentation_times_[id_pair] = requested_presentation_time;
            }));

    ON_CALL(*mock_flatland_presenter_, RemoveSession(::testing::_, ::testing::_))
        .WillByDefault(::testing::Invoke(
            [](scheduling::SessionId session_id, std::optional<zx::event> release_fence) {
              if (release_fence) {
                // Pretend that another frame was rendered, causing the release fence to be
                // signaled.
                release_fence.value().signal(0, ZX_EVENT_SIGNALED);
              }
            }));

    sysmem_allocator_ = utils::CreateSysmemAllocatorClient(dispatcher(), "FlatlandTest::SetUp");

    flatland_presenter_ = std::shared_ptr<FlatlandPresenter>(mock_flatland_presenter_);

    mock_buffer_collection_importer_ = new allocation::MockBufferCollectionImporter();
    buffer_collection_importer_ =
        std::shared_ptr<allocation::BufferCollectionImporter>(mock_buffer_collection_importer_);

    // Capture uninteresting cleanup calls from Allocator dtor.
    EXPECT_CALL(*mock_buffer_collection_importer_,
                ReleaseBufferCollection(::testing::_, ::testing::_))
        .Times(::testing::AtLeast(0));

    // ~Flatland() ensures that RemoveSession will always be called; this is uninteresting.
    EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(::testing::_, ::testing::_))
        .Times(::testing::AtLeast(0));
  }

  void TearDown() override {
    RunLoopUntilIdle();

    auto link_topologies = link_system_->GetResolvedTopologyLinks();
    EXPECT_TRUE(link_topologies.empty());

    buffer_collection_importer_.reset();
    flatland_presenter_.reset();
    flatlands_.clear();
    flatland_displays_.clear();
  }

  std::shared_ptr<allocation::Allocator> CreateAllocator() {
    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> importers;
    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> screenshot_importers;
    importers.push_back(buffer_collection_importer_);
    return std::make_shared<allocation::Allocator>(
        dispatcher(), context_provider_.context(), importers, screenshot_importers,
        utils::CreateSysmemAllocatorClient(dispatcher(), "FlatlandTest::CreateAllocator"));
  }

  virtual std::shared_ptr<Flatland> CreateFlatland(
      const FlatlandConfig& config = FlatlandConfig{}) {
    auto session_id = scheduling::GetNextSessionId();
    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> importers;
    importers.push_back(buffer_collection_importer_);

    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_composition::Flatland>::Create();

    std::shared_ptr<Flatland> flatland = Flatland::New(
        std::make_shared<utils::UnownedDispatcherHolder>(dispatcher()), std::move(server_end),
        session_id,
        /*destroy_instance_functon=*/[this, session_id]() { flatland_errors_.erase(session_id); },
        flatland_presenter_, link_system_, uber_struct_system_->AllocateQueueForSession(session_id),
        importers, [](auto...) {}, [](auto...) {}, [](auto...) {}, [](auto...) {}, [](auto...) {},
        [](auto...) {}, std::move(config));

    // Wait for server channel to be bound; see `Flatland::Bind()`.
    RunLoopUntilIdle();

    auto event_handler = std::make_unique<EventHandler>(session_id, flatland_errors_);
    fidl::Client client(std::move(client_end), dispatcher(), event_handler.get());
    flatlands_.push_back({std::move(client), std::move(event_handler)});
    return flatland;
  }

  // Utility for setting up a client and a server on their own async loops, connected via a FIDL
  // channel.
  class FlatlandEventLoopClientServer {
   public:
    using ClientType = fidl::Client<fuchsia_ui_composition::Flatland>;

    FlatlandEventLoopClientServer(std::shared_ptr<FlatlandPresenter> presenter,
                                  std::shared_ptr<LinkSystem> link_system,
                                  std::shared_ptr<UberStructSystem> uber_struct_system)
        : server_loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
          client_loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
      server_loop_.StartThread("flatland-server-loop");
      client_loop_.StartThread("flatland-client-loop");

      auto session_id = scheduling::GetNextSessionId();
      auto flatland_endpoints = fidl::Endpoints<fuchsia_ui_composition::Flatland>::Create();

      server_ = Flatland::New(
          std::make_shared<utils::UnownedDispatcherHolder>(server_loop_.dispatcher()),
          std::move(flatland_endpoints.server), session_id,
          /*destroy_instance_function=*/[]() {}, std::move(presenter), std::move(link_system),
          uber_struct_system->AllocateQueueForSession(session_id),
          /*buffer_collection_importers=*/{}, [](auto...) {}, [](auto...) {}, [](auto...) {},
          [](auto...) {}, [](auto...) {}, [](auto...) {}, FlatlandConfig{});

      libsync::Completion completion;
      async::PostTask(client_loop_.dispatcher(), [&, dispatcher = client_loop_.dispatcher()]() {
        client_ = std::make_shared<fidl::Client<fuchsia_ui_composition::Flatland>>(
            std::move(flatland_endpoints.client), dispatcher);
        completion.Signal();
      });
      completion.Wait();
    }

    ~FlatlandEventLoopClientServer() {
      DestroyServer();
      DestroyClient();
    }

    async::Loop& server_loop() { return server_loop_; }
    async::Loop& client_loop() { return client_loop_; }
    const std::shared_ptr<Flatland>& server() const { return server_; }
    const std::shared_ptr<ClientType>& client() const { return client_; }

   private:
    // Destroy the server on its own event loop.
    void DestroyServer() {
      ASSERT_TRUE(server_);
      libsync::Completion completion;
      async::PostTask(server_loop_.dispatcher(),
                      [this, &completion, server = std::move(server_)]() mutable {
                        server.reset();
                        server_loop_.Quit();
                        completion.Signal();
                      });
      ASSERT_FALSE(server_);
      completion.Wait();
    }

    // Destroy the client on its own event loop.
    void DestroyClient() {
      ASSERT_TRUE(client_);
      libsync::Completion completion;
      async::PostTask(client_loop_.dispatcher(),
                      [this, &completion, client = std::move(client_)]() mutable {
                        client.reset();
                        client_loop_.Quit();
                        completion.Signal();
                      });
      ASSERT_FALSE(client_);
      completion.Wait();
    }

    async::Loop server_loop_;
    async::Loop client_loop_;
    std::shared_ptr<Flatland> server_;
    std::shared_ptr<ClientType> client_;
  };

  std::unique_ptr<FlatlandEventLoopClientServer> CreateFlatlandEventLoopClientServer() {
    return std::make_unique<FlatlandEventLoopClientServer>(flatland_presenter_, link_system_,
                                                           uber_struct_system_);
  }

  std::shared_ptr<FlatlandDisplay> CreateFlatlandDisplay(uint32_t width_in_px,
                                                         uint32_t height_in_px) {
    static constexpr uint32_t kMaxDisplayLayersCount = 2;
    auto session_id = scheduling::GetNextSessionId();
    auto display = std::make_shared<display::Display>(
        display::WireDisplayId{.value = 1}, width_in_px, height_in_px, kMaxDisplayLayersCount);
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_ui_composition::FlatlandDisplay>::Create();
    flatland_displays_.emplace_back(std::move(client_end), dispatcher());
    return FlatlandDisplay::New(
        std::make_shared<utils::UnownedDispatcherHolder>(dispatcher()), std::move(server_end),
        session_id, std::move(display),
        /*destroy_display_function*/ []() {}, flatland_presenter_, link_system_,
        uber_struct_system_->AllocateQueueForSession(session_id));
  }

  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> CreateToken() {
    auto [token_client, token_server] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator_->AllocateSharedCollection(
        fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
            .token_request(std::move(token_server))
            .Build());
    EXPECT_TRUE(result.ok());
    fidl::SyncClient sync_token(std::move(token_client));
    auto sync_result = sync_token->Sync();
    EXPECT_TRUE(sync_result.is_ok());
    return sync_token.TakeClientEnd();
  }

  // Applies the most recently scheduled session update for each session and signals the release
  // fences of all Presents up to and including that update.
  void ApplySessionUpdatesAndSignalFences() {
    uber_struct_system_->UpdateInstances(pending_instance_updates_);

    // Signal all release fences up to and including the PresentId in |pending_instance_updates_|.
    for (const auto& [session_id, present_id] : pending_instance_updates_) {
      auto begin = pending_release_fences_.lower_bound({session_id, 0});
      auto end = pending_release_fences_.upper_bound({session_id, present_id});
      for (auto fences_kv = begin; fences_kv != end; ++fences_kv) {
        for (auto& event : fences_kv->second) {
          event.signal(0, ZX_EVENT_SIGNALED);
        }
      }
      pending_release_fences_.erase(begin, end);

      auto begin_counters = pending_release_counters_.lower_bound({session_id, 0});
      auto end_counters = pending_release_counters_.upper_bound({session_id, present_id});
      for (auto counters_kv = begin_counters; counters_kv != end_counters; ++counters_kv) {
        for (auto& counter : counters_kv->second) {
          counter.signal(0, ZX_COUNTER_SIGNALED);
        }
      }
      pending_release_counters_.erase(begin_counters, end_counters);
    }

    pending_instance_updates_.clear();
    requested_presentation_times_.clear();
  }

  // Gets the list of registered PresentIds for a particular |session_id|.
  std::vector<scheduling::PresentId> GetRegisteredPresents(scheduling::SessionId session_id) const {
    std::vector<scheduling::PresentId> present_ids;

    auto begin = pending_release_fences_.lower_bound({session_id, 0});
    auto end = pending_release_fences_.upper_bound({session_id + 1, 0});
    for (auto fence_kv = begin; fence_kv != end; ++fence_kv) {
      present_ids.push_back(fence_kv->first.present_id);
    }

    return present_ids;
  }

  // Returns true if |session_id| currently has a session update pending.
  bool HasSessionUpdate(scheduling::SessionId session_id) const {
    return pending_instance_updates_.contains(session_id);
  }

  // Returns the requested presentation time for a particular |id_pair|, or std::nullopt if that
  // pair has not had a presentation scheduled for it.
  std::optional<zx::time> GetRequestedPresentationTime(scheduling::SchedulingIdPair id_pair) {
    auto iter = requested_presentation_times_.find(id_pair);
    if (iter == requested_presentation_times_.end()) {
      return std::nullopt;
    }
    return iter->second;
  }

  // The parent transform must be a topology root or ComputeGlobalTopologyData() will crash.
  bool IsDescendantOf(TransformHandle parent, TransformHandle child) {
    auto snapshot = uber_struct_system_->Snapshot();
    auto links = link_system_->GetResolvedTopologyLinks();
    auto data = GlobalTopologyData::ComputeGlobalTopologyData(
        snapshot.map, links, link_system_->GetInstanceId(), parent);
    for (auto handle : data.topology_vector) {
      if (handle == child) {
        return true;
      }
    }
    return false;
  }

  // Snapshots the UberStructSystem and fetches the UberStruct associated with |flatland|. If no
  // UberStruct exists for |flatland|, returns nullptr.
  std::shared_ptr<const UberStruct> GetUberStruct(Flatland* flatland) {
    auto snapshot = uber_struct_system_->Snapshot();

    auto root = flatland->GetRoot();
    auto uber_struct_kv = snapshot.map.find(root.GetInstanceId());
    if (uber_struct_kv == snapshot.map.end()) {
      return nullptr;
    }

    auto uber_struct = uber_struct_kv->second;
    EXPECT_FALSE(uber_struct->local_topology.empty());
    EXPECT_EQ(uber_struct->local_topology[0].handle, root);

    return uber_struct;
  }

  // Updates all Links reachable from |root_transform|, which must be the root transform of one of
  // the active Flatland instances.
  //
  // Tests that call this function are testing both Flatland and LinkSystem::UpdateLinkWatchers().
  void UpdateLinks(TransformHandle root_transform) {
    // Run the looper in case there are queued commands in, e.g., ObjectLinker.
    RunLoopUntilIdle();

    // This is a replica of the core render loop.
    const auto snapshot = uber_struct_system_->Snapshot();
    const auto links = link_system_->GetResolvedTopologyLinks();
    const auto data = GlobalTopologyData::ComputeGlobalTopologyData(
        snapshot.map, links, link_system_->GetInstanceId(), root_transform);
    const auto matrices =
        flatland::ComputeGlobalMatrices(data.topology_vector, data.parent_indices, snapshot.map);

    link_system_->UpdateLinkWatchers(data.topology_vector, matrices, snapshot.map);
    link_system_->UpdateDevicePixelRatio(display_pixel_ratio_);

    // Run the looper again to process any queued FIDL events (i.e., Link callbacks).
    RunLoopUntilIdle();
  }

  // Calls Present() on a Flatland object and immediately triggers the session update
  // for all sessions so that changes from that Present() are visible in global systems. This is
  // primarily useful for testing the user-facing Flatland API.
  //
  // |flatland| is a Flatland object constructed with the MockFlatlandPresenter owned by the
  // FlatlandTest harness. |expect_success| should be false if the call to Present() is expected to
  // trigger an error.
  void PresentWithArgs(Flatland* flatland, PresentArgs args, bool expect_success,
                       cpp20::source_location location = cpp20::source_location::current()) {
    SCOPED_TRACE(::testing::Message() << location.file_name() << ":" << location.line());
    bool had_acquire_fences = !args.acquire_fences.empty();
    fidl::Arena arena;
    auto builder = fuchsia_ui_composition::wire::PresentArgs::Builder(arena);
    builder.requested_presentation_time(args.requested_presentation_time.get())
        .unsquashable(args.unsquashable);
    if (!args.acquire_fences.empty()) {
      builder.acquire_fences(fidl::VectorView<zx::event>::FromExternal(args.acquire_fences));
    }
    if (!args.release_fences.empty()) {
      builder.release_fences(fidl::VectorView<zx::event>::FromExternal(args.release_fences));
    }
    if (!args.present_fences.empty()) {
      builder.present_fences(fidl::VectorView<zx::counter>::FromExternal(args.present_fences));
    }
    auto present_args = builder.Build();
    flatland->Present(present_args);
    if (expect_success) {
      // Even with no acquire_fences, UberStruct updates queue on the dispatcher.
      if (!had_acquire_fences) {
        EXPECT_CALL(*mock_flatland_presenter_,
                    ScheduleUpdateForSession(args.requested_presentation_time, ::testing::_,
                                             args.unsquashable, ::testing::_, ::testing::_,
                                             ::testing::_, ::testing::_));
      }
      RunLoopUntilIdle();
      if (!args.skip_session_update_and_release_fences) {
        ApplySessionUpdatesAndSignalFences();
      }
      flatland->OnNextFrameBegin(args.present_credits_returned, std::move(args.presentation_infos));
    } else {
      RunLoopUntilIdle();
      EXPECT_EQ(GetFlatlandError(flatland->GetSessionId()), args.expected_error);
    }
  }

  void PresentWithArgs(const std::shared_ptr<Flatland>& flatland, PresentArgs args,
                       bool expect_success,
                       cpp20::source_location location = cpp20::source_location::current()) {
    PresentWithArgs(flatland.get(), std::move(args), expect_success, location);
  }

  void Present(Flatland* flatland, bool expect_success,
               cpp20::source_location location = cpp20::source_location::current()) {
    PresentWithArgs(flatland, PresentArgs(), expect_success, location);
  }

  void Present(const std::shared_ptr<Flatland>& flatland, bool expect_success,
               cpp20::source_location location = cpp20::source_location::current()) {
    Present(flatland.get(), expect_success, location);
  }

  void CreateViewport(
      Flatland* parent, Flatland* child, ContentId viewport_id,
      fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> child_view_watcher,
      fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher> parent_viewport_watcher) {
    fuchsia_ui_views::wire::ViewportCreationToken parent_token;
    fuchsia_ui_views::wire::ViewCreationToken child_token;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value, &child_token.value));

    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                          .Build();

    parent->CreateViewport(viewport_id, std::move(parent_token), properties,
                           std::move(child_view_watcher));

    child->CreateView2(std::move(child_token), NewWireViewIdentityOnCreation(),
                       fuchsia_ui_composition::wire::ViewBoundProtocols(),
                       std::move(parent_viewport_watcher));

    Present(parent, true);
    Present(child, true);

    // After View creation the child should have an associated ViewRef.
    auto child_uber_struct = GetUberStruct(child);
    ASSERT_NE(child_uber_struct, nullptr);
    EXPECT_NE(child_uber_struct->view_ref, nullptr);
  }

  void SetDisplayContent(
      FlatlandDisplay* display, Flatland* child,
      fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> child_view_watcher_server_end,
      fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher>
          parent_viewport_watcher_server_end) {
    FX_CHECK(display);
    FX_CHECK(child);
    FX_CHECK(child_view_watcher_server_end);
    FX_CHECK(parent_viewport_watcher_server_end);
    fuchsia_ui_views::ViewportCreationToken parent_token;
    fuchsia_ui_views::wire::ViewCreationToken child_token;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value));
    auto present_id = scheduling::PeekNextPresentId();
    EXPECT_CALL(*mock_flatland_presenter_,
                ScheduleUpdateForSession(
                    zx::time(0), scheduling::SchedulingIdPair{display->session_id(), present_id},
                    true, ::testing::_, ::testing::_, ::testing::_, ::testing::_));
    display->SetContent(std::move(parent_token), std::move(child_view_watcher_server_end));

    child->CreateView2(std::move(child_token), NewWireViewIdentityOnCreation(), NoViewProtocols(),
                       std::move(parent_viewport_watcher_server_end));
  }

  void RegisterBufferCollection(
      allocation::Allocator* allocator,
      fuchsia_ui_composition::BufferCollectionExportToken bc_export_token,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, bool expect_success,
      cpp20::source_location location = cpp20::source_location::current()) {
    SCOPED_TRACE(::testing::Message() << location.file_name() << ":" << location.line());
    if (expect_success) {
      EXPECT_CALL(*mock_buffer_collection_importer_,
                  ImportBufferCollection(fsl::GetKoid(bc_export_token.value().get()), ::testing::_,
                                         ::testing::_, ::testing::_, ::testing::_))
          .WillOnce(integration_tests::ReturnPromise(fpromise::ok()));
    }
    bool processed_callback = false;
    fuchsia_ui_composition::RegisterBufferCollectionArgs args;
    args.export_token(std::move(bc_export_token));
    args.buffer_collection_token2(std::move(token));
    allocator->RegisterBufferCollection(std::move(args),
                                        [&processed_callback, expect_success](auto result) {
                                          EXPECT_EQ(expect_success, result.is_ok());
                                          processed_callback = true;
                                        });
    RunLoopUntil([&processed_callback] { return processed_callback; });
    EXPECT_TRUE(processed_callback);
  }

  void RegisterBufferCollection(
      const std::shared_ptr<allocation::Allocator>& allocator,
      fuchsia_ui_composition::BufferCollectionExportToken bc_export_token,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, bool expect_success,
      cpp20::source_location location = cpp20::source_location::current()) {
    RegisterBufferCollection(allocator.get(), std::move(bc_export_token), std::move(token),
                             expect_success, location);
  }

  // Helper function to create an image, registering it with sysmem and flatland, and presenting.
  GlobalIdPair CreateImage(
      Flatland* flatland, allocation::Allocator* allocator, ContentId image_id,
      allocation::cpp::BufferCollectionImportExportTokens buffer_collection_import_export_tokens,
      fuchsia_ui_composition::ImageProperties properties) {
    const auto koid =
        fsl::GetKoid(buffer_collection_import_export_tokens.export_token.value().get());
    RegisterBufferCollection(allocator,
                             std::move(buffer_collection_import_export_tokens.export_token),
                             CreateToken(), true);

    FX_DCHECK(properties.size().has_value());
    FX_DCHECK(properties.size()->width());
    FX_DCHECK(properties.size()->height());

    allocation::GlobalImageId global_image_id;
    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(::testing::_, ::testing::_))
        .WillOnce(
            ::testing::Invoke([&global_image_id](const allocation::ImageMetadata& metadata,
                                                 allocation::BufferCollectionUsage usage_type) {
              global_image_id = metadata.identifier;
              return fpromise::make_ok_promise();
            }));

    fidl::Arena arena;
    auto wire_properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                               .size(fuchsia_math::wire::SizeU{properties.size()->width(),
                                                               properties.size()->height()})
                               .Build();

    flatland->CreateImage(
        image_id,
        fuchsia_ui_composition::wire::BufferCollectionImportToken{
            .value = std::move(buffer_collection_import_export_tokens.import_token.value())},
        0, wire_properties);
    Present(flatland, true);
    return {.collection_id = koid, .image_id = global_image_id};
  }

  // Checks the output of GetFlatlandError() for a particular session.
  fuchsia_ui_composition::FlatlandError GetFlatlandError(scheduling::SessionId session_id) {
    auto error_kv = flatland_errors_.find(session_id);
    if (error_kv == flatland_errors_.end()) {
      return static_cast<fuchsia_ui_composition::FlatlandError>(0);
    }
    return error_kv->second;
  }

 protected:
  sys::testing::ComponentContextProvider context_provider_;

  std::shared_ptr<UberStructSystem> uber_struct_system_;
  std::shared_ptr<LinkSystem> link_system_;

  ::testing::StrictMock<MockFlatlandPresenter>* mock_flatland_presenter_;
  std::shared_ptr<FlatlandPresenter> flatland_presenter_;

  allocation::MockBufferCollectionImporter* mock_buffer_collection_importer_;
  std::shared_ptr<allocation::BufferCollectionImporter> buffer_collection_importer_;

  std::vector<
      std::pair<fidl::Client<fuchsia_ui_composition::Flatland>, std::unique_ptr<EventHandler>>>
      flatlands_;
  std::vector<fidl::Client<fuchsia_ui_composition::FlatlandDisplay>> flatland_displays_;
  std::unordered_map<scheduling::SessionId, fuchsia_ui_composition::FlatlandError> flatland_errors_;
  glm::vec2 display_pixel_ratio_ = kDefaultDisplayPixelRatio;

  // Storage for |mock_flatland_presenter_|.
  std::map<scheduling::SchedulingIdPair, std::vector<zx::event>> pending_release_fences_;
  std::map<scheduling::SchedulingIdPair, std::vector<zx::counter>> pending_release_counters_;
  std::map<scheduling::SchedulingIdPair, zx::time> requested_presentation_times_;
  std::unordered_map<scheduling::SessionId, scheduling::PresentId> pending_instance_updates_;
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
};

class Flatland2Test : public FlatlandTest {
 public:
  std::shared_ptr<Flatland> CreateFlatland(
      const FlatlandConfig& config = FlatlandConfig{}) override {
    FX_CHECK(false)
        << "illegal to call CreateFlatland() in Flatland2Test; use CreateFlatland2() instead";
    return nullptr;
  }

  std::shared_ptr<Flatland> CreateFlatland2(std::optional<std::string>* error_log = nullptr) {
    FlatlandConfig config{.use_flatland2 = true};
    auto flatland = FlatlandTest::CreateFlatland(config);
    if (error_log) {
      flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(*error_log));
    }
    return flatland;
  }

  std::vector<ResolvedLayer> ComputeResolvedLayers(Flatland* flatland) {
    auto snapshot = uber_struct_system_->Snapshot();
    auto links = link_system_->GetResolvedTopologyLinks();
    auto root_transform = flatland->GetRoot();
    auto topology_data = GlobalTopologyData::ComputeGlobalTopologyData(
        snapshot.map, links, link_system_->GetInstanceId(), root_transform);

    GlobalMatrixVector global_matrices;
    ComputeGlobalMatrices(global_matrices, topology_data.topology_vector,
                          topology_data.parent_indices, snapshot.map);

    GlobalTransformClipRegionVector clip_regions;
    ComputeGlobalTransformClipRegions(clip_regions, topology_data.topology_vector,
                                      topology_data.parent_indices, global_matrices, snapshot.map);

    GlobalOpacityVector inherited_opacities;
    ComputeGlobalOpacityValues(inherited_opacities, topology_data.topology_vector,
                               topology_data.parent_indices, snapshot.map);

    return flatland::ComputeGlobalResolvedLayers(topology_data, snapshot.map, global_matrices,
                                                 clip_regions, inherited_opacities);
  }

  std::vector<ResolvedLayer> GetRenderables(Flatland* flatland, uint64_t display_width = 1000,
                                            uint64_t display_height = 1000) {
    auto resolved_layers = ComputeResolvedLayers(flatland);
    CullLayersInPlace(&resolved_layers, display_width, display_height);
    return resolved_layers;
  }
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_TESTS_FLATLAND_UNITTEST_H_
