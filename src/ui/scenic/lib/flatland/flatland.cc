// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/flatland.h"

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/natural_ostream.h>
#include <fidl/fuchsia.ui.composition/cpp/natural_types.h>
#include <lib/async/default.h>
#include <lib/async/time.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <lib/zx/eventpair.h>
#include <limits.h>
#include <zircon/errors.h>

#include <array>
#include <cstdint>
#include <functional>
#include <memory>
#include <ranges>
#include <span>
#include <sstream>
#include <string>
#include <unordered_set>
#include <utility>
#include <vector>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/scheduling/id.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/logging.h"
#include "src/ui/scenic/lib/utils/validate_eventpair.h"

#include <glm/gtc/constants.hpp>
#include <glm/gtc/matrix_access.hpp>
#include <glm/gtc/type_ptr.hpp>

using fuchsia_ui_composition::FlatlandError;
using fuchsia_ui_composition::OnNextFrameBeginValues;
using fuchsia_ui_composition::Orientation;

namespace {

// Handle floating point errors up to an epsilon for sample region calls.
void ClampIfNear(float* val, float difference) {
  if (difference > 0.f && difference < 1e-3f) {
    *val -= difference;
  }
}

std::optional<std::string> ValidateViewportProperties(
    const fuchsia_ui_composition::wire::ViewportProperties& properties) {
  if (properties.has_logical_size()) {
    const auto& logical_size = properties.logical_size();
    if (logical_size.width == 0 || logical_size.height == 0) {
      std::ostringstream stream;
      stream << "Logical_size components must be positive, given (" << logical_size.width << ", "
             << logical_size.height << ")";
      return stream.str();
    }
  }

  if (properties.has_inset()) {
    const auto inset = properties.inset();
    if (inset.top < 0 || inset.right < 0 || inset.bottom < 0 || inset.left < 0) {
      std::ostringstream stream;
      stream << "Inset components must be >= 0, given (" << inset.top << ", " << inset.right << ", "
             << inset.bottom << ", " << inset.left << ")";
      return stream.str();
    }
  }

  return std::nullopt;
}

std::pair<zx::event, zx::event> CreateEventAndDup() {
  zx::event event1;
  zx_status_t status = zx::event::create(0, &event1);
  FX_DCHECK(status == ZX_OK);

  zx::event event2;
  status = event1.duplicate(ZX_RIGHT_SAME_RIGHTS, &event2);
  FX_DCHECK(status == ZX_OK);

  return {std::move(event1), std::move(event2)};
}

}  // namespace

namespace flatland {

std::shared_ptr<Flatland> Flatland::New(
    std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
    fidl::ServerEnd<fuchsia_ui_composition::Flatland> server_end, scheduling::SessionId session_id,
    std::function<void()> destroy_instance_function,
    std::shared_ptr<FlatlandPresenter> flatland_presenter, std::shared_ptr<LinkSystem> link_system,
    std::shared_ptr<UberStructSystem::UberStructQueue> uber_struct_queue,
    const std::vector<std::shared_ptr<allocation::BufferCollectionImporter>>&
        buffer_collection_importers,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_views::Focuser>, zx_koid_t)>
        register_view_focuser,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_views::ViewRefFocused>, zx_koid_t)>
        register_view_ref_focused,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::TouchSource>, zx_koid_t)>
        register_touch_source,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::MouseSource>, zx_koid_t)>
        register_mouse_source,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2>, zx_koid_t)>
        register_touch_source_v2,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2>, zx_koid_t)>
        register_mouse_source_v2,
    const FlatlandConfig& config) {
  // clang-format off
  auto flatland = std::shared_ptr<Flatland>(new Flatland(
      dispatcher_holder,
      session_id,
      std::move(flatland_presenter),
      std::move(link_system),
      std::move(uber_struct_queue),
      buffer_collection_importers,
      std::move(register_view_focuser),
      std::move(register_view_ref_focused),
      std::move(register_touch_source),
      std::move(register_mouse_source),
      std::move(register_touch_source_v2),
      std::move(register_mouse_source_v2),
      config));
  // clang-format on

  // Natural FIDL bindings must be created and deleted on the same thread that it handles messages.
  async::PostTask(dispatcher_holder->dispatcher(),
                  [flatland, server_end = std::move(server_end),
                   destroy_instance_function = std::move(destroy_instance_function)]() mutable {
                    flatland->Bind(std::move(server_end), std::move(destroy_instance_function));
                  });

  return flatland;
}

Flatland::Flatland(
    std::shared_ptr<utils::DispatcherHolder> dispatcher_holder, scheduling::SessionId session_id,
    std::shared_ptr<FlatlandPresenter> flatland_presenter, std::shared_ptr<LinkSystem> link_system,
    std::shared_ptr<UberStructSystem::UberStructQueue> uber_struct_queue,
    const std::vector<std::shared_ptr<allocation::BufferCollectionImporter>>&
        buffer_collection_importers,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_views::Focuser>, zx_koid_t)>
        register_view_focuser,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_views::ViewRefFocused>, zx_koid_t)>
        register_view_ref_focused,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::TouchSource>, zx_koid_t)>
        register_touch_source,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::MouseSource>, zx_koid_t)>
        register_mouse_source,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2>, zx_koid_t)>
        register_touch_source_v2,
    fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2>, zx_koid_t)>
        register_mouse_source_v2,
    const FlatlandConfig& config)
    : dispatcher_holder_(std::move(dispatcher_holder)),
      session_id_(session_id),
      present2_helper_([this](fuchsia_scenic_scheduling::FramePresentedInfo info) {
        // If this callback is invoked, we know that `Present()` must have been called, and
        // therefore also know that binding must have been completed, because otherwise `Present()`
        // wouldn't have been called.
        //
        // Caveat: in Flatland unit tests, we invoke methods directly on the Flatland object, not
        // via a FIDL client.  It is conceivable that flakes might arise if the timing relationship
        // with the scheduler changes.
        if (this->binding_data_) {
          this->binding_data_->SendOnFramePresented(std::move(info));
        }
      }),
      flatland_presenter_(std::move(flatland_presenter)),
      link_system_(std::move(link_system)),
      uber_struct_queue_(std::move(uber_struct_queue)),
      buffer_collection_importers_(buffer_collection_importers),
      transforms_(&pool_),
      transform_graph_(session_id_),
      local_root_(transform_graph_.CreateTransform()),
      content_handles_(&pool_),
      layer_handles_(&pool_),
      images2_(&pool_),
      layer_objects_(&pool_),
      layer_stacks_(&pool_),
      layer_stack_handles_(&pool_),
      image_objects_(&pool_),
      error_reporter_(scenic_impl::ErrorReporter::DefaultUnique()),
      pending_image_releases_(&pool_),
      images_to_release_on_present_(&pool_),
      import_tokens_(
          std::make_shared<
              std::unordered_map<allocation::GlobalImageId,
                                 fuchsia_ui_composition::wire::BufferCollectionImportToken>>()),
      register_view_focuser_(std::move(register_view_focuser)),
      register_view_ref_focused_(std::move(register_view_ref_focused)),
      register_touch_source_(std::move(register_touch_source)),
      register_mouse_source_(std::move(register_mouse_source)),
      register_touch_source_v2_(std::move(register_touch_source_v2)),
      register_mouse_source_v2_(std::move(register_mouse_source_v2)),
      config_(config),
      executor_(dispatcher_holder_->dispatcher()) {
  FX_DCHECK(flatland_presenter_);

  FX_LOGS(INFO) << "Flatland NEW session_id=" << session_id_;

  // Pre-allocate space to avoid initial heap allocations.
  pending_create_image_fences_.reserve(8);
}

void Flatland::Bind(fidl::ServerEnd<fuchsia_ui_composition::Flatland> server_end,
                    std::function<void()> destroy_instance_function) {
  // Only called once, by the constructor.
  FX_DCHECK(!binding_data_);
  binding_data_ =
      std::make_unique<BindingData>(this, dispatcher_holder_->dispatcher(), std::move(server_end),
                                    std::move(destroy_instance_function));

  FLATLAND_VERBOSE_LOG << "Flatland session_id=" << session_id_ << " bound to FIDL channel.";
}

Flatland::BindingData::BindingData(Flatland* flatland, async_dispatcher_t* dispatcher,
                                   fidl::ServerEnd<fuchsia_ui_composition::Flatland> server_end,
                                   std::function<void()> destroy_instance_function)
    : binding_(dispatcher, std::move(server_end), flatland, std::mem_fn(&Flatland::OnFidlClosed)),
      destroy_instance_function_(std::move(destroy_instance_function)) {}

Flatland::BindingData::~BindingData() { destroy_instance_function_(); }

void Flatland::BindingData::SendOnFramePresented(
    fuchsia_scenic_scheduling::FramePresentedInfo info) {
  auto result = fidl::SendEvent(binding_)->OnFramePresented(
      fuchsia_ui_composition::FlatlandOnFramePresentedRequest(std::move(info)));
  if (result.is_error()) {
    auto& error = result.error_value().error();
    FX_LOGS(WARNING) << "SendOnFramePresented(): error while sending FIDL event: " << error.status()
                     << " " << error.status_string();
  }
}

void Flatland::BindingData::SendOnNextFrameBegin(uint32_t additional_present_credits,
                                                 FuturePresentationInfos presentation_infos) {
  OnNextFrameBeginValues values;
  values.additional_present_credits(additional_present_credits);
  values.future_presentation_infos(std::move(presentation_infos));

  auto result = fidl::SendEvent(binding_)->OnNextFrameBegin(
      fuchsia_ui_composition::FlatlandOnNextFrameBeginRequest(std::move(values)));
  if (result.is_error()) {
    auto& error = result.error_value().error();
    FX_LOGS(WARNING) << "SendOnNextFrameBegin(): error while sending FIDL event: " << error.status()
                     << " " << error.status_string();
  }
}

void Flatland::BindingData::CloseConnection(FlatlandError error) {
  // NOTE: there's no need to test the return values of OnError()/Cancel()/Close().  If they fail,
  // the binding and waiter will be cleaned up anyway because we'll soon be destroyed (since
  // destroy_instance_function_ has been or will be invoked).

  // Send the error to the client before closing the connection.
  auto result = fidl::SendEvent(binding_)->OnError(error);
  if (result.is_error()) {
    auto& error = result.error_value().error();
    FX_LOGS(WARNING) << "CloseConnection(): error while sending FIDL event: " << error.status()
                     << " " << error.status_string();
  }

  // Immediately close the FIDL interface to prevent future requests.
  binding_.Close(ZX_ERR_BAD_STATE);
}

Flatland::~Flatland() {
  // TODO(https://fxbug.dev/42132996): consider if Link tokens should be returned or not.

  // Clear the scene graph and process dead transforms (they're all dead after clearing).
  // This guarantees that all images will be available to release.
  Clear();
  auto data = transform_graph_.ComputeAndCleanup(GetRoot(), std::numeric_limits<uint64_t>::max());
  ProcessDeadTransforms(data);
  FX_CHECK(image_objects_.empty());

  // Gather all unreleased images; if any exist they will be released by a closure that outlives
  // this Flatland session, hence the heap-allocated vector.
  std::vector<allocation::GlobalImageId> images_to_release(images_to_release_on_present_.begin(),
                                                           images_to_release_on_present_.end());
  for (const auto& pending : pending_image_releases_) {
    images_to_release.insert(images_to_release.end(), pending.ids.begin(), pending.ids.end());
  }
  pending_image_releases_.clear();  // cancels every pending wait, on this thread

  // If there are any images to release, set up a waiter, and pass the event-to-be-signaled to
  // `FlatlandPresenter::RemoveSession`.  This will schedule another frame and signal the event
  // just like any other release fence.
  std::optional<zx::event> image_release_fence;
  if (!images_to_release.empty()) {
    zx::event evt = utils::CreateEvent();
    image_release_fence = utils::CopyZxHandle(evt);

    auto wait = std::make_shared<async::WaitOnce>(evt.get(), ZX_EVENT_SIGNALED);
    zx_status_t status = wait->Begin(
        dispatcher(),
        [importer_refs = buffer_collection_importers_,
         images_to_release = std::move(images_to_release), import_tokens = import_tokens_,
         // We keep several objects alive in the closure:
         //   - the dispatcher, which is about to be released by the Flatland and FlatlandManager.
         //   - the wait object keeps itself alive via the ref in this closure
         //   - the waited-upon fence event: we retain a copy of the handle to avoid reasoning about
         //     whether the FlatlandPresenter implementation will safely keep it alive.
         keepalive_dispatcher = dispatcher_holder_, keepalive_wait = wait,
         keepalive_evt = std::move(evt)](async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                                         const zx_packet_signal_t* /*signal*/) mutable {
          for (auto& image_id : images_to_release) {
            import_tokens->erase(image_id);
            for (auto& importer : importer_refs) {
              importer->ReleaseBufferImage(image_id);
            }
          }
        });
    FX_DCHECK(status == ZX_OK);
  }

  // This will signal the release fence (if any) that we pass to it, and therefore enable the wait
  // above to succeed.
  flatland_presenter_->RemoveSession(session_id_, std::move(image_release_fence));

  FX_LOGS(INFO) << "Flatland DESTROYED session_id=" << session_id_;
}

bool Flatland::IsLayerSampleRectValidForPresent(const LayerObject& layer_obj) const {
  if (layer_obj.mode == LayerObject::Mode::kImage &&
      layer_obj.image_mode.image_id != allocation::kInvalidImageId) {
    const auto& sample_rect = layer_obj.image_mode.sample_rect;
    const float img_w = static_cast<float>(layer_obj.image_mode.image_width);
    const float img_h = static_cast<float>(layer_obj.image_mode.image_height);
    if (sample_rect.x() < 0.f || sample_rect.y() < 0.f ||
        (sample_rect.x() + sample_rect.width()) > img_w ||
        (sample_rect.y() + sample_rect.height()) > img_h) {
      error_reporter_->ERROR() << "Present: sample_rect " << sample_rect
                               << " exceeds bound image extent (0, 0, " << img_w << ", " << img_h
                               << ")";
      return false;
    }
  }

  return true;
}

void Flatland::Present(PresentRequestView request, PresentCompleter::Sync& completer) {
  Present(request->args);
}

void Flatland::Present(fuchsia_ui_composition::wire::PresentArgs& args) {
  // In Flatland unit tests, we invoke methods directly on this object, rather than using a FIDL
  // client over a Zircon channel.  In production situations, the channel is torn down at or before
  // the time that `binding_data_` is destroyed, and therefore there will be no subsequent method
  // invocations, including of `Present()`.
  if (!binding_data_) {
    FX_LOGS(WARNING)
        << "Ignoring Flatland::Present() called after binding_data_ was destroyed in session: "
        << session_id_ << "\nThis should not occur outside of unit tests.";
    return;
  }

  // Utilized in low-hanging optimizations below.  If desired, we could also optimize TRACE_DURATION
  // calls, although (because we could no longer rely on a RAII scope for duration) we would need
  // to split each into a TRACE_DURATION_BEGIN/TRACE_DURATION_END pair.
  const bool trace_enabled = TRACE_CATEGORY_ENABLED("gfx");

  TRACE_DURATION("gfx", "Flatland::Present", "debug_name", TA_STRING(debug_name_.c_str()));
  std::string per_app_tracing_name = "Flatland::PerAppPresent[" + debug_name_ + "]";
  TRACE_DURATION("gfx", per_app_tracing_name.c_str());
  if (trace_enabled) {
    TRACE_FLOW_END("gfx", per_app_tracing_name.c_str(), present_count_);
  }

  ++present_count_;

  // Close any clients that had invalid operations on link protocols.
  if (link_protocol_error_) {
    const char* kError = "Link protocol error";
    FLATLAND_VERBOSE_LOG << "Flatland::Present() session_id=" << session_id_
                         << "  present_count=" << present_count_
                         << "  closing connection: " << kError;
    error_reporter_->ERROR() << kError;
    CloseConnection(FlatlandError::kBadHangingGet);
    return;
  }

  if (!config_.skips_present_credits) {
    // Close any clients that call Present() without any present tokens.
    if (present_credits_ == 0) {
      const char* kError = "Out of present credits";
      FLATLAND_VERBOSE_LOG << "Flatland::Present() session_id=" << session_id_
                           << "  present_count=" << present_count_
                           << "  closing connection: " << kError;
      error_reporter_->ERROR() << kError;
      CloseConnection(FlatlandError::kNoPresentsRemaining);
      return;
    }
    present_credits_--;
  }

  const uint64_t requested_presentation_time =
      args.has_requested_presentation_time() ? args.requested_presentation_time() : 0;
  const bool unsquashable = args.has_unsquashable() ? args.unsquashable() : false;

  std::vector<zx::event> release_fences;
  if (args.has_release_fences()) {
    release_fences.reserve(args.release_fences().size());
    for (auto& fence : args.release_fences()) {
      release_fences.push_back(std::move(fence));
    }
  }

  std::vector<zx::event> acquire_fences;
  if (args.has_acquire_fences()) {
    acquire_fences.reserve(args.acquire_fences().size());
    for (auto& fence : args.acquire_fences()) {
      acquire_fences.push_back(std::move(fence));
    }
  }

  std::vector<zx::counter> present_fences;
  if (args.has_present_fences()) {
    present_fences.reserve(args.present_fences().size());
    for (auto& fence : args.present_fences()) {
      present_fences.push_back(std::move(fence));
    }
  }

  std::vector<zx::counter> release_counters;
  if (args.has_release_counters()) {
    release_counters.reserve(args.release_counters().size());
    for (auto& fence : args.release_counters()) {
      release_counters.push_back(std::move(fence));
    }
  }

  auto root_handle = GetRoot();
  auto uber_struct = std::make_unique<UberStruct>();

  // TODO(https://fxbug.dev/42116832): Decide on a proper limit on compute time for topological
  // sorting.
  auto data = transform_graph_.ComputeAndCleanup(root_handle, std::numeric_limits<uint64_t>::max(),
                                                 uber_struct->resource());
  FX_DCHECK(data.iterations != std::numeric_limits<uint64_t>::max());

  // Don't commit changes if a cycle is detected. Instead, kill the channel and remove the sub-graph
  // from the global graph (the latter is the responsibility of the manager that is notified by
  // `BindingData::destroy_instance_function_`).
  if (!data.cyclical_edges.empty()) {
    FLATLAND_VERBOSE_LOG << "Flatland::Present() session_id=" << session_id_
                         << "  present_count=" << present_count_
                         << "  closing connection: Cycle was detected";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  FX_DCHECK(data.sorted_transforms[0].handle == root_handle);

  // Drop dead transforms and the layer stacks they hosted.  Any image whose last ref this releases
  // is queued in `images_to_release_on_present_`, which the block below drains.
  ProcessDeadTransforms(data);

  if (!images_to_release_on_present_.empty()) {
    zx::event fence;
    zx_status_t status = zx::event::create(0, &fence);
    FX_DCHECK(status == ZX_OK);
    zx::event fence_for_presenter = utils::CopyZxHandle(fence);

    // The `PendingImageRelease` record owns its fence dup and its wait; the handler captures only
    // `this` and the record's iterator.  If the session is destroyed first, `~Flatland()` cancels
    // this wait, taking responsibility for cleaning up all remaining images in the session.
    pending_image_releases_.emplace_back(std::move(fence),
                                         std::move(images_to_release_on_present_));
    images_to_release_on_present_.clear();
    auto it = std::prev(pending_image_releases_.end());
    status =
        it->wait.Begin(dispatcher(), [this, it](async_dispatcher_t*, async::WaitOnce*,
                                                zx_status_t status, const zx_packet_signal_t*) {
          // Cancellation never reaches this handler: the wait is cancelled by
          // its own destructor, on this thread, before the dispatcher dies.
          FX_DCHECK(status == ZX_OK) << "status is: " << zx_status_get_string(status);
          ReleaseImages(it->ids);
          pending_image_releases_.erase(it);
        });
    FX_DCHECK(status == ZX_OK) << "status is: " << status;

    release_fences.push_back(std::move(fence_for_presenter));
  }

  {
    TRACE_DURATION("gfx", "Flatland::Present[populate_uberstruct]");

    uber_struct->local_topology = std::move(data.sorted_transforms);

    uber_struct->local_matrices.reserve(matrices_.size());
    for (const auto& [handle, matrix_data] : matrices_) {
      uber_struct->local_matrices[handle] = matrix_data.GetMatrix();
    }

    uber_struct->local_opacity_values.reserve(opacity_values_.size());
    uber_struct->local_opacity_values.insert(opacity_values_.begin(), opacity_values_.end());

    uber_struct->local_clip_regions.reserve(clip_regions_.size());
    uber_struct->local_clip_regions.insert(clip_regions_.begin(), clip_regions_.end());

    // + 1 for optional full screen hit region
    uber_struct->local_hit_regions_map.reserve(hit_regions_.size() + 1);
    for (const auto& [handle, regions] : hit_regions_) {
      uber_struct->local_hit_regions_map[handle].assign(regions.begin(), regions.end());
    }

    // As per the default hit region policy, if the client has not explicitly set a hit region on
    // the root, add a full screen one.
    if (root_transform_.GetInstanceId() != 0 &&
        hit_regions_.find(root_transform_) == hit_regions_.end()) {
      uber_struct->local_hit_regions_map[root_transform_] = {{flatland::HitRegion::Infinite()}};
    }

    for (const auto& transform : uber_struct->local_topology) {
      if (auto it = layer_stacks_.find(transform.handle); it != layer_stacks_.end()) {
        std::pmr::vector<LayerHandle> handles(it->second.layers.begin(), it->second.layers.end(),
                                              uber_struct->resource());
        uber_struct->layer_stacks.emplace(transform.handle, std::move(handles));

        for (auto layer_handle : it->second.layers) {
          auto layer_obj_it = layer_objects_.find(layer_handle);
          FX_DCHECK(layer_obj_it != layer_objects_.end());

          // The UberStruct contains only those layers which are currently in a layer stack.
          auto [us_layer_it, inserted] = uber_struct->layers.try_emplace(layer_handle);
          if (inserted) {
            // Must copy properties for the newly-inserted layer.  Common properties are always
            // copied, and only the mode-specific properties which match the composition mode
            // are copied.
            auto& us_layer = us_layer_it->second;
            const auto& layer_obj = layer_obj_it->second;

            if (!IsLayerSampleRectValidForPresent(layer_obj)) {
              CloseConnection(FlatlandError::kBadOperation);
              return;
            }

            us_layer.common = layer_obj.common;
            switch (layer_obj.mode) {
              case LayerObject::Mode::kInvisible:
                // The variant defaults to std::monostate, so nothing to do.
                break;
              case LayerObject::Mode::kImage:
                us_layer.content = layer_obj.image_mode;
                break;
              case LayerObject::Mode::kSolidColor:
                us_layer.content = layer_obj.solid_color_mode;
                break;
            }
          }
        }
      }
    }

    if (link_to_parent_.has_value()) {
      uber_struct->view_ref = link_to_parent_->view_ref;
    }

    uber_struct->debug_name.assign(debug_name_);
    uber_struct->creation_time = zx::time_monotonic(async_now(dispatcher()));
    uber_struct->flatland_version = config_.use_flatland2 ? 2u : 1u;
  }

  // Obtain the PresentId which is needed to:
  // - enqueue the UberStruct.
  // - schedule a frame
  // - notify client when the frame has been presented
  auto present_id = scheduling::GetNextPresentId();

  FLATLAND_VERBOSE_LOG << "Flatland::Present() session_id=" << session_id_
                       << "  present_count=" << present_count_ << "  present_id=" << present_id;

  if (!config_.skips_on_frame_presented) {
    // Must avoid calling `RegisterPresent()` because when `skips_on_frame_presented == true`,
    // `FlatlandManager` will avoid notifying this session that frames were presented.
    present2_helper_.RegisterPresent(present_id,
                                     /*present_received_time=*/uber_struct->creation_time);
  }

  // TODO(https://fxbug.dev/414450649): the flow using this nonce is load-bearing; it is relied upon
  // by `//sdk/testing/sl4f/client/lib/src/trace_processing/metrics/flutter_frame_stats.dart`.
  const trace_flow_id_t kLoadBearingTraceNonce = trace_enabled ? TRACE_NONCE() : 0;

  // Micro-optimize tracing.
  if (trace_enabled) {
    // TODO(https://fxbug.dev/414450649): remove this, since it is a subset of the
    // `scenic_session_present` flow.  This will require updating trace-processing scripts.
    TRACE_FLOW_BEGIN("gfx", "ScheduleUpdate", present_id);

    TRACE_FLOW_BEGIN("gfx", "wait_for_fences", kLoadBearingTraceNonce);

    TRACE_INSTAFLOW_BEGIN("gfx", "scenic_session_present", "flatland_present",
                          SESSION_TRACE_ID(session_id_, present_id), "session_id",
                          TA_UINT64(session_id_), "present_id", TA_UINT64(present_id));
  }

  // Decide whether this present requires recomputation of the view tree.  The current heuristic can
  // result in false positives (for example, just because a session has viewports doesn't mean that
  // this presentation affects those child views), but it works well in practice:
  // - components like window managers don't present often, and when they do they often do affect
  //   the view tree
  // - "leaf node" applications typically don't have child views
  const bool recompute_view_tree = !links_to_children_.empty() || view_tree_dirty_;
  view_tree_dirty_ = false;

  // Safe to capture |this| because the Flatland is guaranteed to outlive |fence_queue_|,
  // Flatland is non-movable and FenceQueue does not fire closures after destruction.
  // TODO(https://fxbug.dev/42156567): make the fences be the first arg, and the closure be the
  // second.
  auto task =
      [this, present_id, requested_presentation_time, unsquashable,
       uber_struct = std::move(uber_struct), link_operations = std::move(pending_link_operations_),
       release_fences = std::move(release_fences), release_counters = std::move(release_counters),
       present_fences = std::move(present_fences), trace_enabled, kLoadBearingTraceNonce,
       recompute_view_tree]() mutable {
        // NOTE: this name is important for benchmarking.  Do not remove or modify it
        // without also updating the "process_gfx_trace.go" script.
        TRACE_DURATION("gfx", "scenic_impl::Session::ScheduleNextPresent", "session_id",
                       session_id_, "requested_presentation_time", requested_presentation_time);

        // Micro-optimize tracing.
        if (trace_enabled) {
          // TODO(https://fxbug.dev/414450649): Load-bearing.  See discussion at flow start.
          TRACE_FLOW_END("gfx", "wait_for_fences", kLoadBearingTraceNonce);

          TRACE_INSTAFLOW_STEP("gfx", "scenic_session_present", "acquire_fences_signaled",
                               SESSION_TRACE_ID(session_id_, present_id), "session_id",
                               TA_UINT64(session_id_), "present_id", TA_UINT64(present_id));
        }

        // Push the UberStruct, then schedule the associated Present that will eventually publish
        // it to the InstanceMap used for rendering.
        uber_struct_queue_->Push(present_id, std::move(uber_struct), recompute_view_tree);
        flatland_presenter_->ScheduleUpdateForSession(
            zx::time(requested_presentation_time), {session_id_, present_id}, unsquashable,
            std::move(release_fences), std::move(release_counters), std::move(present_fences),
            config_.schedule_asap);

        // Finalize Link destruction operations after publishing the new UberStruct. This
        // ensures that any local Transforms referenced by the to-be-deleted Links are already
        // removed from the now-published UberStruct.
        for (auto& operation : link_operations) {
          operation();
        }
      };

  // Append pending creation fences to acquire fences to ensure `CreateImage()` completes
  // before this `Present()` takes effect.
  // TODO(https://fxbug.dev/505749054): This is overly eager, and may unnecessarily delay the
  // current Present when the new image isn't referenced in the current UberStruct.
  if (!pending_create_image_fences_.empty()) {
    acquire_fences.insert(acquire_fences.end(),
                          std::make_move_iterator(pending_create_image_fences_.begin()),
                          std::make_move_iterator(pending_create_image_fences_.end()));
    pending_create_image_fences_.clear();
  }

  // TODO(https://fxbug.dev/474444799): If |config_.pass_acquire_fences| is true, these fences
  // can be directly queued on the render task rather than waiting on cpu. This will be possible
  // in the new Flatland API where we define per-layer fences.
  fence_queue_->QueueTask(std::move(task), std::move(acquire_fences));

  pending_link_operations_.clear();
}

void Flatland::CreateView(CreateViewRequestView request, CreateViewCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Flatland::CreateView", "debug_name", TA_STRING(debug_name_.c_str()));
  CreateView(std::move(request->token), std::move(request->parent_viewport_watcher));
}

void Flatland::CreateView(
    fuchsia_ui_views::wire::ViewCreationToken token,
    fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher> parent_viewport_watcher) {
  CreateViewHelper(std::move(token), std::move(parent_viewport_watcher), std::nullopt,
                   std::nullopt);
}

void Flatland::CreateView2(CreateView2RequestView request, CreateView2Completer::Sync& completer) {
  TRACE_DURATION("gfx", "Flatland::CreateView2", "debug_name", TA_STRING(debug_name_.c_str()));
  CreateView2(std::move(request->token), std::move(request->view_identity),
              std::move(request->protocols), std::move(request->parent_viewport_watcher));
}

void Flatland::CreateView2(
    fuchsia_ui_views::wire::ViewCreationToken token,
    fuchsia_ui_views::wire::ViewIdentityOnCreation view_identity,
    fuchsia_ui_composition::wire::ViewBoundProtocols protocols,
    fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher> parent_viewport_watcher) {
  CreateViewHelper(std::move(token), std::move(parent_viewport_watcher), std::move(view_identity),
                   std::move(protocols));
}

void Flatland::CreateViewHelper(
    fuchsia_ui_views::wire::ViewCreationToken token,
    fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher> parent_viewport_watcher,
    std::optional<fuchsia_ui_views::wire::ViewIdentityOnCreation> view_identity,
    std::optional<fuchsia_ui_composition::wire::ViewBoundProtocols> protocols) {
  // Attempting to link with an invalid token will never succeed, so its better to fail early and
  // immediately close the link connection.
  if (!token.value.is_valid()) {
    error_reporter_->ERROR() << "CreateView failed, ViewCreationToken was invalid";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (view_identity.has_value() &&
      !utils::validate_viewref(view_identity->view_ref_control, view_identity->view_ref)) {
    error_reporter_->ERROR() << "CreateView failed, ViewIdentityOnCreation was invalid";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  FX_DCHECK(link_system_);

  if (protocols.has_value()) {
    FX_DCHECK(view_identity.has_value()) << "required for view-bound protocols";
    if (!RegisterViewBoundProtocols(std::move(*protocols),
                                    utils::ExtractKoid(view_identity->view_ref))) {
      // `RegisterViewBoundProtocols()` already called `CloseConnection()`.
      return;
    }
  }
  // This portion of the method is not feed forward. This makes it possible for clients to receive
  // layout information before this operation has been presented. By initializing the link
  // immediately, parents can inform children of layout changes, and child clients can perform
  // layout decisions before their first call to Present().
  auto child_transform_handle = transform_graph_.CreateTransform();

  fuchsia_ui_views::ViewCreationToken natural_token({.value = std::move(token.value)});
  std::optional<fuchsia_ui_views::ViewIdentityOnCreation> natural_view_identity;
  if (view_identity.has_value()) {
    natural_view_identity = fuchsia_ui_views::ViewIdentityOnCreation(
        fuchsia_ui_views::ViewRef({.reference = std::move(view_identity->view_ref.reference)}),
        fuchsia_ui_views::ViewRefControl(
            {.reference = std::move(view_identity->view_ref_control.reference)}));
  }

  LinkSystem::LinkToParent new_link_to_parent = link_system_->CreateLinkToParent(
      dispatcher_holder_, std::move(natural_token), std::move(natural_view_identity),
      std::move(parent_viewport_watcher), child_transform_handle,
      [ref = weak_from_this(), weak_dispatcher_holder = std::weak_ptr<utils::DispatcherHolder>(
                                   dispatcher_holder_)](const std::string& error_log) {
        if (auto dispatcher_holder = weak_dispatcher_holder.lock()) {
          FX_CHECK(dispatcher_holder->dispatcher() == async_get_default_dispatcher())
              << "Link protocol error reported on the wrong dispatcher.";
        }
        if (auto impl = ref.lock())
          impl->ReportLinkProtocolError(error_log);
      });

  FLATLAND_VERBOSE_LOG << "Flatland::CreateView() session_id=" << session_id_
                       << "  link-attachment-point=" << child_transform_handle;

  // This portion of the method is feed-forward. The parent-child relationship between
  // |child_transform_handle| and |local_root_| establishes the Transform hierarchy between the two
  // instances, but the operation will not be visible until the next Present() call includes that
  // topology.
  if (link_to_parent_.has_value()) {
    bool child_removed =
        transform_graph_.RemoveChild(link_to_parent_->child_transform_handle, local_root_);
    FX_DCHECK(child_removed);

    bool transform_released =
        transform_graph_.ReleaseTransform(link_to_parent_->child_transform_handle);
    FX_DCHECK(transform_released);

    // Delay the destruction of the previous parent link until the next Present().
    pending_link_operations_.push_back([old_link_to_parent = std::move(link_to_parent_)]() mutable {
      old_link_to_parent.reset();
    });
  }

  {
    const bool child_added =
        transform_graph_.AddChild(new_link_to_parent.child_transform_handle, local_root_);
    FX_DCHECK(child_added);
  }
  link_to_parent_ = std::move(new_link_to_parent);

  view_tree_dirty_ = true;
}

bool Flatland::RegisterViewBoundProtocols(
    fuchsia_ui_composition::wire::ViewBoundProtocols protocols, const zx_koid_t view_ref_koid) {
  FX_DCHECK(register_view_focuser_);
  FX_DCHECK(register_view_ref_focused_);
  FX_DCHECK(register_touch_source_);
  FX_DCHECK(register_mouse_source_);
  FX_DCHECK(register_touch_source_v2_);
  FX_DCHECK(register_mouse_source_v2_);

  if (protocols.has_touch_source() && protocols.has_touch_source_v2()) {
    error_reporter_->ERROR() << "Cannot register both TouchSource and TouchSourceV2";
    CloseConnection(fuchsia_ui_composition::FlatlandError::kBadOperation);
    return false;
  }

  if (protocols.has_mouse_source() && protocols.has_mouse_source_v2()) {
    error_reporter_->ERROR() << "Cannot register both MouseSource and MouseSourceV2";
    CloseConnection(fuchsia_ui_composition::FlatlandError::kBadOperation);
    return false;
  }

  if (protocols.has_view_focuser()) {
    register_view_focuser_(std::move(protocols.view_focuser()), view_ref_koid);
  }

  if (protocols.has_view_ref_focused()) {
    register_view_ref_focused_(std::move(protocols.view_ref_focused()), view_ref_koid);
  }

  if (protocols.has_touch_source()) {
    register_touch_source_(std::move(protocols.touch_source()), view_ref_koid);
  }

  if (protocols.has_touch_source_v2()) {
    register_touch_source_v2_(std::move(protocols.touch_source_v2()), view_ref_koid);
  }

  if (protocols.has_mouse_source()) {
    register_mouse_source_(std::move(protocols.mouse_source()), view_ref_koid);
  }

  if (protocols.has_mouse_source_v2()) {
    register_mouse_source_v2_(std::move(protocols.mouse_source_v2()), view_ref_koid);
  }

  return true;
}

void Flatland::ReleaseView(ReleaseViewCompleter::Sync& completer) { ReleaseView(); }

void Flatland::ReleaseView() {
  FLATLAND_VERBOSE_LOG << "Flatland::ReleaseView() session_id=" << session_id_;

  if (!link_to_parent_) {
    error_reporter_->ERROR() << "ReleaseView failed, no existing parent Link";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  // Deleting the old LinkToParent's Transform effectively changes this intance's root back to
  // |local_root_|.
  bool child_removed =
      transform_graph_.RemoveChild(link_to_parent_->child_transform_handle, local_root_);
  FX_DCHECK(child_removed);

  bool transform_released =
      transform_graph_.ReleaseTransform(link_to_parent_->child_transform_handle);
  FX_DCHECK(transform_released);

  // Move the old parent link into the delayed operation so that it isn't taken into account when
  // computing the local topology, but doesn't get deleted until after the new UberStruct is
  // published.
  auto old_link_to_parent = std::move(link_to_parent_.value());
  link_to_parent_.reset();

  // Delay the actual destruction of the Link until the next Present().
  pending_link_operations_.push_back([old_link_to_parent = std::move(old_link_to_parent)]() {});
}

void Flatland::Clear(ClearCompleter::Sync& completer) { Clear(); }

void Flatland::Clear() {
  // Clear user-defined mappings and local matrices.
  transforms_.clear();
  content_handles_.clear();
  ClearFlatland2State();
  matrices_.clear();

  // We always preserve the link origin when clearing the graph. This call will place all other
  // TransformHandles in the dead_transforms set in the next Present(), which will trigger cleanup
  // of Images and BufferCollections.
  transform_graph_.ResetGraph(local_root_);

  // If a parent Link exists, delay its destruction until Present().
  if (link_to_parent_.has_value()) {
    auto local_link = std::move(link_to_parent_);
    link_to_parent_.reset();

    pending_link_operations_.push_back(
        [local_link = std::move(local_link)]() mutable { local_link.reset(); });
  }

  // Delay destruction of all child Links until Present().
  auto local_links = std::move(links_to_children_);
  links_to_children_.clear();

  pending_link_operations_.push_back(
      [local_links = std::move(local_links)]() mutable { local_links.clear(); });

  debug_name_.clear();
}

void Flatland::CreateTransform(CreateTransformRequestView request,
                               CreateTransformCompleter::Sync& completer) {
  CreateTransform(TransformId(request->transform_id.value));
}

void Flatland::CreateTransform(TransformId transform_id) {
  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "CreateTransform called with transform_id=" << kInvalidTransformId;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (transforms_.contains(transform_id)) {
    error_reporter_->ERROR() << "CreateTransform called with pre-existing transform_id="
                             << transform_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  TransformHandle transform_handle = transform_graph_.CreateTransform();
  FLATLAND_VERBOSE_LOG << "Flatland::CreateTransform() session_id=" << session_id_
                       << "  transform_id=" << transform_id << "  transform=" << transform_handle;
  transforms_.insert({transform_id, transform_handle});
}

void Flatland::SetTranslation(SetTranslationRequestView request,
                              SetTranslationCompleter::Sync& completer) {
  SetTranslation(TransformId(request->transform_id.value), request->translation);
}

void Flatland::SetTranslation(TransformId transform_id, fuchsia_math::wire::Vec translation) {
  FLATLAND_VERBOSE_LOG << "Flatland::SetTranslation() session_id=" << session_id_
                       << "  transform_id=" << transform_id << "  translation= (" << translation.x
                       << ", " << translation.y << ")";

  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetTranslation called with transform_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);

  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetTranslation failed, transform_id " << transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  matrices_[transform_kv->second].SetTranslation(translation);
}

void Flatland::SetOrientation(SetOrientationRequestView request,
                              SetOrientationCompleter::Sync& completer) {
  SetOrientation(TransformId(request->transform_id.value), request->orientation);
}

void Flatland::SetOrientation(TransformId transform_id,
                              fuchsia_ui_composition::Orientation orientation) {
  FLATLAND_VERBOSE_LOG << "Flatland::SetOrientation() session_id=" << session_id_
                       << "  transform_id=" << transform_id << "  orientation=" << orientation;

  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetOrientation called with transform_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);

  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetOrientation failed, transform_id " << transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  matrices_[transform_kv->second].SetOrientation(orientation);
}

void Flatland::SetScale(SetScaleRequestView request, SetScaleCompleter::Sync& completer) {
  SetScale(TransformId(request->transform_id.value), request->scale);
}

void Flatland::SetScale(TransformId transform_id, fuchsia_math::wire::VecF scale) {
  const float scale_x = scale.x;
  const float scale_y = scale.y;

  FLATLAND_VERBOSE_LOG << "Flatland::SetScale() session_id=" << session_id_
                       << "  transform_id=" << transform_id << "  scale= (" << scale_x << ", "
                       << scale_y << ")";

  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetScale called with transform_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);

  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetScale failed, transform_id " << transform_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (scale_x <= 0.f || scale_y <= 0.f) {
    error_reporter_->ERROR() << "SetScale failed, values must be positive (" << scale_x << ", "
                             << scale_y << " ).";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (isinf(scale_x) || isinf(scale_y) || isnan(scale_x) || isnan(scale_y)) {
    error_reporter_->ERROR() << "SetScale failed, invalid scale values (" << scale_x << ", "
                             << scale_y << " ).";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  matrices_[transform_kv->second].SetScale(scale);
}

void Flatland::SetOpacity(SetOpacityRequestView request, SetOpacityCompleter::Sync& completer) {
  SetOpacity(TransformId(request->transform_id.value), request->value);
}

void Flatland::SetOpacity(TransformId transform_id, float opacity) {
  FLATLAND_VERBOSE_LOG << "Flatland::SetOpacity() session_id=" << session_id_
                       << "  transform_id=" << transform_id << "  opacity=" << opacity;

  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetOpacity called with transform_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (isinf(opacity) || isnan(opacity)) {
    error_reporter_->ERROR() << "SetOpacity failed, invalid opacity value " << opacity;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (opacity < 0.f || opacity > 1.f) {
    error_reporter_->ERROR() << "Opacity value is not within valid range [0,1].";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);

  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetOpacity failed, transform_id " << transform_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  // Erase the value from the map since we store 1.f implicity.
  if (opacity == 1.f) {
    opacity_values_.erase(transform_kv->second);
  } else {
    opacity_values_[transform_kv->second] = opacity;
  }
}

void Flatland::SetClipBoundary(SetClipBoundaryRequestView request,
                               SetClipBoundaryCompleter::Sync& completer) {
  SetClipBoundary(
      TransformId(request->transform_id.value),
      request->rect ? std::optional<fuchsia_math::wire::Rect>(*request->rect) : std::nullopt);
}

void Flatland::SetClipBoundary(TransformId transform_id,
                               std::optional<fuchsia_math::wire::Rect> bounds) {
  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetClipBoundary called with transform_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);

  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetClipBoundary failed, transform_id " << transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  // If the optional bounds are empty, then remove them.
  if (!bounds.has_value()) {
    FLATLAND_VERBOSE_LOG << "Flatland::SetClipBoundary() session_id=" << session_id_
                         << "  transform_id=" << transform_id << "  ... clearing clip region";
    clip_regions_.erase(transform_kv->second);
    return;
  }

  if (!TransformClipRegion::IsValid(*bounds)) {
    error_reporter_->ERROR() << "SetClipBoundary failed, rectangle bounds overflow (" << bounds->x
                             << ", " << bounds->y << ", " << bounds->width << ", " << bounds->height
                             << ")";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  FLATLAND_VERBOSE_LOG << "Flatland::SetClipBoundary() session_id=" << session_id_
                       << "  transform_id=" << transform_id << "  rect=(" << bounds->x << ", "
                       << bounds->y << ", " << bounds->width << ", " << bounds->height << ")";
  SetClipBoundaryInternal(transform_kv->second, TransformClipRegion::From(*bounds));
}

void Flatland::SetClipBoundaryInternal(TransformHandle handle, TransformClipRegion bounds) {
  if (bounds.width() <= 0 || bounds.height() <= 0) {
    error_reporter_->ERROR() << "SetClipBoundary failed, width/height must both be positive "
                             << "(" << bounds.width() << ", " << bounds.height() << ")";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  clip_regions_[handle] = bounds;
}

void Flatland::ClearFlatland2State() {
  for (const auto& [layer_id, handle] : layer_handles_) {
    ReleaseLayerObject(handle);
  }
  layer_handles_.clear();
  // The stacks' transforms die with ResetGraph(); the dead-transform cleanup
  // drops their LayerStackData.
  layer_stack_handles_.clear();
  for (const auto& [image_id, global_id] : images2_) {
    ReleaseImageObject(global_id);
  }
  images2_.clear();
}

void Flatland::ProcessDeadTransforms(const TransformGraph::TopologyData& data) {
  for (const auto& dead_handle : data.dead_transforms) {
    matrices_.erase(dead_handle);

    auto it = layer_stacks_.find(dead_handle);
    if (it != layer_stacks_.end()) {
      for (const auto& layer_handle : it->second.layers) {
        ReleaseLayerObject(layer_handle);
      }
      layer_stacks_.erase(it);
    }
  }
}

void Flatland::AddChild(AddChildRequestView request, AddChildCompleter::Sync& completer) {
  AddChild(TransformId(request->parent_transform_id.value),
           TransformId(request->child_transform_id.value));
}

void Flatland::AddChild(TransformId parent_transform_id, TransformId child_transform_id) {
  if (parent_transform_id == kInvalidTransformId || child_transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "AddChild called with transform_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto parent_global_kv = transforms_.find(parent_transform_id);
  auto child_global_kv = transforms_.find(child_transform_id);

  if (parent_global_kv == transforms_.end()) {
    error_reporter_->ERROR() << "AddChild failed, parent_transform_id " << parent_transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (child_global_kv == transforms_.end()) {
    error_reporter_->ERROR() << "AddChild failed, child_transform_id " << child_transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  bool added = transform_graph_.AddChild(parent_global_kv->second, child_global_kv->second);

  if (!added) {
    error_reporter_->ERROR() << "AddChild failed, connection already exists between parent "
                             << parent_transform_id << " and child " << child_transform_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
}

void Flatland::RemoveChild(RemoveChildRequestView request, RemoveChildCompleter::Sync& completer) {
  RemoveChild(TransformId(request->parent_transform_id.value),
              TransformId(request->child_transform_id.value));
}

void Flatland::RemoveChild(TransformId parent_transform_id, TransformId child_transform_id) {
  if (parent_transform_id == kInvalidTransformId || child_transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "RemoveChild failed, transform_id " << parent_transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto parent_global_kv = transforms_.find(parent_transform_id);
  auto child_global_kv = transforms_.find(child_transform_id);

  if (parent_global_kv == transforms_.end()) {
    error_reporter_->ERROR() << "RemoveChild failed, parent_transform_id " << parent_transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (child_global_kv == transforms_.end()) {
    error_reporter_->ERROR() << "RemoveChild failed, child_transform_id " << child_transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  bool removed = transform_graph_.RemoveChild(parent_global_kv->second, child_global_kv->second);

  if (!removed) {
    error_reporter_->ERROR() << "RemoveChild failed, connection between parent "
                             << parent_transform_id << " and child " << child_transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
}

void Flatland::ReplaceChildren(ReplaceChildrenRequestView request,
                               ReplaceChildrenCompleter::Sync& completer) {
  size_t count = request->new_child_transform_ids.size();
  if (count > fuchsia_ui_composition::kMaxChildTransforms) {
    error_reporter_->ERROR() << "ReplaceChildren failed, too many children: " << count;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  std::array<TransformId, fuchsia_ui_composition::kMaxChildTransforms> new_children;
  for (size_t i = 0; i < count; ++i) {
    new_children[i] = TransformId(request->new_child_transform_ids[i].value);
  }
  ReplaceChildren(TransformId(request->parent_transform_id.value),
                  std::span(new_children.data(), count));
}

void Flatland::ReplaceChildren(TransformId parent_transform_id,
                               std::span<const TransformId> new_child_transform_ids) {
  if (parent_transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "ReplaceChildren failed, parent transform_id "
                             << parent_transform_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto parent_global_kv = transforms_.find(parent_transform_id);
  if (parent_global_kv == transforms_.end()) {
    error_reporter_->ERROR() << "ReplaceChildren failed, parent transform_id "
                             << parent_transform_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  size_t count = new_child_transform_ids.size();
  if (count > fuchsia_ui_composition::kMaxChildTransforms) {
    error_reporter_->ERROR() << "ReplaceChildren failed, too many children: " << count;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  // Look up the TransformHandle corresponding to the client-visible TransformId.  Use a std::array
  // to avoid heap allocations; benchmarking showed this to be slightly faster (and significantly
  // lower-variance) for 3 children.  Additional microoptimization opportunity: remove the default
  // field initialization, so it doesn't default-initialize all `kMaxChildTransforms` elements.
  std::array<TransformHandle, fuchsia_ui_composition::kMaxChildTransforms> children;
  for (size_t i = 0; i < count; ++i) {
    auto child_transform_id = new_child_transform_ids[i];
    if (child_transform_id == kInvalidTransformId) {
      error_reporter_->ERROR() << "ReplaceChildren failed, child transform_id "
                               << child_transform_id << " not found";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }

    auto child_global_kv = transforms_.find(child_transform_id);
    if (child_global_kv == transforms_.end()) {
      error_reporter_->ERROR() << "ReplaceChildren failed, child transform_id "
                               << child_transform_id << " not found";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }
    children[i] = child_global_kv->second;
  }

  bool replaced =
      transform_graph_.ReplaceChildren(parent_global_kv->second, std::span(children.data(), count));
  if (!replaced) {
    error_reporter_->ERROR()
        << "ReplaceChildren failed, cannot add duplicate children to the same parent: "
        << parent_transform_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
}

void Flatland::SetRootTransform(SetRootTransformRequestView request,
                                SetRootTransformCompleter::Sync& completer) {
  SetRootTransform(TransformId(request->transform_id.value));
}

void Flatland::SetRootTransform(TransformId transform_id) {
  // SetRootTransform(0) is special -- it only clears the existing root transform.
  if (transform_id == kInvalidTransformId) {
    transform_graph_.ClearChildren(local_root_);
    return;
  }

  const auto global_kv = transforms_.find(transform_id);
  if (global_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetRootTransform failed, transform_id " << transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  transform_graph_.ClearChildren(local_root_);

  FLATLAND_VERBOSE_LOG << "Flatland::SetRootTransform() session_id=" << session_id_
                       << "  client_transform_id=" << transform_id
                       << "  transform=" << global_kv->second;

  bool added = transform_graph_.AddChild(local_root_, global_kv->second);
  FX_DCHECK(added);

  root_transform_ = global_kv->second;
}

void Flatland::CreateViewport(CreateViewportRequestView request,
                              CreateViewportCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Flatland::CreateViewport", "debug_name", TA_STRING(debug_name_.c_str()));

  CreateViewport(ContentId(request->viewport_id.value), std::move(request->token),
                 request->properties, std::move(request->child_view_watcher));
}

void Flatland::CreateViewport(
    ContentId viewport_id, fuchsia_ui_views::wire::ViewportCreationToken token,
    const fuchsia_ui_composition::wire::ViewportProperties& properties,
    fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> child_view_watcher) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "CreateViewport is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  // Attempting to link with an invalid token will never succeed, so its better to fail early and
  // immediately close the link connection.
  if (!token.value.is_valid()) {
    error_reporter_->ERROR() << "CreateViewport failed, ViewportCreationToken was invalid";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (!properties.has_logical_size()) {
    error_reporter_->ERROR()
        << "CreateViewport must be provided a ViewportProperties with a logical size";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (auto error = ValidateViewportProperties(properties)) {
    error_reporter_->ERROR() << "CreateViewport failed: " << *error;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  fuchsia_ui_composition::ViewportProperties natural_properties;
  natural_properties.logical_size(fuchsia_math::SizeU({
      .width = properties.logical_size().width,
      .height = properties.logical_size().height,
  }));
  if (properties.has_inset()) {
    natural_properties.inset(fuchsia_math::Inset({
        .top = properties.inset().top,
        .right = properties.inset().right,
        .bottom = properties.inset().bottom,
        .left = properties.inset().left,
    }));
  } else {
    natural_properties.inset(fuchsia_math::Inset({.top = 0, .right = 0, .bottom = 0, .left = 0}));
  }

  if (viewport_id == kInvalidContentId) {
    error_reporter_->ERROR() << "CreateViewport called with ContentId zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (content_handles_.contains(viewport_id)) {
    error_reporter_->ERROR() << "CreateViewport called with existing ContentId " << viewport_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  FX_DCHECK(link_system_);

  // The ViewportProperties and ChildViewWatcherImpl live on a handle from this Flatland instance.
  const auto parent_transform_handle = transform_graph_.CreateTransform();

  fuchsia_ui_views::ViewportCreationToken natural_token({.value = std::move(token.value)});

  // We can initialize the Link importer immediately, since no state changes actually occur before
  // the feed-forward portion of this method. We also forward the initial ViewportProperties
  // through the LinkSystem immediately, so the child can receive them as soon as possible.
  LinkSystem::LinkToChild link_to_child = link_system_->CreateLinkToChild(
      dispatcher_holder_, std::move(natural_token), natural_properties,
      std::move(child_view_watcher), parent_transform_handle,
      [ref = weak_from_this(), weak_dispatcher_holder = std::weak_ptr<utils::DispatcherHolder>(
                                   dispatcher_holder_)](const std::string& error_log) {
        if (auto dispatcher_holder = weak_dispatcher_holder.lock()) {
          FX_CHECK(dispatcher_holder->dispatcher() == async_get_default_dispatcher())
              << "Link protocol error reported on the wrong dispatcher.";
        }
        if (auto impl = ref.lock())
          impl->ReportLinkProtocolError(error_log);
      });

  // This is the feed-forward portion of the method. Here, we add the link to the map, and
  // initialize its layout with the desired properties. The Link will not actually result in
  // additions to the Transform hierarchy until it is added to a Transform.
  {
    const bool child_added = transform_graph_.AddChild(link_to_child.parent_transform_handle,
                                                       link_to_child.internal_link_handle);
    FX_DCHECK(child_added);
  }

  FLATLAND_VERBOSE_LOG << "Flatland::CreateViewport() session_id=" << session_id_
                       << "  viewport_id=" << viewport_id
                       << "  parent_transform=" << link_to_child.parent_transform_handle
                       << "  internal_link_handle=" << link_to_child.internal_link_handle;

  // Default the link size to the logical size, which is just an identity scale matrix, so
  // that future logical size changes will result in the correct scale matrix.
  const fuchsia_math::wire::SizeU size = properties.logical_size();

  content_handles_[viewport_id] = link_to_child.parent_transform_handle;
  links_to_children_[link_to_child.parent_transform_handle] = {
      .link = std::move(link_to_child), .properties = std::move(natural_properties)};

  // Set clip bounds on the transform associated with the viewport content.
  const int32_t width = static_cast<int32_t>(size.width);
  const int32_t height = static_cast<int32_t>(size.height);
  FX_DCHECK(width >= 0 && height >= 0)
      << "Integer overflow.  width=" << width << ", height=" << height;
  SetClipBoundaryInternal(parent_transform_handle,
                          TransformClipRegion({.x = 0, .y = 0, .width = width, .height = height}));
}

LayerObject* Flatland::GetFacadeLayerObject(TransformHandle content_handle) {
  auto stack_it = layer_stacks_.find(content_handle);
  if (stack_it == layer_stacks_.end()) {
    return nullptr;
  }
  FX_DCHECK(stack_it->second.layers.size() == 1);  // by construction, there will be exactly 1 layer
  if (stack_it->second.layers.empty()) {
    return nullptr;
  }
  auto layer_handle = stack_it->second.layers[0];
  auto layer_it = layer_objects_.find(layer_handle);
  if (layer_it == layer_objects_.end()) {
    return nullptr;
  }
  return &layer_it->second;
}

UberStructLayer::ImageModeProperties* Flatland::GetFacadeLayerImageContent(
    TransformHandle content_handle) {
  auto* layer = GetFacadeLayerObject(content_handle);
  if (!layer || layer->mode != LayerObject::Mode::kImage) {
    return nullptr;
  }
  return &layer->image_mode;
}

UberStructLayer::SolidColorModeProperties* Flatland::GetFacadeLayerSolidColorContent(
    TransformHandle content_handle) {
  auto* layer = GetFacadeLayerObject(content_handle);
  if (!layer || layer->mode != LayerObject::Mode::kSolidColor) {
    return nullptr;
  }
  return &layer->solid_color_mode;
}

// TODO(https://fxbug.dev/474444799): This is a stub; the only thing it is supposed to demonstrate
// is that it captures "illegal usage".
void Flatland::CreateViewport2(CreateViewport2RequestView request,
                               CreateViewport2Completer::Sync& completer) {
  CreateViewport2(ViewportId(request->viewport_id.value), std::move(request->token),
                  request->properties, std::move(request->child_view_watcher));
}

void Flatland::CreateViewport2(
    ViewportId viewport_id, fuchsia_ui_views::wire::ViewportCreationToken token,
    const fuchsia_ui_composition::wire::ViewportProperties& properties,
    fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> child_view_watcher) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "CreateViewport2 called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  error_reporter_->ERROR() << "CreateViewport2: NOT IMPLEMENTED";
  CloseConnection(FlatlandError::kBadOperation);
}

std::optional<allocation::ImageMetadata> Flatland::ImportImage(
    fuchsia_ui_composition::wire::BufferCollectionImportToken import_token, uint32_t vmo_index,
    const fuchsia_ui_composition::wire::ImageProperties& properties) {
  const BufferCollectionId global_collection_id = fsl::GetRelatedKoid(import_token.value.get());

  // Check if there is a valid peer.
  if (global_collection_id == ZX_KOID_INVALID) {
    error_reporter_->ERROR() << "ImportImage called with no valid export token";
    CloseConnection(FlatlandError::kBadOperation);
    return std::nullopt;
  }

  if (!properties.has_size()) {
    error_reporter_->ERROR() << "ImportImage failed, ImageProperties did not specify size";
    CloseConnection(FlatlandError::kBadOperation);
    return std::nullopt;
  }

  if (!properties.size().width) {
    error_reporter_->ERROR() << "ImportImage failed, ImageProperties did not specify a width";
    CloseConnection(FlatlandError::kBadOperation);
    return std::nullopt;
  }

  if (!properties.size().height) {
    error_reporter_->ERROR() << "ImportImage failed, ImageProperties did not specify a height";
    CloseConnection(FlatlandError::kBadOperation);
    return std::nullopt;
  }

  allocation::ImageMetadata metadata;
  metadata.identifier = allocation::GenerateUniqueImageId();
  metadata.collection_id = global_collection_id;
  metadata.vmo_index = vmo_index;
  metadata.width = properties.size().width;
  metadata.height = properties.size().height;

  import_tokens_->emplace(metadata.identifier, std::move(import_token));

  // This fence is used to bridge promise resolution (see below) to the existing `fence_queue_`
  // mechanism, to guarantee that the image has been created by the time the corresponding
  // `Present()`'s UberStruct is applied to the global scene graph.
  auto [create_image_fence, create_image_fence_dup] = CreateEventAndDup();
  pending_create_image_fences_.push_back(std::move(create_image_fence));

  std::vector<fpromise::promise<>> promises;
  promises.reserve(buffer_collection_importers_.size());
  for (auto& importer : buffer_collection_importers_) {
    auto promise =
        importer->ImportBufferImage(metadata, allocation::BufferCollectionUsage::kClientImage);
    promises.push_back(std::move(promise));
  }
  auto join_promise =
      fpromise::join_promise_vector(std::move(promises))
          .and_then([this, id = metadata.identifier, fence = std::move(create_image_fence_dup)](
                        std::vector<fpromise::result<>>& results) mutable -> fpromise::result<> {
            bool ok = std::ranges::all_of(results, [](auto& result) { return result.is_ok(); });

            if (ok) {
              // Signal the fence to unblock `Present()` via `fence_queue_` (see above).
              fence.signal(0, ZX_EVENT_SIGNALED);
              return fpromise::ok();
            }

            // Drop the reference(s).
            // Unbind from any layers that still reference this image.
            for (auto& [handle, layer] : layer_objects_) {
              if (layer.image_mode.image_id == id) {
                UnbindLayerImage(layer);
              }
            }
            // For Flatland2, if the client ImageId is still alive in images2_, erase it and
            // release its reference.
            for (auto it = images2_.begin(); it != images2_.end(); ++it) {
              if (it->second == id) {
                images2_.erase(it);
                ReleaseImageObject(id);
                break;
              }
            }
            FX_CHECK(!image_objects_.contains(id));

            error_reporter_->ERROR() << "Importer could not import image.";
            CloseConnection(FlatlandError::kBadOperation);
            return fpromise::error();
          });
  executor_.schedule_task(std::move(join_promise));
  return metadata;
}

void Flatland::CreateImage(CreateImageRequestView request, CreateImageCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Flatland::CreateImage", "debug_name", TA_STRING(debug_name_.c_str()));

  CreateImage(ContentId(request->image_id.value), std::move(request->import_token),
              request->vmo_index, request->properties);
}

void Flatland::CreateImage(ContentId image_id,
                           fuchsia_ui_composition::wire::BufferCollectionImportToken import_token,
                           uint32_t vmo_index,
                           const fuchsia_ui_composition::wire::ImageProperties& properties) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "CreateImage is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidContentId) {
    error_reporter_->ERROR() << "CreateImage called with image_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (content_handles_.contains(image_id)) {
    error_reporter_->ERROR() << "CreateImage called with pre-existing image_id " << image_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto metadata = ImportImage(std::move(import_token), vmo_index, properties);
  if (!metadata) {
    return;
  }

  LayerHandle layer_handle = CreateLayerObject();
  UberStructLayer::ImageModeProperties content{
      // Defaults are correct for all properties except for the sample rect and the image binding.
      // The latter is modified by `BindLayerImage()` below.
      .sample_rect = {{
          .x = 0.f,
          .y = 0.f,
          .width = static_cast<float>(metadata->width),
          .height = static_cast<float>(metadata->height),
      }},
  };
  auto& layer_object = layer_objects_[layer_handle];
  layer_object.mode = LayerObject::Mode::kImage;
  layer_object.image_mode = content;
  layer_object.common.display_rect = {{
      .x = 0,
      .y = 0,
      .width = static_cast<int32_t>(metadata->width),
      .height = static_cast<int32_t>(metadata->height),
  }};

  image_objects_.try_emplace(metadata->identifier,
                             ImageObject{.metadata = *metadata, .ref_count = 0});
  BindLayerImage(layer_object, metadata->identifier);  // Increments image ref-count.

  TransformHandle stack_handle = CreateLayerStackData();
  SetLayerStackData(stack_handle, {layer_handle});
  content_handles_[image_id] = stack_handle;

  FLATLAND_VERBOSE_LOG << "Flatland::CreateImage() session_id=" << session_id_
                       << "  image_id=" << image_id << "  size=" << metadata->width << "x"
                       << metadata->height << "  handle=" << stack_handle;
}

void Flatland::CreateImage2(CreateImage2RequestView request,
                            CreateImage2Completer::Sync& completer) {
  CreateImage2(ImageId(request->image_id.value), std::move(request->import_token),
               request->vmo_index, request->properties);
}

void Flatland::CreateImage2(ImageId image_id,
                            fuchsia_ui_composition::wire::BufferCollectionImportToken import_token,
                            uint32_t vmo_index,
                            const fuchsia_ui_composition::wire::ImageProperties& properties) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "CreateImage2 called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidImageId) {
    error_reporter_->ERROR() << "CreateImage2 called with image_id " << kInvalidImageId;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (images2_.contains(image_id)) {
    error_reporter_->ERROR() << "CreateImage2 called with pre-existing image_id " << image_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto metadata = ImportImage(std::move(import_token), vmo_index, properties);
  if (!metadata) {
    return;
  }

  images2_[image_id] = metadata->identifier;
  auto [it, success] = image_objects_.try_emplace(
      metadata->identifier, ImageObject{.metadata = *metadata, .ref_count = 1});
  FX_CHECK(success) << "image object already exists: " << metadata->identifier;

  FLATLAND_VERBOSE_LOG << "Flatland::CreateImage2() session_id=" << session_id_
                       << "  image_id=" << image_id << "  global_id=" << metadata->identifier
                       << "  size=" << metadata->width << "x" << metadata->height;
}

void Flatland::SetImageSampleRegion(SetImageSampleRegionRequestView request,
                                    SetImageSampleRegionCompleter::Sync& completer) {
  SetImageSampleRegion(ContentId(request->image_id.value), types::RectangleF::From(request->rect));
}

void Flatland::SetImageSampleRegion(ContentId image_id, types::RectangleF rect) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "SetImageSampleRegion is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidContentId) {
    error_reporter_->ERROR() << "SetImageSampleRegion called with content id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  const auto content_kv = content_handles_.find(image_id);
  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "SetImageSampleRegion called with non-existent image_id "
                             << image_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  float image_width = 0.f;
  float image_height = 0.f;

  auto* image_content = GetFacadeLayerImageContent(content_kv->second);
  if (!image_content) {
    error_reporter_->ERROR() << "SetImageSampleRegion called on non-image content.";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  image_width = static_cast<float>(image_content->image_width);
  image_height = static_cast<float>(image_content->image_height);

  // The provided sample region needs to be within the bounds of the image.
  {
    // This clamping is required in cases where (x+width>image_width) or (y+height>image_height)
    // by a small epsilon. The downstream code expects these numbers to be within the
    // (image_width, image_height) limits, so we only clamp the positive differences. The root
    // cause is the precision errors in floating point arithmetic when a client tries to calculate
    // floats within pixel space.
    // TODO(https://fxbug.dev/42082599): Remove floating point precision error checks and use
    // uints instead.
    float clamped_width = rect.width();
    float clamped_height = rect.height();
    ClampIfNear(&clamped_width, rect.x() + clamped_width - image_width);
    ClampIfNear(&clamped_height, rect.y() + clamped_height - image_height);
    if (rect.x() < 0.f || clamped_width < 0.f || (rect.x() + clamped_width) > image_width ||
        rect.y() < 0.f || clamped_height < 0.f || (rect.y() + clamped_height) > image_height) {
      error_reporter_->ERROR() << "SetImageSampleRegion rect " << rect.x() << "," << rect.y() << ","
                               << clamped_width << "," << clamped_height
                               << " out of bounds for image (" << image_width << ", "
                               << image_height << ")";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }
    rect = ImageSampleRegion(
        {.x = rect.x(), .y = rect.y(), .width = clamped_width, .height = clamped_height});
  }

  image_content->sample_rect = rect;
}

void Flatland::SetImageDestinationSize(SetImageDestinationSizeRequestView request,
                                       SetImageDestinationSizeCompleter::Sync& completer) {
  SetImageDestinationSize(ContentId(request->image_id.value), request->size);
}

void Flatland::SetImageDestinationSize(ContentId image_id, fuchsia_math::wire::SizeU size) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "SetImageDestinationSize is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidContentId) {
    error_reporter_->ERROR() << "SetImageDestinationSize called with image_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto content_kv = content_handles_.find(image_id);

  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "SetImageDestinationSize called with non-existent image_id "
                             << image_id.value();
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto* layer = GetFacadeLayerObject(content_kv->second);
  if (!layer || layer->mode != LayerObject::Mode::kImage) {
    error_reporter_->ERROR() << "SetImageDestinationSize called on non-image content  "
                             << image_id.value();
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  layer->common.display_rect = types::Rectangle({.x = 0,
                                                 .y = 0,
                                                 .width = static_cast<int32_t>(size.width),
                                                 .height = static_cast<int32_t>(size.height)});
}

void Flatland::SetImageBlendingFunction(SetImageBlendingFunctionRequestView request,
                                        SetImageBlendingFunctionCompleter::Sync& completer) {
  SetImageBlendMode(ContentId(request->image_id.value), BlendMode::From(request->blend_mode));
}

void Flatland::SetImageBlendMode(SetImageBlendModeRequestView request,
                                 SetImageBlendModeCompleter::Sync& completer) {
  // Typically we do checks in the "real implementation" of the method, i.e. the one
  // that takes internal Scenic types rather than FIDL types.  However in this case we
  // need to validate before calling `BlendMode::From()`.
  if (request->blend_mode.IsUnknown()) {
    error_reporter_->ERROR() << "SetImageBlendMode: unknown blend mode";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  SetImageBlendMode(ContentId(request->image_id.value), BlendMode::From(request->blend_mode));
}

void Flatland::SetImageBlendMode(ContentId image_id, BlendMode blend_mode) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "SetImageBlendMode is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidContentId) {
    error_reporter_->ERROR() << "SetImageBlendMode called with content id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto content_kv = content_handles_.find(image_id);
  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "SetImageBlendMode called with non-existent image_id " << image_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto* layer = GetFacadeLayerObject(content_kv->second);
  if (!layer) {
    error_reporter_->ERROR() << "SetImageBlendMode called on non-existent content.";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  FX_CHECK(layer->mode != LayerObject::Mode::kInvisible);
  layer->common.blend_mode = blend_mode;
}

void Flatland::SetImageFlip(SetImageFlipRequestView request,
                            SetImageFlipCompleter::Sync& completer) {
  SetImageFlip(ContentId(request->image_id.value), request->flip);
}

void Flatland::SetImageFlip(ContentId image_id, fuchsia_ui_composition::ImageFlip flip) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "SetImageFlip is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidContentId) {
    error_reporter_->ERROR() << "SetImageFlip called with content id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto content_kv = content_handles_.find(image_id);
  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "SetImageFlip called with non-existent image_id " << image_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto* image_content = GetFacadeLayerImageContent(content_kv->second);
  if (!image_content) {
    error_reporter_->ERROR() << "SetImageFlip called on non-image content.";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  // In Flatland1 there is no per-image orientation,
  // so we compose the flip with a 0-degree rotation.
  image_content->transform =
      types::RotateFlip::From(fuchsia_ui_composition::Orientation::kCcw0Degrees, flip);
}

void Flatland::CreateFilledRect(CreateFilledRectRequestView request,
                                CreateFilledRectCompleter::Sync& completer) {
  CreateFilledRect(ContentId(request->rect_id.value));
}

void Flatland::CreateFilledRect(ContentId rect_id) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "CreateFilledRect is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (rect_id == kInvalidContentId) {
    error_reporter_->ERROR() << "CreateFilledRect called with rect_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (content_handles_.contains(rect_id)) {
    error_reporter_->ERROR() << "CreateFilledRect called with pre-existing content id " << rect_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  allocation::ImageMetadata metadata;
  // allocation::kInvalidImageId is overloaded in the renderer to signal that a
  // default 1x1 white texture should be applied to this rectangle.
  metadata.identifier = allocation::kInvalidImageId;

  TransformHandle handle;  // Lifted from if/else for FLATLAND_VERBOSE_LOG below.

  LayerHandle layer_handle = CreateLayerObject();
  UberStructLayer::SolidColorModeProperties content{
      // Set default color to opaque white (matches Flatland1 default multiply_color behavior
      // before SetSolidFill is called, though it's typically set immediately after).
      .color = {1.f, 1.f, 1.f, 1.f},
  };
  auto& layer_object = layer_objects_[layer_handle];
  layer_object.mode = LayerObject::Mode::kSolidColor;
  layer_object.solid_color_mode = content;
  layer_object.common.display_rect = {{.x = 0, .y = 0, .width = 0, .height = 0}};

  handle = CreateLayerStackData();
  SetLayerStackData(handle, {layer_handle});
  content_handles_[rect_id] = handle;

  FLATLAND_VERBOSE_LOG << "Flatland::CreateFilledRect() session_id=" << session_id_
                       << "  rect_id=" << rect_id << "  handle=" << handle;
}

void Flatland::SetSolidFill(SetSolidFillRequestView request,
                            SetSolidFillCompleter::Sync& completer) {
  SetSolidFill(ContentId(request->rect_id.value), request->color, request->size);
}

void Flatland::SetSolidFill(ContentId rect_id, fuchsia_ui_composition::wire::ColorRgba color,
                            fuchsia_math::wire::SizeU size) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "SetSolidFill is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (rect_id == kInvalidContentId) {
    error_reporter_->ERROR() << "SetSolidFill called with rect_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto content_kv = content_handles_.find(rect_id);

  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "SetSolidFill called with non-existent rect_id " << rect_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (color.red < 0.f || color.red > 1.f || isinf(color.red) || isnan(color.red) ||
      color.green < 0.f || color.green > 1.f || isinf(color.green) || isnan(color.green) ||
      color.blue < 0.f || color.blue > 1.f || isinf(color.blue) || isnan(color.blue) ||
      color.alpha < 0.f || color.alpha > 1.f || isinf(color.alpha) || isnan(color.alpha)) {
    error_reporter_->ERROR() << "Invalid color channel(s) (" << color.red << ", " << color.green
                             << ", " << color.blue << ", " << color.alpha << ")";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  FLATLAND_VERBOSE_LOG << "Flatland::SetSolidFill() session_id=" << session_id_
                       << "  rect_id=" << rect_id << "  handle=" << content_kv->second
                       << "  rgba=" << color.red << "," << color.green << "," << color.blue << ","
                       << color.alpha << "  size=" << size.width << "x" << size.height;

  auto* layer = GetFacadeLayerObject(content_kv->second);
  if (!layer || layer->mode != LayerObject::Mode::kSolidColor) {
    error_reporter_->ERROR() << "Missing metadata for rect with id  " << rect_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  auto& solid_color = layer->solid_color_mode;
  solid_color.color = std::array<float, 4>{color.red, color.green, color.blue, color.alpha};
  layer->common.display_rect = types::Rectangle({
      .x = 0,
      .y = 0,
      .width = static_cast<int32_t>(size.width),
      .height = static_cast<int32_t>(size.height),
  });
  // Derive the blend mode from the fill alpha: opaque fills get REPLACE,
  // translucent fills get PREMULTIPLIED_ALPHA. The derivation runs on
  // every SetSolidFill call, so it overwrites any blend mode set earlier;
  // in the other order, a later SetImageBlendMode overwrites the derived
  // value. Last call wins, matching classic Flatland1 (the CTF pixel tests
  // rely on the fill-then-blend order).
  layer->common.blend_mode =
      color.alpha < 1.f ? types::BlendMode::kPremultipliedAlpha() : types::BlendMode::kReplace();
}

void Flatland::ReleaseFilledRect(ReleaseFilledRectRequestView request,
                                 ReleaseFilledRectCompleter::Sync& completer) {
  ReleaseFilledRect(ContentId(request->rect_id.value));
}

void Flatland::ReleaseFilledRect(ContentId rect_id) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "ReleaseFilledRect is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (rect_id == kInvalidContentId) {
    error_reporter_->ERROR() << "ReleaseFilledRect called with rect_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto content_kv = content_handles_.find(rect_id);

  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "ReleaseFilledRect failed, rect_id " << rect_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto* solid_color = GetFacadeLayerSolidColorContent(content_kv->second);
  if (!solid_color) {
    error_reporter_->ERROR() << "ReleaseFilledRect failed, content_id " << rect_id
                             << " has no metadata.";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  bool erased_from_graph = transform_graph_.ReleaseTransform(content_kv->second);
  FX_DCHECK(erased_from_graph);

  // Even though the handle is released, it may still be referenced by client Transforms. The
  // layer_stacks_ and layer_objects_ maps preserve the entry until it shows up in the
  // dead_transforms list.
  content_handles_.erase(rect_id);
}

void Flatland::SetImageOpacity(SetImageOpacityRequestView request,
                               SetImageOpacityCompleter::Sync& completer) {
  SetImageOpacity(ContentId(request->image_id.value), request->val);
}

void Flatland::SetImageOpacity(ContentId image_id, float opacity) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "SetImageOpacity is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidContentId) {
    error_reporter_->ERROR() << "SetImageOpacity called with invalid image_id";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto content_kv = content_handles_.find(image_id);
  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "SetImageOpacity called with non-existent image_id " << image_id;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (opacity < 0.f || opacity > 1.f) {
    error_reporter_->ERROR() << "Opacity value is not within valid range [0,1].";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto* layer = GetFacadeLayerObject(content_kv->second);
  if (!layer || layer->mode != LayerObject::Mode::kImage) {
    error_reporter_->ERROR() << "SetImageOpacity called on non-image content.";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  layer->common.opacity = opacity;
}

void Flatland::SetHitRegions(SetHitRegionsRequestView request,
                             SetHitRegionsCompleter::Sync& completer) {
  SetHitRegions(TransformId(request->transform_id.value),
                std::span(request->regions.data(), request->regions.size()));
}

void Flatland::SetHitRegions(TransformId transform_id,
                             std::span<const fuchsia_ui_composition::wire::HitRegion> regions) {
  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetHitRegions called with invalid transform ID";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);
  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetHitRegions failed, transform_id " << transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  // Validate |regions|.
  for (const auto& region : regions) {
    const auto& rect = region.region;

    if (!types::RectangleF::IsValid(rect)) {
      error_reporter_->ERROR() << "SetHitRegions failed, contains invalid dimensions: ("
                               << rect.width << "," << rect.height << ")";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }
  }

  // Reformat into internal type.
  std::vector<flatland::HitRegion> list;
  list.reserve(regions.size());
  for (const auto& region : regions) {
    list.emplace_back(types::RectangleF::From(region.region), region.hit_test);
  }
  hit_regions_[transform_kv->second] = std::move(list);
}

void Flatland::SetInfiniteHitRegion(SetInfiniteHitRegionRequestView request,
                                    SetInfiniteHitRegionCompleter::Sync& completer) {
  SetInfiniteHitRegion(TransformId(request->transform_id.value), request->hit_test);
}

void Flatland::SetInfiniteHitRegion(TransformId transform_id,
                                    fuchsia_ui_composition::HitTestInteraction hit_test) {
  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetHitRegions called with invalid transform ID";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);
  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetHitRegions failed, transform_id " << transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  hit_regions_[transform_kv->second] = {flatland::HitRegion::Infinite(hit_test)};
}

void Flatland::SetContent(SetContentRequestView request, SetContentCompleter::Sync& completer) {
  SetContent(TransformId(request->transform_id.value), ContentId(request->content_id.value));
}

void Flatland::SetContent(TransformId transform_id, ContentId content_id) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "SetContent is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetContent called with transform_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);

  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetContent failed, transform_id " << transform_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (content_id == kInvalidContentId) {
    transform_graph_.ClearPriorityChild(transform_kv->second);
    FLATLAND_VERBOSE_LOG << "Flatland::SetContent() session_id=" << session_id_
                         << "  client_transform_id=" << transform_id
                         << "  transform=" << transform_kv->second << "  ... cleared content.";
    return;
  }

  auto content_kv = content_handles_.find(content_id);

  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "SetContent failed, content_id " << content_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  FLATLAND_VERBOSE_LOG << "Flatland::SetContent() session_id=" << session_id_
                       << "  client_transform_id=" << transform_id
                       << "  transform=" << transform_kv->second
                       << "  client_content_id=" << content_id
                       << "  content=" << content_kv->second;

  transform_graph_.SetPriorityChild(transform_kv->second, content_kv->second);
}

void Flatland::SetTransformContent(SetTransformContentRequestView request,
                                   SetTransformContentCompleter::Sync& completer) {
  SetTransformContent(TransformId(request->transform_id.value),
                      request->content.has_value() ? &request->content.value() : nullptr);
}

void Flatland::SetTransformContent(TransformId transform_id,
                                   const fuchsia_ui_composition::wire::TransformContent* content) {
  if (!content) {
    ClearTransformContent(transform_id);
    return;
  }

  switch (content->Which()) {
    case fuchsia_ui_composition::wire::TransformContent::Tag::kLayerStack: {
      SetTransformContent(transform_id, LayerStackId(content->layer_stack().value));
      break;
    }
    case fuchsia_ui_composition::wire::TransformContent::Tag::kViewport: {
      SetTransformContent(transform_id, ViewportId(content->viewport().value));
      break;
    }
    default: {
      error_reporter_->ERROR() << "SetTransformContent: unknown content type";
      CloseConnection(FlatlandError::kBadOperation);
    }
  }
}

void Flatland::SetTransformContent(TransformId transform_id, LayerStackId layer_stack_id) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "SetTransformContent called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetTransformContent called with transform_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);
  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetTransformContent: transform " << transform_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (layer_stack_id == kInvalidLayerStackId) {
    error_reporter_->ERROR()
        << "SetTransformContent: LayerStackId must be non-zero (to clear content, omit "
           "`content`)";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto stack_it = layer_stack_handles_.find(layer_stack_id);
  if (stack_it == layer_stack_handles_.end()) {
    error_reporter_->ERROR() << "SetTransformContent failed, layer_stack_id "
                             << layer_stack_id.value() << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  FLATLAND_VERBOSE_LOG << "Flatland::SetTransformContent() session_id=" << session_id_
                       << "  client_transform_id=" << transform_id
                       << "  transform=" << transform_kv->second
                       << "  client_layer_stack_id=" << layer_stack_id.value()
                       << "  content=" << stack_it->second;

  transform_graph_.SetPriorityChild(transform_kv->second, stack_it->second);
}

void Flatland::SetTransformContent(TransformId transform_id, ViewportId viewport_id) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "SetTransformContent called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetTransformContent called with transform_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);
  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetTransformContent: transform " << transform_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (viewport_id == kInvalidViewportId) {
    error_reporter_->ERROR()
        << "SetTransformContent: ViewportId must be non-zero (to clear content, omit "
           "`content`)";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  error_reporter_->ERROR() << "SetTransformContent(viewport) not yet implemented";
  CloseConnection(FlatlandError::kBadOperation);
}

void Flatland::ClearTransformContent(TransformId transform_id) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "SetTransformContent called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "SetTransformContent called with transform_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);
  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "SetTransformContent: transform " << transform_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  transform_graph_.ClearPriorityChild(transform_kv->second);
  FLATLAND_VERBOSE_LOG << "Flatland::SetTransformContent() session_id=" << session_id_
                       << "  client_transform_id=" << transform_id << " ... cleared content.";
}

void Flatland::SetViewportProperties(SetViewportPropertiesRequestView request,
                                     SetViewportPropertiesCompleter::Sync& completer) {
  SetViewportProperties(ContentId(request->viewport_id.value), request->properties);
}

void Flatland::SetViewportProperties(
    ContentId viewport_id, const fuchsia_ui_composition::wire::ViewportProperties& properties) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "SetViewportProperties is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (viewport_id == kInvalidContentId) {
    error_reporter_->ERROR() << "SetViewportProperties called with link_id zero.";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  const auto content_kv = content_handles_.find(viewport_id);
  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "SetViewportProperties failed, link_id " << viewport_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  const auto viewport_handle = content_kv->second;

  auto link_kv = links_to_children_.find(viewport_handle);
  if (link_kv == links_to_children_.end()) {
    error_reporter_->ERROR() << "SetViewportProperties failed, content_id " << viewport_id
                             << " is not a Link";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  LinkToChildData& link_data = link_kv->second;
  if (!link_data.link.importer.valid()) {
    // Other side of the Viewport has been invalidated and the Viewport should be released.
    // Calling SetViewportProperties() must still be allowed since the client may not have gotten
    // the destruction message yet.
    return;
  }

  if (auto error = ValidateViewportProperties(properties)) {
    error_reporter_->ERROR() << "SetViewportProperties failed: " << *error;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  FX_DCHECK(link_data.properties.logical_size().has_value());
  FX_DCHECK(link_data.properties.inset().has_value());
  if (properties.has_logical_size()) {
    link_data.properties.logical_size(fuchsia_math::SizeU({
        .width = properties.logical_size().width,
        .height = properties.logical_size().height,
    }));
  }
  if (properties.has_inset()) {
    link_data.properties.inset(fuchsia_math::Inset({
        .top = properties.inset().top,
        .right = properties.inset().right,
        .bottom = properties.inset().bottom,
        .left = properties.inset().left,
    }));
  }

  // Update the clip boundaries when the properties change.
  const int32_t width = static_cast<int32_t>(link_data.properties.logical_size()->width());
  const int32_t height = static_cast<int32_t>(link_data.properties.logical_size()->height());
  FX_DCHECK(width >= 0 && height >= 0)
      << "Integer overflow.  width=" << width << ", height=" << height;
  SetClipBoundaryInternal(viewport_handle,
                          TransformClipRegion({.x = 0, .y = 0, .width = width, .height = height}));

  link_system_->UpdateViewportPropertiesFor(viewport_handle, link_data.properties);
}

void Flatland::SetViewportProperties2(SetViewportProperties2RequestView request,
                                      SetViewportProperties2Completer::Sync& completer) {
  SetViewportProperties2(ViewportId(request->viewport_id.value), request->properties);
}

void Flatland::SetViewportProperties2(
    ViewportId viewport_id, const fuchsia_ui_composition::wire::ViewportProperties& properties) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "SetViewportProperties2 called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  error_reporter_->ERROR() << "SetViewportProperties2: NOT IMPLEMENTED";
  CloseConnection(FlatlandError::kBadOperation);
}

void Flatland::ReleaseTransform(ReleaseTransformRequestView request,
                                ReleaseTransformCompleter::Sync& completer) {
  ReleaseTransform(TransformId(request->transform_id.value));
}

void Flatland::ReleaseTransform(TransformId transform_id) {
  if (transform_id == kInvalidTransformId) {
    error_reporter_->ERROR() << "ReleaseTransform called with transform_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto transform_kv = transforms_.find(transform_id);

  if (transform_kv == transforms_.end()) {
    error_reporter_->ERROR() << "ReleaseTransform failed, transform_id " << transform_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  bool erased_from_graph = transform_graph_.ReleaseTransform(transform_kv->second);
  FX_DCHECK(erased_from_graph);
  transforms_.erase(transform_kv);
}

void Flatland::ReleaseViewport(ReleaseViewportRequestView request,
                               ReleaseViewportCompleter::Sync& completer) {
  ReleaseViewport(ContentId(request->viewport_id.value),
                  [completer = completer.ToAsync()](
                      fuchsia_ui_views::wire::ViewportCreationToken token) mutable {
                    completer.Reply(std::move(token));
                  });
}

void Flatland::ReleaseViewport(
    ContentId viewport_id,
    fit::function<void(fuchsia_ui_views::wire::ViewportCreationToken)> completer) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "ReleaseViewport is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (viewport_id == kInvalidContentId) {
    error_reporter_->ERROR() << "ReleaseViewport called with link_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto content_kv = content_handles_.find(viewport_id);

  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "ReleaseViewport failed, link_id " << viewport_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto link_kv = links_to_children_.find(content_kv->second);

  if (link_kv == links_to_children_.end()) {
    error_reporter_->ERROR() << "ReleaseViewport failed, content_id " << viewport_id
                             << " is not a Link";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  LinkToChildData& link_data = link_kv->second;

  // Deleting the LinkToChild's |parent_transform_handle| effectively deletes the link from
  // the local topology, even if the link object itself is not deleted.
  {
    const bool child_removed = transform_graph_.RemoveChild(link_data.link.parent_transform_handle,
                                                            link_data.link.internal_link_handle);
    FX_DCHECK(child_removed);
    const bool content_released =
        transform_graph_.ReleaseTransform(link_data.link.parent_transform_handle);
    FX_DCHECK(content_released);
  }

  // Move the old child link into the delayed operation so that the ContentId is immediately free
  // for re-use, but it doesn't get deleted until after the new UberStruct is published.
  auto link_to_child = std::move(link_data);
  links_to_children_.erase(content_kv->second);
  content_handles_.erase(content_kv);

  // Delay the actual destruction of the link until the next Present().
  pending_link_operations_.push_back(
      [link_to_child = std::move(link_to_child), completer = std::move(completer)]() mutable {
        fuchsia_ui_views::wire::ViewportCreationToken return_token;

        // If the link is still valid, return the original token. If not, create an orphaned
        // zx::channel and return it since the ObjectLinker does not retain the orphaned token.
        auto link_token = link_to_child.link.importer.ReleaseToken();
        if (link_token.has_value()) {
          return_token.value = zx::channel(std::move(link_token.value()));
        } else {
          // |peer_token| immediately falls out of scope, orphaning |return_token|.
          zx::channel peer_token;
          zx::channel::create(0, &return_token.value, &peer_token);
        }

        completer(std::move(return_token));
      });
}

void Flatland::ReleaseViewport2(ReleaseViewport2RequestView request,
                                ReleaseViewport2Completer::Sync& completer) {
  ReleaseViewport2(ViewportId(request->viewport_id.value),
                   [completer = completer.ToAsync()](
                       fuchsia_ui_views::wire::ViewportCreationToken token) mutable {
                     completer.Reply(std::move(token));
                   });
}

void Flatland::ReleaseViewport2(
    ViewportId viewport_id,
    fit::function<void(fuchsia_ui_views::wire::ViewportCreationToken)> completer) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "ReleaseViewport2 called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  error_reporter_->ERROR() << "ReleaseViewport2: NOT IMPLEMENTED";
  CloseConnection(FlatlandError::kBadOperation);
}

void Flatland::ReleaseImage(ReleaseImageRequestView request,
                            ReleaseImageCompleter::Sync& completer) {
  ReleaseImage(ContentId(request->image_id.value));
}

void Flatland::ReleaseImage(ContentId image_id) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "ReleaseImage is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidContentId) {
    error_reporter_->ERROR() << "ReleaseImage called with image_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto content_kv = content_handles_.find(image_id);

  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "ReleaseImage failed, image_id " << image_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto* image_content = GetFacadeLayerImageContent(content_kv->second);
  if (!image_content) {
    error_reporter_->ERROR() << "ReleaseImage failed, content_id " << image_id
                             << " is not an Image";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  FLATLAND_VERBOSE_LOG << "Flatland::ReleaseImage() session_id=" << session_id_
                       << "  client_image_id=" << image_id
                       << "  image_handle=" << content_kv->second;

  bool erased_from_graph = transform_graph_.ReleaseTransform(content_kv->second);
  FX_DCHECK(erased_from_graph);

  // Even though the handle is released, it may still be referenced by client Transforms. The
  // layer_stacks_ and layer_objects_ maps preserve the entry until it shows up in the
  // dead_transforms list.
  content_handles_.erase(image_id);
}

void Flatland::ReleaseImage2(ReleaseImage2RequestView request,
                             ReleaseImage2Completer::Sync& completer) {
  ReleaseImage2(ImageId(request->image_id.value));
}

void Flatland::ReleaseImage2(ImageId image_id) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "ReleaseImage2 called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidImageId) {
    error_reporter_->ERROR() << "ReleaseImage2 called with image_id " << kInvalidImageId;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto it = images2_.find(image_id);
  if (it == images2_.end()) {
    error_reporter_->ERROR() << "ReleaseImage2 failed, image_id " << image_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  const allocation::GlobalImageId global_id = it->second;
  images2_.erase(it);
  ReleaseImageObject(global_id);

  FLATLAND_VERBOSE_LOG << "Flatland::ReleaseImage2() session_id=" << session_id_
                       << "  client_image_id=" << image_id << "  global_image_id=" << global_id;
}

void Flatland::SetDebugName(SetDebugNameRequestView request,
                            SetDebugNameCompleter::Sync& completer) {
  std::string name(request->name.get());

  TRACE_INSTANT("gfx", "Flatland::SetDebugName()", TRACE_SCOPE_PROCESS, "name",
                TA_STRING(name.c_str()));

  SetDebugName(std::move(name));
}

void Flatland::SetDebugName(std::string name) {
  std::stringstream stream;
  if (!name.empty())
    stream << "Flatland client(" << name << "): ";

  FX_LOGS(INFO) << "Flatland::SetDebugName() session_id=" << session_id_ << "  name: " << name;

  error_reporter_->SetPrefix(stream.str());
  debug_name_ = std::move(name);
}

void Flatland::ReleaseImageImmediately(ReleaseImageImmediatelyRequestView request,
                                       ReleaseImageImmediatelyCompleter::Sync& completer) {
  ReleaseImageImmediately(ContentId(request->image_id.value));
}

void Flatland::ReleaseImageImmediately(ContentId image_id) {
  if (config_.use_flatland2) {
    error_reporter_->ERROR() << "ReleaseImageImmediately is illegal because Flatland2 is enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (!config_.use_trusted_flatland_api) {
    error_reporter_->ERROR()
        << "ReleaseImageImmediately called on a Flatland instance not created via "
           "TrustedFlatlandFactory.";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (image_id == kInvalidContentId) {
    error_reporter_->ERROR() << "ReleaseImageImmediately called with image_id 0";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto content_kv = content_handles_.find(image_id);

  if (content_kv == content_handles_.end()) {
    error_reporter_->ERROR() << "ReleaseImageImmediately failed, image_id " << image_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  allocation::GlobalImageId identifier = allocation::kInvalidImageId;

  auto* image_content = GetFacadeLayerImageContent(content_kv->second);
  if (!image_content) {
    error_reporter_->ERROR() << "ReleaseImageImmediately failed, content_id " << image_id
                             << " is not an Image";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  identifier = image_content->image_id;
  image_content->image_id = allocation::kInvalidImageId;  // revert to invisible
  image_content->image_width = 0;
  image_content->image_height = 0;

  FLATLAND_VERBOSE_LOG << "Flatland::ReleaseImageImmediately() session_id=" << session_id_
                       << "  client_image_id=" << image_id
                       << "  image_handle=" << content_kv->second;

  bool erased_from_graph = transform_graph_.ReleaseTransform(content_kv->second);
  FX_DCHECK(erased_from_graph);
  content_handles_.erase(image_id);

  // Immediate release bypasses the ImageObject ref count on purpose: this is
  // the trusted, synchronous path, and on the Flatland1 facade the binding
  // cleared above is the image's only ref.
  {
    auto image_it = image_objects_.find(identifier);
    FX_CHECK(image_it != image_objects_.end() && image_it->second.ref_count == 1);
    image_objects_.erase(image_it);
  }

  // Release the image from all importers immediately.
  for (auto& importer : buffer_collection_importers_) {
    importer->ReleaseBufferImage(identifier);
  }
}

void Flatland::ReleaseImageImmediately2(ReleaseImageImmediately2RequestView request,
                                        ReleaseImageImmediately2Completer::Sync& completer) {
  ReleaseImageImmediately2(ImageId(request->image_id.value));
}

// TODO(https://fxbug.dev/474444799): This is a stub; the only thing it is supposed to demonstrate
// is that it captures "illegal usage".  See TODO in CreateLayer.
void Flatland::ReleaseImageImmediately2(ImageId image_id) {
  if (!config_.use_trusted_flatland_api) {
    error_reporter_->ERROR()
        << "ReleaseImageImmediately2 called on a Flatland instance not created via "
           "TrustedFlatlandFactory.";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "ReleaseImageImmediately2 called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  error_reporter_->ERROR() << "ReleaseImageImmediately2: NOT IMPLEMENTED";
  CloseConnection(FlatlandError::kBadOperation);
}

void Flatland::CreateLayer(CreateLayerRequestView request, CreateLayerCompleter::Sync& completer) {
  CreateLayer(LayerId(request->layer_id.value));
}

void Flatland::CreateLayer(LayerId layer_id) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "CreateLayer called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (layer_id == kInvalidLayerId) {
    error_reporter_->ERROR() << "CreateLayer: layer id 0 is invalid";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (layer_handles_.contains(layer_id)) {
    error_reporter_->ERROR() << "CreateLayer: layer " << layer_id << " already exists";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  LayerHandle handle = CreateLayerObject();
  // TODO(https://fxbug.dev/474444799): ideally wouldn't need to reach back into the map to add ref.
  layer_objects_[handle].ref_count++;
  layer_handles_[layer_id] = handle;
}

void Flatland::ReleaseLayer(ReleaseLayerRequestView request,
                            ReleaseLayerCompleter::Sync& completer) {
  ReleaseLayer(LayerId(request->layer_id.value));
}

void Flatland::ReleaseLayer(LayerId layer_id) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "ReleaseLayer called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto it = layer_handles_.find(layer_id);
  if (it == layer_handles_.end()) {
    error_reporter_->ERROR() << "ReleaseLayer: layer " << layer_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  const LayerHandle handle = it->second;
  layer_handles_.erase(it);

  ReleaseLayerObject(handle);
}

void Flatland::CreateLayerStack(CreateLayerStackRequestView request,
                                CreateLayerStackCompleter::Sync& completer) {
  CreateLayerStack(LayerStackId(request->stack_id.value));
}

void Flatland::CreateLayerStack(LayerStackId layer_stack_id) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "CreateLayerStack called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (layer_stack_id == kInvalidLayerStackId) {
    error_reporter_->ERROR() << "CreateLayerStack called with id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (layer_stack_handles_.contains(layer_stack_id)) {
    error_reporter_->ERROR() << "CreateLayerStack: layer stack " << layer_stack_id
                             << " already exists";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  TransformHandle handle = CreateLayerStackData();
  layer_stack_handles_[layer_stack_id] = handle;
}

void Flatland::ReleaseLayerStack(ReleaseLayerStackRequestView request,
                                 ReleaseLayerStackCompleter::Sync& completer) {
  ReleaseLayerStack(LayerStackId(request->stack_id.value));
}

void Flatland::ReleaseLayerStack(LayerStackId layer_stack_id) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "ReleaseLayerStack called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (layer_stack_id == kInvalidLayerStackId) {
    error_reporter_->ERROR() << "ReleaseLayerStack called with layer_stack_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto stack_it = layer_stack_handles_.find(layer_stack_id);
  if (stack_it == layer_stack_handles_.end()) {
    error_reporter_->ERROR() << "ReleaseLayerStack: layer stack " << layer_stack_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  TransformHandle handle = stack_it->second;
  layer_stack_handles_.erase(stack_it);

  bool erased_from_graph = transform_graph_.ReleaseTransform(handle);
  FX_DCHECK(erased_from_graph);
}

void Flatland::SetStackLayers(SetStackLayersRequestView request,
                              SetStackLayersCompleter::Sync& completer) {
  const size_t num_layers = request->layers.size();
  if (num_layers > fuchsia_ui_composition::kMaxStackLayers) {
    error_reporter_->ERROR() << "SetStackLayers: too many layers: " << num_layers;
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  std::array<LayerId, fuchsia_ui_composition::kMaxStackLayers> stack_layers;
  for (size_t i = 0; i < num_layers; ++i) {
    stack_layers[i] = LayerId(request->layers[i].value);
  }
  SetStackLayers(LayerStackId(request->stack_id.value),
                 std::span<const LayerId>(stack_layers.data(), num_layers));
}

void Flatland::SetStackLayers(LayerStackId layer_stack_id,
                              std::span<const flatland::LayerId> layers) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "SetStackLayers called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  if (layers.size() > fuchsia_ui_composition::kMaxStackLayers) {
    error_reporter_->ERROR() << "SetStackLayers: too many layers: " << layers.size();
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  if (layer_stack_id == kInvalidLayerStackId) {
    error_reporter_->ERROR() << "SetStackLayers called with layer_stack_id zero";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto stack_it = layer_stack_handles_.find(layer_stack_id);
  if (stack_it == layer_stack_handles_.end()) {
    error_reporter_->ERROR() << "SetStackLayers failed, layer_stack_id " << layer_stack_id
                             << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  // Stack array and linear scan avoid all dynamic allocations (heap/PMR) and outperform
  // hash tables for small, bounded inputs (N <= kMaxStackLayers = 32).
  std::array<LayerHandle, fuchsia_ui_composition::kMaxStackLayers> new_layer_handles;
  for (size_t i = 0; i < layers.size(); ++i) {
    const auto& layer_id = layers[i];
    if (layer_id == kInvalidLayerId) {
      error_reporter_->ERROR() << "SetStackLayers failed, layer_id is zero";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }

    if (std::find(layers.begin(), layers.begin() + i, layer_id) != layers.begin() + i) {
      error_reporter_->ERROR() << "SetStackLayers failed, duplicate layer_id " << layer_id
                               << " in vector";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }

    auto layer_it = layer_handles_.find(layer_id);
    if (layer_it == layer_handles_.end()) {
      error_reporter_->ERROR() << "SetStackLayers failed, layer_id " << layer_id << " not found";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }

    new_layer_handles[i] = layer_it->second;
  }

  SetLayerStackData(stack_it->second,
                    std::span<const LayerHandle>(new_layer_handles.data(), layers.size()));
}

void Flatland::SetLayerImage(SetLayerImageRequestView request,
                             SetLayerImageCompleter::Sync& completer) {
  SetLayerImage(LayerId(request->layer_id.value), ImageId(request->image_id.value),
                request->acquire_fence.has_value()
                    ? std::make_optional(std::move(request->acquire_fence.value()))
                    : std::nullopt,
                request->release_fence.has_value()
                    ? std::make_optional(std::move(request->release_fence.value()))
                    : std::nullopt);
}

void Flatland::SetLayerImage(
    LayerId layer_id, ImageId image_id,
    std::optional<fuchsia_ui_composition::wire::WaitFence> acquire_fence,
    std::optional<fuchsia_ui_composition::wire::SignalFence> release_fence) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "SetLayerImage called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto it = layer_handles_.find(layer_id);
  if (it == layer_handles_.end()) {
    error_reporter_->ERROR() << "SetLayerImage: layer " << layer_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  LayerObject& layer_object = GetLayerObject(it->second);

  auto image_it = images2_.find(image_id);
  if (image_it == images2_.end()) {
    error_reporter_->ERROR() << "SetLayerImage: image " << image_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  // TODO(https://fxbug.dev/540952629): Add support for acquire and release fences.
  if (acquire_fence.has_value() || release_fence.has_value()) {
    error_reporter_->ERROR() << "SetLayerImage: fences not yet implemented";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  BindLayerImage(layer_object, image_it->second);
}

void Flatland::SetLayerProperties(SetLayerPropertiesRequestView request,
                                  SetLayerPropertiesCompleter::Sync& completer) {
  SetLayerProperties(LayerId(request->layer_id.value), request->properties);
}

void Flatland::SetLayerProperties(LayerId layer_id,
                                  const fuchsia_ui_composition::wire::LayerProperties& properties) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "SetLayerProperties called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto it = layer_handles_.find(layer_id);
  if (it == layer_handles_.end()) {
    error_reporter_->ERROR() << "SetLayerProperties: layer " << layer_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  LayerObject& layer_object = GetLayerObject(it->second);

  // TODO(https://fxbug.dev/474444799): is there anything to check here beyond well-formedness?
  // This will be clipped downstream anyway, and the layer discarded if invisible, right?
  if (properties.has_display_rect()) {
    if (!types::Rectangle::IsValid(properties.display_rect())) {
      error_reporter_->ERROR() << "SetLayerProperties: display_rect is invalid";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }
    layer_object.common.display_rect = types::Rectangle::From(properties.display_rect());
  }

  if (properties.has_opacity()) {
    const float opacity = properties.opacity();
    if (opacity < 0.f || opacity > 1.f || isnan(opacity) || isinf(opacity)) {
      error_reporter_->ERROR() << "SetLayerProperties: opacity value " << opacity
                               << " is not within valid range [0, 1]";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }
    layer_object.common.opacity = opacity;
  }

  if (properties.has_blend_mode()) {
    if (properties.blend_mode().IsUnknown()) {
      error_reporter_->ERROR() << "SetLayerProperties: unknown blend mode";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }
    layer_object.common.blend_mode = types::BlendMode::From(properties.blend_mode());
  }

  if (properties.has_color()) {
    const auto& color = properties.color();
    if (color.red < 0.f || color.red > 1.f || isnan(color.red) || isinf(color.red) ||
        color.green < 0.f || color.green > 1.f || isnan(color.green) || isinf(color.green) ||
        color.blue < 0.f || color.blue > 1.f || isnan(color.blue) || isinf(color.blue) ||
        color.alpha < 0.f || color.alpha > 1.f || isnan(color.alpha) || isinf(color.alpha)) {
      error_reporter_->ERROR() << "SetLayerProperties: Invalid color channel(s) (" << color.red
                               << ", " << color.green << ", " << color.blue << ", " << color.alpha
                               << ")";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }
    layer_object.solid_color_mode.color = {color.red, color.green, color.blue, color.alpha};
  }

  if (properties.has_sample_rect()) {
    // This simple well-formedness check is necessary but insufficient.
    // The sample rect must also be validated against the dimensions of the layer's bound image,
    // which is validated once per Present(), and only for layers whose composition mode is IMAGE.
    // Doing that check here would force clients into a data-dependent call order (growing an
    // image requires SetLayerImage first; shrinking it requires SetLayerProperties first),
    // could not validate a rect authored while no image is bound, and would reject
    // intermediate states that never reach the screen. See the `sample_rect` doc comment in
    // flatland2.fidl.
    if (!types::RectangleF::IsValid(properties.sample_rect())) {
      error_reporter_->ERROR() << "SetLayerProperties: sample_rect is invalid";
      CloseConnection(FlatlandError::kBadOperation);
      return;
    }
    const auto sample_rect = types::RectangleF::From(properties.sample_rect());
    if (sample_rect == types::RectangleF()) {
      // (0,0,0,0) is the sentinel value meaning "sample the entire image".
      layer_object.sample_rect_is_full_image = true;
      if (layer_object.image_mode.image_id != allocation::kInvalidImageId) {
        layer_object.image_mode.sample_rect = types::RectangleF({
            .x = 0.f,
            .y = 0.f,
            .width = static_cast<float>(layer_object.image_mode.image_width),
            .height = static_cast<float>(layer_object.image_mode.image_height),
        });
      } else {
        layer_object.image_mode.sample_rect = types::RectangleF();
      }
    } else {
      // A non-zero rect specifies a concrete sub-rect, clearing the full-image sentinel.
      layer_object.sample_rect_is_full_image = false;
      layer_object.image_mode.sample_rect = sample_rect;
    }
  }

  if (properties.has_transform()) {
    layer_object.image_mode.transform = types::RotateFlip::From(properties.transform());
  }

  if (properties.has_hint_damage_rects()) {
    std::vector<types::Rectangle> damage_rects;
    damage_rects.reserve(properties.hint_damage_rects().size());
    for (const auto& rect : properties.hint_damage_rects()) {
      if (!types::Rectangle::IsValid(rect)) {
        error_reporter_->ERROR() << "SetLayerProperties: hint_damage_rects entry is invalid";
        CloseConnection(FlatlandError::kBadOperation);
        return;
      }
      damage_rects.push_back(types::Rectangle::From(rect));
    }
    layer_object.hint_damage_rects = std::move(damage_rects);
  }

  if (properties.has_hint_visible_rects()) {
    std::vector<types::Rectangle> visible_rects;
    visible_rects.reserve(properties.hint_visible_rects().size());
    for (const auto& rect : properties.hint_visible_rects()) {
      if (!types::Rectangle::IsValid(rect)) {
        error_reporter_->ERROR() << "SetLayerProperties: hint_visible_rects entry is invalid";
        CloseConnection(FlatlandError::kBadOperation);
        return;
      }
      visible_rects.push_back(types::Rectangle::From(rect));
    }
    layer_object.hint_visible_rects = std::move(visible_rects);
  }

  if (properties.has_composition_mode()) {
    switch (properties.composition_mode()) {
      case fuchsia_ui_composition::CompositionMode::kInvisible:
        layer_object.mode = LayerObject::Mode::kInvisible;
        break;
      case fuchsia_ui_composition::CompositionMode::kImage:
        layer_object.mode = LayerObject::Mode::kImage;
        break;
      case fuchsia_ui_composition::CompositionMode::kSolidColor:
        layer_object.mode = LayerObject::Mode::kSolidColor;
        break;
      default:
        error_reporter_->ERROR() << "SetLayerProperties: Unknown composition_mode";
        CloseConnection(FlatlandError::kBadOperation);
        return;
    }
  }
}

void Flatland::ResetLayer(ResetLayerRequestView request, ResetLayerCompleter::Sync& completer) {
  ResetLayer(LayerId(request->layer_id.value));
}

void Flatland::ResetLayer(LayerId layer_id) {
  if (!config_.use_flatland2) {
    error_reporter_->ERROR() << "ResetLayer called, but Flatland2 not enabled";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }

  auto it = layer_handles_.find(layer_id);
  if (it == layer_handles_.end()) {
    error_reporter_->ERROR() << "ResetLayer: layer " << layer_id << " not found";
    CloseConnection(FlatlandError::kBadOperation);
    return;
  }
  LayerObject& layer_object = GetLayerObject(it->second);

  UnbindLayerImage(layer_object);
  layer_object.common = UberStructLayer::CommonProperties{};
  layer_object.image_mode = UberStructLayer::ImageModeProperties{};
  layer_object.solid_color_mode = UberStructLayer::SolidColorModeProperties{};
  layer_object.mode = LayerObject::Mode::kInvisible;
  layer_object.sample_rect_is_full_image = true;
  layer_object.hint_damage_rects.clear();
  layer_object.hint_visible_rects.clear();
}

void Flatland::OnNextFrameBegin(uint32_t additional_present_credits,
                                FuturePresentationInfos presentation_infos) {
  TRACE_DURATION("gfx", "Flatland::OnNextFrameBegin");
  present_credits_ += additional_present_credits;

  FLATLAND_VERBOSE_LOG << "Flatland::OnNextFrameBegin() session_id=" << session_id_
                       << "  additional_present_credits=" << additional_present_credits
                       << "  skips_present_credits=" << config_.skips_present_credits;

  // Only send an `OnNextFrameBegin` event if the client has not opted out of the Flatland present
  // credit flow and has at least one present credit. It is guaranteed that this won't stall
  // clients because the current policy is to always return present tokens upon processing them.
  // If and when a new policy is adopted, we should take care to ensure this guarantee is upheld.
  if (binding_data_ && !config_.skips_present_credits) {
    if (present_credits_ > 0) {
      binding_data_->SendOnNextFrameBegin(additional_present_credits,
                                          std::move(presentation_infos));
    }
  }
}

void Flatland::OnFramePresented(const std::map<scheduling::PresentId, zx::time>& latched_times,
                                scheduling::PresentTimestamps present_times) {
  TRACE_DURATION("gfx", "Flatland::OnFramePresented");

#if USE_FLATLAND_VERBOSE_LOGGING
  std::ostringstream oss;
  oss << "Flatland::OnFramePresented() session_id=" << session_id_;
  for (const auto& [present_id, time] : latched_times) {
    oss << "\n         present_id=" << present_id << "  time=" << time.get();
  }
  FLATLAND_VERBOSE_LOG << oss.str();
#endif

  if (TRACE_CATEGORY_ENABLED("gfx")) {
    for (const auto& [present_id, latched_timestamp] : latched_times) {
      TRACE_INSTAFLOW_END("gfx", "scenic_session_present", "flatland_frame_presented",
                          SESSION_TRACE_ID(session_id_, present_id), "session_id",
                          TA_UINT64(session_id_), "present_id", TA_UINT64(present_id),
                          "latched_time", TA_INT64(latched_timestamp.get()), "presentation_time",
                          TA_INT64(present_times.presented_time.get()));
    }
  }

  if (!config_.skips_on_frame_presented) {
    // TODO(https://fxbug.dev/42141795): remove `num_presents_allowed` from this event.  Clients
    // should obtain this information from OnPresentProcessedValues().
    present2_helper_.OnPresented(latched_times, present_times, /*num_presents_allowed=*/0);
  } else {
    // This is not harmful.  However, we expect that `FlatlandManager` will avoid calling this, to
    // avoid posting this no-op call on this Flatland session's dispatcher.
    FX_LOGS(WARNING) << "Skipping OnFramePresented for session_id=" << session_id_;
  }
}

TransformHandle Flatland::GetRoot() const {
  return link_to_parent_ ? link_to_parent_->child_transform_handle : local_root_;
}

std::optional<TransformHandle> Flatland::GetContentHandle(ContentId content_id) const {
  auto handle_kv = content_handles_.find(content_id);
  if (handle_kv == content_handles_.end()) {
    return std::nullopt;
  }
  return handle_kv->second;
}

// For validating properties associated with transforms in tests only. If |transform_id| does not
// exist for this Flatland instance, returns std::nullopt.
std::optional<TransformHandle> Flatland::GetTransformHandle(TransformId transform_id) const {
  auto handle_kv = transforms_.find(transform_id);
  if (handle_kv == transforms_.end()) {
    return std::nullopt;
  }
  return handle_kv->second;
}

void Flatland::SetErrorReporter(std::unique_ptr<scenic_impl::ErrorReporter> error_reporter) {
  error_reporter_ = std::move(error_reporter);
}

scheduling::SessionId Flatland::GetSessionId() const { return session_id_; }

void Flatland::OnFidlClosed(fidl::UnbindInfo unbind_info) {
  if (!unbind_info.is_user_initiated()) {
    FX_LOGS(INFO) << "Flatland::OnFidlClosed() session_id=" << session_id_
                  << " because: " << unbind_info.FormatDescription();
  }

  binding_data_.reset();
}

void Flatland::ReportLinkProtocolError(const std::string& error_log) {
  error_reporter_->ERROR() << error_log;
  link_protocol_error_ = true;
}

void Flatland::CloseConnection(FlatlandError error) {
  if (binding_data_) {
    FX_LOGS(INFO) << "Flatland::CloseConnection() session_id=" << session_id_
                  << "  error: " << error;

    binding_data_->CloseConnection(error);
    binding_data_.reset();
  }
}

// MatrixData function implementations

// static
float Flatland::MatrixData::GetOrientationAngle(fuchsia_ui_composition::Orientation orientation) {
  // The matrix is specified in view-space coordinates, in which the +y axis points downwards (not
  // upwards). Rotations which are specified as counter-clockwise must actually occur in a
  // clockwise fashion in this coordinate space (a vector on the +x axis rotates towards -y axis
  // to give the appearance of a counter-clockwise rotation).
  switch (orientation) {
    case Orientation::kCcw0Degrees:
      return 0.f;
    case Orientation::kCcw90Degrees:
      return -glm::half_pi<float>();
    case Orientation::kCcw180Degrees:
      return -glm::pi<float>();
    case Orientation::kCcw270Degrees:
      return -glm::three_over_two_pi<float>();
  }
  __UNREACHABLE;
  return 0.f;
}

void Flatland::MatrixData::SetTranslation(fuchsia_math::wire::Vec translation) {
  translation_.x = static_cast<float>(translation.x);
  translation_.y = static_cast<float>(translation.y);
  RecomputeMatrix();
}

void Flatland::MatrixData::SetOrientation(fuchsia_ui_composition::Orientation orientation) {
  angle_ = GetOrientationAngle(orientation);

  RecomputeMatrix();
}

void Flatland::MatrixData::SetScale(fuchsia_math::wire::VecF scale) {
  scale_.x = scale.x;
  scale_.y = scale.y;
  RecomputeMatrix();
}

void Flatland::MatrixData::RecomputeMatrix() {
  // Manually compose the matrix rather than use glm transformations since the order of operations
  // is always the same. glm matrices are column-major, so are indexed like:
  //   0 3 6
  //   1 4 7
  //   2 5 8
  float* vals = static_cast<float*>(glm::value_ptr(matrix_));

  // Translation in the third column.
  vals[6] = translation_.x;
  vals[7] = translation_.y;

  // Rotation and scale combined into the first two columns.
  const float s = sin(angle_);
  const float c = cos(angle_);

  vals[0] = c * scale_.x;
  vals[1] = s * scale_.x;
  vals[3] = -1.f * s * scale_.y;
  vals[4] = c * scale_.y;
}

glm::mat3 Flatland::MatrixData::GetMatrix() const { return matrix_; }

LayerHandle Flatland::CreateLayerObject() {
  LayerHandle handle(session_id_, next_layer_handle_++);
  LayerObject obj;
  obj.ref_count = 0;
  layer_objects_[handle] = std::move(obj);
  return handle;
}

void Flatland::ReleaseImageObject(allocation::GlobalImageId id) {
  auto it = image_objects_.find(id);
  FX_CHECK(it != image_objects_.end()) << "Image not found: " << id.value();
  FX_CHECK(it->second.ref_count > 0) << "Image ref_count underflow: " << id.value();
  if (--it->second.ref_count > 0) {
    return;
  }
  image_objects_.erase(it);
  images_to_release_on_present_.push_back(id);
}

// TODO(https://fxbug.dev/540952629): When release fences are implemented, UnbindLayerImage will
// also stage the binding's release fence.
void Flatland::UnbindLayerImage(LayerObject& layer) {
  auto& image_mode = layer.image_mode;
  if (image_mode.image_id == allocation::kInvalidImageId) {
    return;
  }
  const allocation::GlobalImageId id = image_mode.image_id;
  image_mode.image_id = allocation::kInvalidImageId;
  image_mode.image_width = 0;
  image_mode.image_height = 0;
  ReleaseImageObject(id);
}

void Flatland::BindLayerImage(LayerObject& layer, allocation::GlobalImageId id) {
  FX_CHECK(id != allocation::kInvalidImageId);
  if (layer.image_mode.image_id == id) {
    return;
  }

  auto it = image_objects_.find(id);
  FX_CHECK(it != image_objects_.end()) << "Image not found: " << id.value();
  ++it->second.ref_count;

  UnbindLayerImage(layer);

  auto& image_mode = layer.image_mode;
  image_mode.image_id = id;
  image_mode.image_width = it->second.metadata.width;
  image_mode.image_height = it->second.metadata.height;

  if (layer.sample_rect_is_full_image) {
    image_mode.sample_rect = types::RectangleF({
        .x = 0.f,
        .y = 0.f,
        .width = static_cast<float>(image_mode.image_width),
        .height = static_cast<float>(image_mode.image_height),
    });
  }
}

void Flatland::ReleaseImages(std::span<const allocation::GlobalImageId> ids) {
  for (const auto& image_id : ids) {
    import_tokens_->erase(image_id);
    for (auto& importer : buffer_collection_importers_) {
      importer->ReleaseBufferImage(image_id);
    }
  }
}

void Flatland::ReleaseLayerObject(LayerHandle handle) {
  auto it = layer_objects_.find(handle);
  FX_CHECK(it != layer_objects_.end()) << "Layer not found: " << handle;
  FX_DCHECK(it->second.ref_count > 0);
  it->second.ref_count--;
  if (it->second.ref_count > 0) {
    return;
  }

  UnbindLayerImage(it->second);
  layer_objects_.erase(it);
}

TransformHandle Flatland::CreateLayerStackData() {
  TransformHandle content_handle = transform_graph_.CreateTransform();
  layer_stacks_.try_emplace(content_handle, LayerStackData{std::pmr::vector<LayerHandle>(&pool_)});
  return content_handle;
}

void Flatland::SetLayerStackData(TransformHandle stack_handle,
                                 std::span<const LayerHandle> layers) {
  auto stack_it = layer_stacks_.find(stack_handle);
  FX_CHECK(stack_it != layer_stacks_.end()) << "Stack not found: " << stack_handle;

  for (const auto& handle : layers) {
    auto it = layer_objects_.find(handle);
    FX_CHECK(it != layer_objects_.end()) << "Layer not found: " << handle;
    it->second.ref_count++;
  }

  for (const auto& handle : stack_it->second.layers) {
    ReleaseLayerObject(handle);
  }

  stack_it->second.layers.assign(layers.begin(), layers.end());
}

std::vector<allocation::GlobalImageId> Flatland::CleanupFlatland2StateForTest(
    const std::vector<TransformHandle>& dead_handles) {
  const size_t initial_size = images_to_release_on_present_.size();

  // Drop stacks whose content_handle is dead.
  for (const auto& dead_handle : dead_handles) {
    auto it = layer_stacks_.find(dead_handle);
    // Not all transform handles correspond to layer stacks, so it's OK to not find a layer stack.
    // TODO(https://fxbug.dev/523371761): revisit this when finalizing Flatland1 facade, to see if
    // this function's shape/signature/impl still make sense.
    if (it != layer_stacks_.end()) {
      // Decrement ref_count of all layers in the stack.
      for (const auto& layer_handle : it->second.layers) {
        ReleaseLayerObject(layer_handle);
      }
      layer_stacks_.erase(it);
    }
  }

  return std::vector<allocation::GlobalImageId>(
      images_to_release_on_present_.begin() + initial_size, images_to_release_on_present_.end());
}

void Flatland::SetLayerImageForTest(LayerHandle handle, allocation::GlobalImageId image) {
  auto it = layer_objects_.find(handle);
  FX_CHECK(it != layer_objects_.end()) << "Layer not found: " << handle;
  it->second.mode = LayerObject::Mode::kImage;
  if (image == allocation::kInvalidImageId) {
    UnbindLayerImage(it->second);
    return;
  }
  // Tests bind ids that were never imported; give them an ImageObject.
  image_objects_.try_emplace(image, ImageObject{.ref_count = 0});
  BindLayerImage(it->second, image);
}

void Flatland::SetLayerSolidColorForTest(LayerHandle handle) {
  auto it = layer_objects_.find(handle);
  FX_CHECK(it != layer_objects_.end()) << "Layer not found: " << handle;
  if (it == layer_objects_.end()) {
    return;
  }
  it->second.mode = LayerObject::Mode::kSolidColor;
}

LayerObject* Flatland::GetLayerObjectForTest(LayerHandle handle) {
  auto it = layer_objects_.find(handle);
  if (it == layer_objects_.end()) {
    return nullptr;
  }
  return &it->second;
}

const ImageObject* Flatland::GetImageObjectForTest(allocation::GlobalImageId global_id) const {
  auto it = image_objects_.find(global_id);
  if (it == image_objects_.end()) {
    return nullptr;
  }
  return &it->second;
}

const LayerStackData* Flatland::GetLayerStackDataForTest(TransformHandle handle) {
  auto it = layer_stacks_.find(handle);
  if (it == layer_stacks_.end()) {
    return nullptr;
  }
  return &it->second;
}

void Flatland::ReleaseTransformForTest(TransformHandle handle) {
  transform_graph_.ReleaseTransform(handle);
}

void Flatland::SetPriorityChildForTest(TransformId parent, TransformHandle child) {
  auto it = transforms_.find(parent);
  if (it != transforms_.end()) {
    transform_graph_.SetPriorityChild(it->second, child);
  }
}

LayerHandle Flatland::GetLayerHandleForTest(LayerId layer_id) {
  auto it = layer_handles_.find(layer_id);
  if (it == layer_handles_.end()) {
    return LayerHandle();
  }
  return it->second;
}

std::optional<TransformHandle> Flatland::GetLayerStackHandleForTest(
    LayerStackId layer_stack_id) const {
  auto it = layer_stack_handles_.find(layer_stack_id);
  if (it == layer_stack_handles_.end()) {
    return std::nullopt;
  }
  return it->second;
}

size_t Flatland::PendingImageReleaseCountForTest() const { return pending_image_releases_.size(); }

allocation::GlobalImageId Flatland::GetGlobalImageIdForTest(ImageId image_id) const {
  auto it = images2_.find(image_id);
  if (it == images2_.end()) {
    return allocation::kInvalidImageId;
  }
  return it->second;
}

}  // namespace flatland
