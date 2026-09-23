// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/flatland_manager.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/fit/function.h>
#include <lib/scheduler/role.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <utility>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/dispatcher_holder.h"
#include "src/ui/scenic/lib/utils/task_utils.h"

namespace flatland {

FlatlandManager::FlatlandManager(
    async_dispatcher_t* dispatcher, const std::shared_ptr<FlatlandPresenter>& flatland_presenter,
    const std::shared_ptr<UberStructSystem>& uber_struct_system,
    const std::shared_ptr<LinkSystem>& link_system, std::shared_ptr<display::Display> display,
    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> buffer_collection_importers,
    std::function<void(fidl::ServerEnd<fuchsia_ui_views::Focuser>, zx_koid_t)>
        register_view_focuser,
    std::function<void(fidl::ServerEnd<fuchsia_ui_views::ViewRefFocused>, zx_koid_t)>
        register_view_ref_focused,
    std::function<void(fidl::ServerEnd<fuchsia_ui_pointer::TouchSource>, zx_koid_t)>
        register_touch_source,
    std::function<void(fidl::ServerEnd<fuchsia_ui_pointer::MouseSource>, zx_koid_t)>
        register_mouse_source,
    std::function<void(fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2>, zx_koid_t)>
        register_touch_source_v2,
    std::function<void(fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2>, zx_koid_t)>
        register_mouse_source_v2)
    : flatland_presenter_(flatland_presenter),
      uber_struct_system_(uber_struct_system),
      link_system_(link_system),
      buffer_collection_importers_(std::move(buffer_collection_importers)),
      primary_display_(std::move(display)),
      register_view_focuser_(std::move(register_view_focuser)),
      register_view_ref_focused_(std::move(register_view_ref_focused)),
      register_touch_source_(std::move(register_touch_source)),
      register_mouse_source_(std::move(register_mouse_source)),
      register_touch_source_v2_(std::move(register_touch_source_v2)),
      register_mouse_source_v2_(std::move(register_mouse_source_v2)),
      cleanup_loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
      executor_(dispatcher) {
  FX_DCHECK(dispatcher);
  FX_DCHECK(flatland_presenter_);
  FX_DCHECK(uber_struct_system_);
  FX_DCHECK(link_system_);
  FX_DCHECK(register_view_focuser_);
  FX_DCHECK(register_view_ref_focused_);
  FX_DCHECK(register_touch_source_);
  FX_DCHECK(register_mouse_source_);
  FX_DCHECK(register_touch_source_v2_);
  FX_DCHECK(register_mouse_source_v2_);
#ifndef NDEBUG
  for (auto& buffer_collection_importer : buffer_collection_importers_) {
    FX_DCHECK(buffer_collection_importer);
  }
#endif

  cleanup_loop_.StartThread("FlatlandManager Cleanup");
}

FlatlandManager::~FlatlandManager() {
  // Clean up externally managed resources.
  for (auto it = flatland_instances_.begin(); it != flatland_instances_.end();) {
    // Use post-increment because otherwise the iterator would be invalidated when the entry is
    // erased within RemoveFlatlandInstance().
    RemoveFlatlandInstance(it++->first);
  }

  // Destroy the flatland manager only after all flatland instances have been destroyed on their
  // worker threads.
  while (alive_sessions_ > 0) {
    std::this_thread::yield();
  }
}

std::optional<scheduling::SessionId> FlatlandManager::CreateFlatland(
    fidl::ServerEnd<fuchsia_ui_composition::Flatland> request, const FlatlandConfig& config) {
  utils::CheckIsOnMainThread();

  if (config.use_trusted_flatland_api) {
    return CreateTrustedFlatland(std::move(request), config);
  } else {
    return CreateUntrustedFlatland(std::move(request), config);
  }
}

std::optional<scheduling::SessionId> FlatlandManager::CreateTrustedFlatland(
    fidl::ServerEnd<fuchsia_ui_composition::Flatland> request, const FlatlandConfig& config) {
  const scheduling::SessionId id = uber_struct_system_->GetNextInstanceId();
  FX_DCHECK(flatland_instances_.find(id) == flatland_instances_.end());
  FX_DCHECK(flatland_display_instances_.find(id) == flatland_display_instances_.end());

  auto result = flatland_instances_.emplace(id, std::make_unique<FlatlandInstance>());
  FX_DCHECK(result.second);
  auto& instance = result.first->second;

  instance->loop = std::make_shared<utils::UnownedDispatcherHolder>(executor_.dispatcher());

  if (!config.skips_present_credits) {
    all_clients_opt_out_present_info_ = false;
  }

  instance->impl = NewFlatland(
      instance->loop, std::move(request), id, [this, id] { DestroyInstanceFunction(id); },
      flatland_presenter_, link_system_, uber_struct_system_->AllocateQueueForSession(id),
      buffer_collection_importers_, config);

  alive_sessions_++;
  return id;
}

std::optional<scheduling::SessionId> FlatlandManager::CreateUntrustedFlatland(
    fidl::ServerEnd<fuchsia_ui_composition::Flatland> request, const FlatlandConfig& config) {
  const scheduling::SessionId id = uber_struct_system_->GetNextInstanceId();
  FX_DCHECK(flatland_instances_.find(id) == flatland_instances_.end());
  FX_DCHECK(flatland_display_instances_.find(id) == flatland_display_instances_.end());

  zx_koid_t endpoint_id;
  zx_koid_t peer_endpoint_id;
  std::tie(endpoint_id, peer_endpoint_id) = fsl::GetKoids(request.channel().get());

  const std::string name =
      "Flatland ID=" + std::to_string(id) + " PEER=" + std::to_string(peer_endpoint_id);

  // Allocate the worker Loop first so that the Flatland impl can be bound to its dispatcher.
  auto loop_holder = std::make_shared<utils::LoopDispatcherHolder>(
      &kAsyncLoopConfigNoAttachToCurrentThread,
      [this](std::unique_ptr<async::Loop> loop_to_destroy) {
        FX_DCHECK(loop_to_destroy->dispatcher() != this->cleanup_loop_.dispatcher());

        async::PostTask(this->cleanup_loop_.dispatcher(),
                        [loop_to_destroy = std::move(loop_to_destroy), this]() mutable {
                          loop_to_destroy.reset();  // Explicitly destroy the loop.
                          // Allow FlatlandManager's dtor to continue destroying cleanup_loop_.
                          this->alive_sessions_--;
                        });
      });

  auto result = flatland_instances_.emplace(id, std::make_unique<FlatlandInstance>());
  FX_DCHECK(result.second);
  auto& instance = result.first->second;
  instance->loop = loop_holder;

  async::PostTask(instance->loop->dispatcher(), []() {
    zx_status_t status = fuchsia_scheduler::SetRoleForThisThread("fuchsia.graphics.flatland");
    if (status != ZX_OK) {
      // "fuchsia.graphics.flatland" isn't defined for all products. This is a no-op on those
      // products and a failure is WAI.
      FX_LOGS(INFO) << "Failed to apply profile to flatland thread: " << status;
    }
  });

  // TODO(https://fxbug.dev/491886218): Address the edge case where the only Flatland connection(s)
  // that require present credits, get disconnected but we continue to compute
  // FuturePresentationInfo for no reason.
  if (!config.skips_present_credits) {
    all_clients_opt_out_present_info_ = false;
  }

  instance->impl = NewFlatland(
      instance->loop, std::move(request), id, [this, id] { DestroyInstanceFunction(id); },
      flatland_presenter_, link_system_, uber_struct_system_->AllocateQueueForSession(id),
      buffer_collection_importers_, config);

  zx_status_t status = loop_holder->loop().StartThread(name.c_str());
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to start thread for Flatland session: " << status;
    flatland_instances_.erase(id);
    return std::nullopt;
  }

  alive_sessions_++;
  return id;
}

std::shared_ptr<Flatland> FlatlandManager::NewFlatland(
    std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
    fidl::ServerEnd<fuchsia_ui_composition::Flatland> request, scheduling::SessionId session_id,
    std::function<void()> destroy_instance_function,
    std::shared_ptr<FlatlandPresenter> flatland_presenter, std::shared_ptr<LinkSystem> link_system,
    std::shared_ptr<UberStructSystem::UberStructQueue> uber_struct_queue,
    const std::vector<std::shared_ptr<allocation::BufferCollectionImporter>>&
        buffer_collection_importers,
    const FlatlandConfig& config) const {
  return Flatland::New(std::move(dispatcher_holder), std::move(request), session_id,
                       std::move(destroy_instance_function), std::move(flatland_presenter),
                       std::move(link_system), std::move(uber_struct_queue),
                       std::move(buffer_collection_importers),
                       /*register_view_focuser*/ register_view_focuser_,
                       /*register_view_ref_focused*/ register_view_ref_focused_,
                       /*register_touch_source*/ register_touch_source_,
                       /*register_mouse_source*/ register_mouse_source_,
                       /*register_touch_source_v2*/ register_touch_source_v2_,
                       /*register_mouse_source_v2*/ register_mouse_source_v2_, config);
}

void FlatlandManager::CreateFlatlandDisplay(
    fidl::ServerEnd<fuchsia_ui_composition::FlatlandDisplay> request) {
  const scheduling::SessionId id = uber_struct_system_->GetNextInstanceId();
  FX_DCHECK(flatland_instances_.find(id) == flatland_instances_.end());
  FX_DCHECK(flatland_display_instances_.find(id) == flatland_display_instances_.end());

  // TODO(https://fxbug.dev/42156949): someday there will be a DisplayToken or something for the
  // client to identify which hardware display this FlatlandDisplay is associated with.  For now:
  // hard-coded.
  auto hw_display = primary_display_;

  if (hw_display->is_claimed()) {
    // TODO(https://fxbug.dev/42156567): error reporting direct to client somehow?
    FX_LOGS(ERROR) << "Display id=" << hw_display->display_id().value()
                   << " is already claimed, cannot instantiate FlatlandDisplay.";
    return;
  }
  hw_display->Claim();

  // Allocate the worker Loop first so that the impl can be bound to its dispatcher.
  auto [new_instance_iterator, inserted] =
      flatland_display_instances_.emplace(id, std::make_unique<FlatlandDisplayInstance>());
  FX_DCHECK(inserted);

  auto& instance = new_instance_iterator->second;
  instance->loop = std::make_shared<utils::LoopDispatcherHolder>(
      &kAsyncLoopConfigNoAttachToCurrentThread,
      [this](std::unique_ptr<async::Loop> loop_to_destroy) {
        // Destroying a loop on its own dispatcher deadlocks, as it tries to join its own thread.
        FX_DCHECK(loop_to_destroy->dispatcher() != this->cleanup_loop_.dispatcher());

        async::PostTask(this->cleanup_loop_.dispatcher(),
                        [loop_to_destroy = std::move(loop_to_destroy), this]() mutable {
                          loop_to_destroy.reset();  // Explicitly destroy the loop.
                          // Allow FlatlandManager's dtor to continue destroying cleanup_loop_.
                          this->alive_sessions_--;
                        });
      });
  instance->display = hw_display;
  instance->impl = FlatlandDisplay::New(
      instance->loop, std::move(request), id, hw_display,
      [this, id] { DestroyInstanceFunction(id); }, flatland_presenter_, link_system_,
      uber_struct_system_->AllocateQueueForSession(id));

  auto dpr_callback = [this](const glm::vec2& dpr) { link_system_->UpdateDevicePixelRatio(dpr); };
  hw_display->SetDPRCallback(std::move(dpr_callback));
  link_system_->UpdateDevicePixelRatio(hw_display->device_pixel_ratio());

  const std::string name = "Flatland Display ID=" + std::to_string(id);
  zx_status_t status = instance->loop->loop().StartThread(name.c_str());
  FX_DCHECK(status == ZX_OK);

  alive_sessions_++;
}

void FlatlandManager::UpdateInstances(
    const std::unordered_map<scheduling::SessionId, scheduling::PresentId>& instances_to_update) {
  TRACE_DURATION("gfx", "FlatlandManager::UpdateInstances", "count",
                 TA_UINT64(instances_to_update.size()));
  utils::CheckIsOnMainThread();

  const auto results = uber_struct_system_->UpdateInstances(instances_to_update);

  // Prepares the return of tokens to each session that didn't fail to update.
  for (const auto& [session_id, present_credits_returned] : results.present_credits_returned) {
    FX_DCHECK((flatland_instances_.find(session_id) != flatland_instances_.end()) ||
              (flatland_display_instances_.find(session_id) != flatland_display_instances_.end()));

    // TODO(https://fxbug.dev/42156567): we currently only keep track of present tokens for Flatland
    // sessions, not FlatlandDisplay sessions.  It's not clear what we could do with them for
    // FlatlandDisplay: there is no API that would allow sending them to the client.  Maybe the
    // current approach is OK?  Maybe we should DCHECK that |present_credits_returned| is only
    // non-zero for Flatlands, not FlatlandDisplays?

    // Add the session to the map of updated_sessions, and increment the number of present tokens it
    // should receive after the firing of the SendHintsToStartRendering().
    if (flatland_instances_updated_.find(session_id) == flatland_instances_updated_.end()) {
      flatland_instances_updated_[session_id] = 0;
    }
    flatland_instances_updated_[session_id] += present_credits_returned;
  }
}

void FlatlandManager::SendHintsToStartRendering() {
  TRACE_DURATION("gfx", "FlatlandManager::SendHintsToStartRendering");
  utils::CheckIsOnMainThread();

  if (all_clients_opt_out_present_info_) {
    // We know that no clients want to receive `-> OnNextFrameBegin`, so avoid the costly
    // computation of `GetFuturePresentationInfos()` below.
    return;
  }

  // Compute future frame info and send it to all Flatland instances that had updates this frame.
  //
  // `this` is safe to capture, as the callback is guaranteed to run on the calling thread.
  const std::vector<scheduling::FuturePresentationInfo> presentation_infos =
      flatland_presenter_->GetFuturePresentationInfos();
  for (const auto& [session_id, present_credits_returned] : flatland_instances_updated_) {
    auto instance_kv = flatland_instances_.find(session_id);

    // Skip sessions that have exited since their frame was rendered.
    if (instance_kv == flatland_instances_.end()) {
      continue;
    }

    // Skip sessions who aren't using present credits.
    if (instance_kv->second->impl->config().skips_present_credits) {
      continue;
    }

    // Make a copy of the vector manually.
    Flatland::FuturePresentationInfos presentation_infos_copy(presentation_infos.size());
    for (size_t i = 0; i < presentation_infos.size(); ++i) {
      auto& info = presentation_infos[i];
      fuchsia_scenic_scheduling::PresentationInfo info_copy;
      info_copy.latch_point(info.latch_point.get());
      info_copy.presentation_time(info.presentation_time.get());
      presentation_infos_copy[i] = std::move(info_copy);
    }

    // The first time we send credits we should send the maximum amount for the client to get
    // started.
    uint32_t credits_returned = present_credits_returned;
    if (!instance_kv->second->initial_credits_returned) {
      credits_returned = scheduling::FrameScheduler::kMaxPresentsInFlight;
      instance_kv->second->initial_credits_returned = true;
    }

    SendPresentCredits(instance_kv->second.get(), credits_returned,
                       std::move(presentation_infos_copy));
  }

  // Prepare map for the next frame.
  flatland_instances_updated_.clear();
}

void FlatlandManager::OnFramePresented(
    const std::unordered_map<scheduling::SessionId,
                             std::map<scheduling::PresentId, /*latched_time*/ zx::time>>&
        latched_times,
    scheduling::PresentTimestamps present_times) {
  TRACE_DURATION("gfx", "FlatlandManager::OnFramePresented");

  utils::CheckIsOnMainThread();

  for (const auto& [session_id, latch_times] : latched_times) {
    auto instance_kv = flatland_instances_.find(session_id);

    // Skip sessions that have exited since their frame was rendered.
    if (instance_kv == flatland_instances_.end()) {
      continue;
    }

    SendFramePresented(instance_kv->second.get(), latch_times, present_times);
  }
}

size_t FlatlandManager::GetSessionCount() const { return flatland_instances_.size(); }

async_dispatcher_t* FlatlandManager::GetSessionDispatcherForTest(
    scheduling::SessionId session_id) const {
  auto it = flatland_instances_.find(session_id);
  if (it == flatland_instances_.end()) {
    return nullptr;
  }
  return it->second->loop->dispatcher();
}

std::vector<scheduling::SessionId> FlatlandManager::GetSessionIdsForTest() const {
  std::vector<scheduling::SessionId> ids;
  for (const auto& [id, _] : flatland_instances_) {
    ids.push_back(id);
  }
  return ids;
}

void FlatlandManager::SendPresentCredits(FlatlandInstance* instance,
                                         uint32_t present_credits_returned,
                                         Flatland::FuturePresentationInfos presentation_infos) {
  TRACE_DURATION("gfx", "FlatlandManager::SendPresentCredits");
  utils::CheckIsOnMainThread();

  // The Flatland impl must be accessed on the thread it is bound to.
  std::weak_ptr<Flatland> weak_impl = instance->impl;
  utils::ExecuteOrPostTaskOnDispatcher(
      instance->loop->dispatcher(), [weak_impl, present_credits_returned,
                                     presentation_infos = std::move(presentation_infos)]() mutable {
        auto impl = weak_impl.lock();
        FX_CHECK(impl) << "Missing Flatland instance in SendPresentCredits().";
        impl->OnNextFrameBegin(present_credits_returned, std::move(presentation_infos));
      });
}

void FlatlandManager::SendFramePresented(
    FlatlandInstance* instance,
    const std::map<scheduling::PresentId, /*latched_time*/ zx::time>& latched_times,
    scheduling::PresentTimestamps present_times) {
  utils::CheckIsOnMainThread();

  if (instance->impl->config().skips_on_frame_presented) {
    // This Flatland session has opted out of `-> OnFramePresented` events.
    return;
  }

  // The Flatland impl must be accessed on the thread it is bound to.
  std::weak_ptr<Flatland> weak_impl = instance->impl;
  utils::ExecuteOrPostTaskOnDispatcher(
      instance->loop->dispatcher(), [weak_impl, latched_times, present_times]() {
        auto impl = weak_impl.lock();
        FX_CHECK(impl) << "Missing Flatland instance in SendFramePresented().";
        impl->OnFramePresented(latched_times, present_times);
      });
}

void FlatlandManager::RemoveFlatlandInstance(scheduling::SessionId session_id) {
  utils::CheckIsOnMainThread();

  bool found = false;

  {
    auto instance_kv = flatland_instances_.find(session_id);
    if (instance_kv != flatland_instances_.end()) {
      found = true;
      auto& instance = instance_kv->second;
      const bool is_main_thread_session = (instance->loop->dispatcher() == executor_.dispatcher());

      if (is_main_thread_session) {
        // Cleanup trusted Flatland sessions that run on Scenic's main thread.
        // Destroy the implementation immediately, then decrement session count.
        instance->impl.reset();
        alive_sessions_--;
      } else {
        // Cleanup untrusted Flatland sessions that run on a dedicated worker thread.
        // Post to the Flatland session's worker thread to destroy Flatland impl.
        // Deleting the instance on the worker thread triggers ~LoopDispatcherHolder()
        // which safely deletes the Loop via the cleanup_loop_.
        async::PostTask(instance->loop->dispatcher(), [instance = std::move(instance)]() {
          TRACE_DURATION("gfx", "FlatlandManager::RemoveFlatlandInstance[task]");

          // A flatland instance must release all its resources, and its loop must be
          // destroyed on cleanup_loop_, before |alive_sessions_| can safely be
          // decremented. This ensures that flatland manager outlives every flatland
          // instance.
          instance->impl.reset();
          // Actually decrement alive_sessions_ in instance loop destruction thunk.
        });
      }
      flatland_instances_.erase(session_id);
    }
  }
  {
    auto instance_kv = flatland_display_instances_.find(session_id);
    if (instance_kv != flatland_display_instances_.end()) {
      found = true;
      // Below, we push destruction of the object to a different thread.  But first, we need to
      // relinquish ownership of the display.
      instance_kv->second->display->Unclaim();

      // The Flatland impl must be destroyed on the thread that owns the looper it is
      // bound to. Remove the instance from the map, then push cleanup onto the worker thread. Note
      // that the closure exists only to transfer the cleanup responsibilities to the worker thread.
      //
      // Note: Capturing "this" is safe as a flatland manager is guaranteed to outlive any flatland
      // display instance.
      async::PostTask(
          instance_kv->second->loop->dispatcher(), [instance = std::move(instance_kv->second)]() {
            TRACE_DURATION("gfx", "FlatlandManager::RemoveFlatlandInstance[display/task]");

            // A flatland display instance must release all its resources, and its loop
            // must be destroyed on cleanup_loop_, before |alive_sessions_| can safely
            // be decremented. This ensures that flatland manager outlives every
            // flatland instance.
            instance->impl.reset();
            // Actually decrement alive_sessions_ in instance loop destruction thunk.
          });
      flatland_display_instances_.erase(session_id);
    }
  }
  FX_DCHECK(found) << "No instance or display with ID: " << session_id;

  // Other resource cleanup can safely occur on the main thread.
  uber_struct_system_->RemoveSession(session_id);
}

void FlatlandManager::DestroyInstanceFunction(scheduling::SessionId session_id) {
  // This function is called on the Flatland instance thread, but the instance removal must be
  // triggered from the main thread since it accesses and modifies the |flatland_instances_| map.
  executor_.schedule_task(
      fpromise::make_promise([this, session_id] { this->RemoveFlatlandInstance(session_id); }));
}

std::shared_ptr<FlatlandDisplay> FlatlandManager::GetPrimaryFlatlandDisplayForRendering() {
  FX_CHECK(flatland_display_instances_.size() <= 1);
  return flatland_display_instances_.empty() ? nullptr
                                             : flatland_display_instances_.begin()->second->impl;
}

}  // namespace flatland
