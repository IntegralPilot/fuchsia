// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>

#include <functional>
#include <initializer_list>
#include <list>
#include <map>
#include <memory>
#include <memory_resource>
#include <optional>
#include <span>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>

#include "src/ui/lib/escher/flib/fence_queue.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/flatland_config.h"
#include "src/ui/scenic/lib/flatland/flatland_presenter.h"
#include "src/ui/scenic/lib/flatland/flatland_session_types.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/link_system.h"
#include "src/ui/scenic/lib/flatland/transform_graph.h"
#include "src/ui/scenic/lib/flatland/transform_handle.h"
#include "src/ui/scenic/lib/flatland/uber_struct_system.h"
#include "src/ui/scenic/lib/scenic/util/error_reporter.h"
#include "src/ui/scenic/lib/scheduling/id.h"
#include "src/ui/scenic/lib/scheduling/present2_helper.h"
#include "src/ui/scenic/lib/utils/dispatcher_holder.h"

#include <glm/glm.hpp>
#include <glm/mat3x3.hpp>
#include <glm/vec2.hpp>

namespace flatland {

// Implements the `fuchsia.ui.composition.Flatland` protocol.  It is intended to run on its own
// thread/dispatcher, and communicates with the main/render thread(s) via the UberStruct mechanism,
// as well as other interfaces such as FlatlandPresenter.  Because `fuchsia.ui.composition.Flatland`
// is a stateful protocol, each client is connected to a different Flatland object.
class Flatland : public fidl::WireServer<fuchsia_ui_composition::Flatland>,
                 public std::enable_shared_from_this<Flatland> {
 public:
  using BufferCollectionId = uint64_t;
  using FuturePresentationInfos = std::vector<fuchsia_scenic_scheduling::PresentationInfo>;

  // Instantiates a new Flatland object and binds it to serve the Flatland protocol over the
  // `server_end` channel.  Method invocations received on this channel will be serviced on
  // the thread managed by `dispatcher_holder`.
  //
  // The `destroy_instance_function` is called to notify the instance's manager that the instance
  // should be destroyed.  This function is invoked on the thread owned by `dispatcher_holder`. When
  // this function is invoked, the client FIDL connection has already been closed.  There are two
  // situations that result in the invocation of `destroy_instance_function`:
  //   - the client closes the FIDL channel
  //   - the client makes illegal use of the API (or associated APIs like ChildViewWatcher)
  //
  // `flatland_presenter`, `link_system`, `uber_struct_queue`, and `buffer_collection_importers`
  // allow this Flatland object to access resources shared by all Flatland instances for actions
  // like frame scheduling, linking, buffer allocation, and presentation to the global scene graph.
  static std::shared_ptr<Flatland> New(
      std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
      fidl::ServerEnd<fuchsia_ui_composition::Flatland> server_end,
      scheduling::SessionId session_id, std::function<void()> destroy_instance_function,
      std::shared_ptr<FlatlandPresenter> flatland_presenter,
      std::shared_ptr<LinkSystem> link_system,
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
      const FlatlandConfig& config);

  // Because this object captures its "this" pointer in internal closures, it is unsafe to copy or
  // move it. Disable all copy and move operations.
  Flatland(const Flatland&) = delete;
  Flatland& operator=(const Flatland&) = delete;
  Flatland(Flatland&&) = delete;
  Flatland& operator=(Flatland&&) = delete;

  ~Flatland() override;

  // |fuchsia_ui_composition::Flatland|
  void Present(PresentRequestView request, PresentCompleter::Sync& completer) override;
  void Present(fuchsia_ui_composition::wire::PresentArgs& args);

  // |fuchsia_ui_composition::Flatland|
  void CreateView(CreateViewRequestView request, CreateViewCompleter::Sync& completer) override;
  void CreateView(
      fuchsia_ui_views::wire::ViewCreationToken token,
      fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher> parent_viewport_watcher);
  void CreateView2(CreateView2RequestView request, CreateView2Completer::Sync& completer) override;
  void CreateView2(
      fuchsia_ui_views::wire::ViewCreationToken token,
      fuchsia_ui_views::wire::ViewIdentityOnCreation view_identity,
      fuchsia_ui_composition::wire::ViewBoundProtocols protocols,
      fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher> parent_viewport_watcher);

  // |fuchsia_ui_composition::Flatland|
  // TODO(https://fxbug.dev/42162046): Consider returning tokens for re-linking.
  void ReleaseView(ReleaseViewCompleter::Sync& completer) override;
  void ReleaseView();

  // |fuchsia_ui_composition::Flatland|
  void Clear(ClearCompleter::Sync& completer) override;
  void Clear();

  // |fuchsia_ui_composition::Flatland|
  void CreateTransform(CreateTransformRequestView request,
                       CreateTransformCompleter::Sync& completer) override;
  void CreateTransform(TransformId transform_id);

  // |fuchsia_ui_composition::Flatland|
  void SetTranslation(SetTranslationRequestView request,
                      SetTranslationCompleter::Sync& completer) override;
  void SetTranslation(TransformId transform_id, fuchsia_math::wire::Vec translation);

  // |fuchsia_ui_composition::Flatland|
  void SetOrientation(SetOrientationRequestView request,
                      SetOrientationCompleter::Sync& completer) override;
  void SetOrientation(TransformId transform_id, fuchsia_ui_composition::Orientation orientation);

  // |fuchsia_ui_composition::Flatland|
  void SetScale(SetScaleRequestView request, SetScaleCompleter::Sync& completer) override;
  void SetScale(TransformId transform_id, fuchsia_math::wire::VecF scale);

  // |fuchsia_ui_composition::Flatland|
  void SetOpacity(SetOpacityRequestView request, SetOpacityCompleter::Sync& completer) override;
  void SetOpacity(TransformId transform_id, float value);

  // |fuchsia_ui_composition::Flatland|
  void SetClipBoundary(SetClipBoundaryRequestView request,
                       SetClipBoundaryCompleter::Sync& completer) override;
  void SetClipBoundary(TransformId transform_id, std::optional<fuchsia_math::wire::Rect> bounds);

  // |fuchsia_ui_composition::Flatland|
  void AddChild(AddChildRequestView request, AddChildCompleter::Sync& completer) override;
  void AddChild(TransformId parent_transform_id, TransformId child_transform_id);

  // |fuchsia_ui_composition::Flatland|
  void RemoveChild(RemoveChildRequestView request, RemoveChildCompleter::Sync& completer) override;
  void RemoveChild(TransformId parent_transform_id, TransformId child_transform_id);

  // |fuchsia_ui_composition::Flatland|
  void ReplaceChildren(ReplaceChildrenRequestView request,
                       ReplaceChildrenCompleter::Sync& completer) override;
  void ReplaceChildren(TransformId parent_transform_id,
                       std::span<const TransformId> new_child_transform_ids);
  void ReplaceChildren(TransformId parent_transform_id,
                       std::initializer_list<TransformId> new_child_transform_ids) {
    ReplaceChildren(parent_transform_id,
                    std::span(new_child_transform_ids.begin(), new_child_transform_ids.size()));
  }

  // |fuchsia_ui_composition::Flatland|
  void SetRootTransform(SetRootTransformRequestView request,
                        SetRootTransformCompleter::Sync& completer) override;
  void SetRootTransform(TransformId transform_id);

  // |fuchsia_ui_composition::Flatland|
  void CreateViewport(CreateViewportRequestView request,
                      CreateViewportCompleter::Sync& completer) override;
  void CreateViewport(ContentId viewport_id, fuchsia_ui_views::wire::ViewportCreationToken token,
                      const fuchsia_ui_composition::wire::ViewportProperties& properties,
                      fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> child_view_watcher);

  void CreateViewport2(CreateViewport2RequestView request,
                       CreateViewport2Completer::Sync& completer) override;
  void CreateViewport2(
      ViewportId viewport_id, fuchsia_ui_views::wire::ViewportCreationToken token,
      const fuchsia_ui_composition::wire::ViewportProperties& properties,
      fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> child_view_watcher);

  // |fuchsia_ui_composition::Flatland|
  void CreateImage(CreateImageRequestView request, CreateImageCompleter::Sync& completer) override;
  void CreateImage(ContentId image_id,
                   fuchsia_ui_composition::wire::BufferCollectionImportToken import_token,
                   uint32_t vmo_index,
                   const fuchsia_ui_composition::wire::ImageProperties& properties);

  void CreateImage2(CreateImage2RequestView request,
                    CreateImage2Completer::Sync& completer) override;
  void CreateImage2(ImageId image_id,
                    fuchsia_ui_composition::wire::BufferCollectionImportToken import_token,
                    uint32_t vmo_index,
                    const fuchsia_ui_composition::wire::ImageProperties& properties);

  // |fuchsia_ui_composition::Flatland|
  void SetImageSampleRegion(SetImageSampleRegionRequestView request,
                            SetImageSampleRegionCompleter::Sync& completer) override;
  void SetImageSampleRegion(ContentId image_id, types::RectangleF rect);

  // |fuchsia_ui_composition::Flatland|
  void SetImageDestinationSize(SetImageDestinationSizeRequestView request,
                               SetImageDestinationSizeCompleter::Sync& completer) override;
  void SetImageDestinationSize(ContentId image_id, fuchsia_math::wire::SizeU size);

  // |fuchsia_ui_composition::Flatland|
  void SetImageBlendingFunction(SetImageBlendingFunctionRequestView request,
                                SetImageBlendingFunctionCompleter::Sync& completer) override;

  // |fuchsia_ui_composition::Flatland|
  void SetImageBlendMode(SetImageBlendModeRequestView request,
                         SetImageBlendModeCompleter::Sync& completer) override;
  void SetImageBlendMode(ContentId image_id, BlendMode blend_mode);

  // |fuchsia_ui_composition::Flatland|
  void SetImageFlip(SetImageFlipRequestView request,
                    SetImageFlipCompleter::Sync& completer) override;
  void SetImageFlip(ContentId image_id, fuchsia_ui_composition::ImageFlip flip);

  // |fuchsia_ui_composition::Flatland|
  void CreateFilledRect(CreateFilledRectRequestView request,
                        CreateFilledRectCompleter::Sync& completer) override;
  void CreateFilledRect(ContentId rect_id);

  // |fuchsia_ui_composition::Flatland|
  void SetSolidFill(SetSolidFillRequestView request,
                    SetSolidFillCompleter::Sync& completer) override;
  void SetSolidFill(ContentId rect_id, fuchsia_ui_composition::wire::ColorRgba color,
                    fuchsia_math::wire::SizeU size);

  // |fuchsia_ui_composition::Flatland|
  void ReleaseFilledRect(ReleaseFilledRectRequestView request,
                         ReleaseFilledRectCompleter::Sync& completer) override;
  void ReleaseFilledRect(ContentId rect_id);

  // |fuchsia_ui_composition::Flatland|
  void SetImageOpacity(SetImageOpacityRequestView request,
                       SetImageOpacityCompleter::Sync& completer) override;
  void SetImageOpacity(ContentId image_id, float opacity);

  // |fuchsia_ui_composition::Flatland|
  void SetHitRegions(SetHitRegionsRequestView request,
                     SetHitRegionsCompleter::Sync& completer) override;
  void SetHitRegions(TransformId transform_id,
                     std::span<const fuchsia_ui_composition::wire::HitRegion> regions);
  void SetHitRegions(TransformId transform_id,
                     std::initializer_list<fuchsia_ui_composition::wire::HitRegion> regions) {
    SetHitRegions(transform_id, std::span(regions.begin(), regions.size()));
  }

  // |fuchsia_ui_composition::Flatland|
  void SetInfiniteHitRegion(SetInfiniteHitRegionRequestView request,
                            SetInfiniteHitRegionCompleter::Sync& completer) override;
  void SetInfiniteHitRegion(TransformId transform_id,
                            fuchsia_ui_composition::HitTestInteraction hit_test);

  // |fuchsia_ui_composition::Flatland|
  void SetContent(SetContentRequestView request, SetContentCompleter::Sync& completer) override;
  void SetContent(TransformId transform_id, ContentId content_id);

  // |fuchsia_ui_composition::Flatland2|
  void SetTransformContent(SetTransformContentRequestView request,
                           SetTransformContentCompleter::Sync& completer) override;
  void SetTransformContent(TransformId transform_id,
                           const fuchsia_ui_composition::wire::TransformContent* content);
  void SetTransformContent(TransformId transform_id, LayerStackId layer_stack_id);
  void SetTransformContent(TransformId transform_id, ViewportId viewport_id);
  void ClearTransformContent(TransformId transform_id);

  // |fuchsia_ui_composition::Flatland|
  void SetViewportProperties(SetViewportPropertiesRequestView request,
                             SetViewportPropertiesCompleter::Sync& completer) override;
  void SetViewportProperties(ContentId viewport_id,
                             const fuchsia_ui_composition::wire::ViewportProperties& properties);

  void SetViewportProperties2(SetViewportProperties2RequestView request,
                              SetViewportProperties2Completer::Sync& completer) override;
  void SetViewportProperties2(ViewportId viewport_id,
                              const fuchsia_ui_composition::wire::ViewportProperties& properties);

  // |fuchsia_ui_composition::Flatland|
  void ReleaseTransform(ReleaseTransformRequestView request,
                        ReleaseTransformCompleter::Sync& completer) override;
  void ReleaseTransform(TransformId transform_id);

  // |fuchsia_ui_composition::Flatland|
  void ReleaseViewport(ReleaseViewportRequestView request,
                       ReleaseViewportCompleter::Sync& completer) override;
  void ReleaseViewport(
      ContentId viewport_id,
      fit::function<void(fuchsia_ui_views::wire::ViewportCreationToken)> completer);

  void ReleaseViewport2(ReleaseViewport2RequestView request,
                        ReleaseViewport2Completer::Sync& completer) override;
  void ReleaseViewport2(
      ViewportId viewport_id,
      fit::function<void(fuchsia_ui_views::wire::ViewportCreationToken)> completer);

  // |fuchsia_ui_composition::Flatland|
  void ReleaseImage(ReleaseImageRequestView request,
                    ReleaseImageCompleter::Sync& completer) override;
  void ReleaseImage(ContentId image_id);

  // |fuchsia_ui_composition::Flatland2|
  void ReleaseImage2(ReleaseImage2RequestView request,
                     ReleaseImage2Completer::Sync& completer) override;
  void ReleaseImage2(ImageId image_id);

  // |fuchsia_ui_composition::Flatland|
  void SetDebugName(SetDebugNameRequestView request,
                    SetDebugNameCompleter::Sync& completer) override;
  void SetDebugName(std::string name);

  // |fuchsia_ui_composition::TrustedFlatland|
  void ReleaseImageImmediately(ReleaseImageImmediatelyRequestView request,
                               ReleaseImageImmediatelyCompleter::Sync& completer) override;
  void ReleaseImageImmediately(ContentId image_id);

  void ReleaseImageImmediately2(ReleaseImageImmediately2RequestView request,
                                ReleaseImageImmediately2Completer::Sync& completer) override;
  void ReleaseImageImmediately2(ImageId image_id);

  // |fuchsia_ui_composition::Flatland2|
  void CreateLayer(CreateLayerRequestView request, CreateLayerCompleter::Sync& completer) override;
  void CreateLayer(LayerId layer_id);

  // |fuchsia_ui_composition::Flatland2|
  void ReleaseLayer(ReleaseLayerRequestView request,
                    ReleaseLayerCompleter::Sync& completer) override;
  void ReleaseLayer(LayerId layer_id);

  // |fuchsia_ui_composition::Flatland2|
  void CreateLayerStack(CreateLayerStackRequestView request,
                        CreateLayerStackCompleter::Sync& completer) override;
  void CreateLayerStack(LayerStackId layer_stack_id);

  // |fuchsia_ui_composition::Flatland2|
  void ReleaseLayerStack(ReleaseLayerStackRequestView request,
                         ReleaseLayerStackCompleter::Sync& completer) override;
  void ReleaseLayerStack(LayerStackId layer_stack_id);

  // |fuchsia_ui_composition::Flatland2|
  void SetStackLayers(SetStackLayersRequestView request,
                      SetStackLayersCompleter::Sync& completer) override;
  void SetStackLayers(LayerStackId layer_stack_id, std::span<const LayerId> layers);
  void SetStackLayers(LayerStackId layer_stack_id, std::initializer_list<LayerId> layers) {
    SetStackLayers(layer_stack_id, std::span<const LayerId>(layers.begin(), layers.end()));
  }

  // |fuchsia_ui_composition::Flatland2|
  void SetLayerImage(SetLayerImageRequestView request,
                     SetLayerImageCompleter::Sync& completer) override;
  void SetLayerImage(LayerId layer_id, ImageId image_id,
                     std::optional<fuchsia_ui_composition::wire::WaitFence> acquire_fence,
                     std::optional<fuchsia_ui_composition::wire::SignalFence> release_fence);

  // |fuchsia_ui_composition::Flatland2|
  void SetLayerProperties(SetLayerPropertiesRequestView request,
                          SetLayerPropertiesCompleter::Sync& completer) override;
  void SetLayerProperties(LayerId layer_id,
                          const fuchsia_ui_composition::wire::LayerProperties& properties);

  // |fuchsia_ui_composition::Flatland2|
  void ResetLayer(ResetLayerRequestView request, ResetLayerCompleter::Sync& completer) override;
  void ResetLayer(LayerId layer_id);

  // Called just before the FIDL client receives the event of the same name, indicating that this
  // Flatland instance should allow a |additional_present_credits| calls to Present().
  void OnNextFrameBegin(uint32_t additional_present_credits,
                        FuturePresentationInfos presentation_infos);

  // Called when this Flatland instance should send the OnFramePresented() event to the FIDL
  // client.
  void OnFramePresented(const std::map<scheduling::PresentId, zx::time>& latched_times,
                        scheduling::PresentTimestamps present_times);

  // For validating the transform hierarchy in tests only. For the sake of testing, the "root" will
  // always be the top-most TransformHandle from the TransformGraph owned by this Flatland. If
  // currently linked to a parent, that means the Link's child_transform_handle. If not, that means
  // the local_root_.
  TransformHandle GetRoot() const;

  // For validating properties associated with content in tests only. If |content_id| does not
  // exist for this Flatland instance, returns std::nullopt.
  std::optional<TransformHandle> GetContentHandle(ContentId content_id) const;

  // For validating properties associated with layer stacks in tests only. If |layer_stack_id| does
  // not exist for this Flatland instance, returns std::nullopt.
  std::optional<TransformHandle> GetLayerStackHandleForTest(LayerStackId layer_stack_id) const;

  // For validating properties associated with transforms in tests only. If |transform_id| does not
  // exist for this Flatland instance, returns std::nullopt.
  std::optional<TransformHandle> GetTransformHandle(TransformId transform_id) const;

  // For validating logs in tests only.
  void SetErrorReporter(std::unique_ptr<scenic_impl::ErrorReporter> error_reporter);

  // For using as a unique identifier in tests only.
  scheduling::SessionId GetSessionId() const;

  // Allow others to see how this Flatland session is configured.
  const FlatlandConfig& config() const { return config_; }

 private:
  Flatland(std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
           scheduling::SessionId session_id, std::shared_ptr<FlatlandPresenter> flatland_presenter,
           std::shared_ptr<LinkSystem> link_system,
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
           const FlatlandConfig& config);

  // `Flatland::New()` dispatches a task to invoke this.
  void Bind(fidl::ServerEnd<fuchsia_ui_composition::Flatland> server_end,
            std::function<void()> destroy_instance_function);

  void OnFidlClosed(fidl::UnbindInfo unbind_info);

  void ReportLinkProtocolError(const std::string& error_log);
  void CloseConnection(fuchsia_ui_composition::FlatlandError error);

  // Note: Any new CreateView function must use this helper function for it to have the same
  // test coverage as its siblings.
  void CreateViewHelper(
      fuchsia_ui_views::wire::ViewCreationToken token,
      fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher> parent_viewport_watcher,
      std::optional<fuchsia_ui_views::wire::ViewIdentityOnCreation> view_identity,
      std::optional<fuchsia_ui_composition::wire::ViewBoundProtocols> protocols);

  // Registers view-bound protocols (e.g. focuser, input sources) for |view_ref_koid|.
  // Returns true on success, or false if invalid protocol combinations were provided (in which
  // case CloseConnection() has already been called).
  bool RegisterViewBoundProtocols(fuchsia_ui_composition::wire::ViewBoundProtocols protocols,
                                  zx_koid_t view_ref_koid);

  // Sets clip bounds on the provided transform handle. Takes in TransformHandle and not
  // TransformID as a parameter so that it can be applied to content transforms that do
  // not have an external ID that they are mapped to.
  void SetClipBoundaryInternal(TransformHandle handle, TransformClipRegion bounds);

  // Called by `Clear()`.  Releases every Flatland2 object referenced by a client ID.
  // All IDs are immediately safe to reuse.
  void ClearFlatland2State();

  // For each dead transform:
  // 1) Remove the corresponding matrix
  // 2) If it hosts a layer stack, drop the stack and release its layers via `ReleaseLayerObject()`.
  void ProcessDeadTransforms(const TransformGraph::TopologyData& data);

  // Validates sample_rect extents for an image layer during Present().
  // Returns true if valid, otherwise logs an error via `error_reporter_` and returns false.
  bool IsLayerSampleRectValidForPresent(const LayerObject& layer_obj) const;

  // The dispatcher this Flatland instance is running on.
  async_dispatcher_t* dispatcher() const { return dispatcher_holder_->dispatcher(); }
  std::shared_ptr<utils::DispatcherHolder> dispatcher_holder_;

  // The unique SessionId for this Flatland session. Used to schedule Presents and register
  // UberStructs with the UberStructSystem.
  const scheduling::SessionId session_id_;

  // A Present2Helper to facilitate sendng the appropriate OnFramePresented() callback to FIDL
  // clients when frames are presented to the display.
  scheduling::Present2Helper present2_helper_;

  // A FlatlandPresenter shared between Flatland instances. Flatland uses this interface to get
  // PresentIds when publishing to the UberStructSystem.
  std::shared_ptr<FlatlandPresenter> flatland_presenter_;

  // A link system shared between Flatland instances, so that links can be made between them.
  std::shared_ptr<LinkSystem> link_system_;

  // An UberStructSystem shared between Flatland instances. Flatland publishes local data to the
  // UberStructSystem in order to have it seen by the global render loop.
  std::shared_ptr<UberStructSystem::UberStructQueue> uber_struct_queue_;

  // Used to import Flatland images to external services that Flatland does not have knowledge of.
  // Each importer is used for a different service.
  std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> buffer_collection_importers_;

  // True if there were errors in ParentViewportWatcher or ChildViewWatcher channels.
  bool link_protocol_error_ = false;

  // The number of Present() calls remaining before the client runs out. This value is potentially
  // incremented when OnNextFrameBegin() is called, and decremented by 1 for each Present() call.
  uint32_t present_credits_ = 1;

  // Used for client->Flatland present flow IDs.
  uint64_t present_count_ = 0;

  // Must be managed by a shared_ptr because the implementation uses weak_from_this().
  std::shared_ptr<escher::FenceQueue> fence_queue_ = std::make_shared<escher::FenceQueue>();

  // Pool allocator shared between `transforms_` and `content_handles_` to recycle their
  // identically-sized nodes and prevent heap churn. Must be declared above the maps.
  std::pmr::unsynchronized_pool_resource pool_;

  // A map from user-generated ID to global handle. This map constitutes the set of transforms that
  // can be referenced by the user through method calls. Keep in mind that additional transforms may
  // be kept alive through child references.
  std::pmr::unordered_map<TransformId, TransformHandle> transforms_;

  // A graph representing this flatland instance's local transforms and their relationships.
  TransformGraph transform_graph_;

  // A unique transform for this instance, the local_root_, is part of the transform_graph_,
  // and will never be released or changed during the course of the instance's lifetime. This makes
  // it a fixed attachment point for cross-instance Links.
  const TransformHandle local_root_;

  // The transform from the last call to SetRootTransform(). Unlike |local_root_|, this can change
  // over time.
  //
  // Initialize to an invalid handle.
  TransformHandle root_transform_ = TransformHandle(0, 0);

  // A mapping from user-generated ID to the TransformHandle that owns that piece of Content.
  // Attaching Content to a Transform consists of setting one of these "Content Handles" as the
  // priority child of the Transform.
  std::pmr::unordered_map<ContentId, TransformHandle> content_handles_;

  // A mapping from user-generated ID to the LayerHandle that owns that layer object.
  std::pmr::unordered_map<LayerId, LayerHandle> layer_handles_;
  // Supplies the session-unique suffix for new LayerHandles.
  uint64_t next_layer_handle_ = 1;

  // A mapping from user-generated `ImageId` to the internal `GlobalImageId`.
  // Erased the moment the client releases the id.
  std::pmr::unordered_map<ImageId, allocation::GlobalImageId> images2_;

  // Flatland2 layer state authored by this session, keyed by session-internal handles.
  // `layer_objects_` owns the layers; `layer_stacks_` maps a stack's content handle (its
  // attachment point in the transform graph) to the ordered list of layers it displays
  // (back-most first).  Both API versions populate these: Flatland2 sessions directly,
  // Flatland1 sessions through the facade, where each image or filled rect is a
  // single-layer stack.
  std::pmr::unordered_map<LayerHandle, LayerObject> layer_objects_;
  std::pmr::unordered_map<TransformHandle, LayerStackData> layer_stacks_;
  std::pmr::unordered_map<LayerStackId, TransformHandle> layer_stack_handles_;

  // Image resources, keyed by the never-reused GlobalImageId.  See ImageObject.
  std::pmr::unordered_map<allocation::GlobalImageId, ImageObject> image_objects_;

  // TODO(https://fxbug.dev/523371761): public for tests.  Later, revisit whether any can be
  // made private (if so, they'll be reordered in the file).
  // Consider using `friend class FlatlandTest`.
 public:
  LayerHandle CreateLayerObject();

  // Decrements `LayerObject` ref count, destroying it at zero.  When destroyed, any bound image is
  // released via `UnbindLayerImage()`.
  void ReleaseLayerObject(LayerHandle handle);
  TransformHandle CreateLayerStackData();

  // Replaces the stack's entire existing layer list with `layers`. This operation
  // adjusts each affected LayerObject's ref_count: +1 for each handle newly added
  // (per occurrence) and -1 for each handle removed from the stack (via ReleaseLayerObject).
  // A pure reorder is ref-count-neutral. Layers destroyed by the decrement follow
  // the normal ReleaseLayerObject path (bound image into the release machinery).
  // Every handle in `layers` must exist in layer_objects_.
  void SetLayerStackData(TransformHandle stack_handle, std::span<const LayerHandle> layers);
  void SetLayerStackData(TransformHandle stack_handle, std::initializer_list<LayerHandle> layers) {
    SetLayerStackData(stack_handle, std::span<const LayerHandle>(layers.begin(), layers.end()));
  }

  // Test-only accessor/mutators
  // TODO(https://fxbug.dev/523371761): once everything lands, verify whether these are necessary
  // to keep, or whether they are well-covered by tests for production callers such as
  // `CreateImage()` and `ProcessDeadTransforms()`.
  std::vector<allocation::GlobalImageId> CleanupFlatland2StateForTest(
      const std::vector<TransformHandle>& dead_handles);
  void SetLayerImageForTest(LayerHandle handle, allocation::GlobalImageId image);
  void SetLayerSolidColorForTest(LayerHandle handle);
  LayerObject* GetLayerObjectForTest(LayerHandle handle);
  const ImageObject* GetImageObjectForTest(allocation::GlobalImageId global_id) const;
  const LayerStackData* GetLayerStackDataForTest(TransformHandle handle);
  void ReleaseTransformForTest(TransformHandle handle);
  void SetPriorityChildForTest(TransformId parent, TransformHandle child);
  LayerHandle GetLayerHandleForTest(LayerId layer_id);
  size_t PendingImageReleaseCountForTest() const;
  allocation::GlobalImageId GetGlobalImageIdForTest(ImageId image_id) const;

 private:
  // The only place that `ImageObject::ref_count` is decremented.  At zero the object is erased,
  // but the image may still be on-screen, so it is not released yet.  Instead, the global image ID
  // is queued in `images_to_release_on_present_`, and the next `Present()` releases it once a
  // release fence signals (or `~Flatland()` does, if the session dies before presenting).
  void ReleaseImageObject(allocation::GlobalImageId id);

  // Clears any image bound to the layer, and calls `ReleaseImageObject()` on it.
  // No-op for a layer with no bound image.
  void UnbindLayerImage(LayerObject& layer);

  // Binds `id` to `layer`, replacing any current binding.  `id` must map to an existing image;
  // rebinding the same image is a no-op.  Manages ref-counts of incoming/outgoing images.
  // The incoming image's width/height are copied into the layer's `ImageModeProperties`.
  void BindLayerImage(LayerObject& layer, allocation::GlobalImageId id);

  // Releases `ids` through the buffer collection importers and forgets their
  // import tokens.  Called when a frame's release fence signals.
  void ReleaseImages(std::span<const allocation::GlobalImageId> ids);

  // TODO(https://fxbug.dev/523371761): after transition to Flatland2 UberStruct schema is complete,
  // revisit order of public/private sections, and verify "methods-first, fields-last" declaration
  // order (as mandated by style guide).

  // Shared image import helper for CreateImage and CreateImage2.
  std::optional<allocation::ImageMetadata> ImportImage(
      fuchsia_ui_composition::wire::BufferCollectionImportToken import_token, uint32_t vmo_index,
      const fuchsia_ui_composition::wire::ImageProperties& properties);

  // Return the `LayerObject` corresponding to `handle`, which must exist.
  // The returned reference remains valid until this element is erased.
  LayerObject& GetLayerObject(LayerHandle handle) {
    auto it = layer_objects_.find(handle);
    FX_CHECK(it != layer_objects_.end()) << "GetLayerObject() called with bad handle: " << handle;
    return it->second;
  }

  // Returns the LayerObject for the given stack's content handle, or nullptr if none exists.
  LayerObject* GetFacadeLayerObject(TransformHandle content_handle);

  // Helper to extract ImageModeProperties from a layer, returning nullptr if the layer does not
  // exist or does not contain ImageModeProperties.
  UberStructLayer::ImageModeProperties* GetFacadeLayerImageContent(TransformHandle content_handle);

  // Helper to extract SolidColorModeProperties from a layer, returning nullptr if the layer does
  // not exist or does not contain SolidColorModeProperties.
  UberStructLayer::SolidColorModeProperties* GetFacadeLayerSolidColorContent(
      TransformHandle content_handle);

  // The set of link operations that are pending a call to Present(). Unlike other operations,
  // whose effects are only visible when a new UberStruct is published, Link destruction operations
  // result in immediate changes in the LinkSystem. To avoid having these changes visible before
  // Present() is called, the actual destruction of Links happens in the following Present().
  std::vector<fit::function<void()>> pending_link_operations_;

  // Fences that signal when an image creation operation has completed.
  // These are appended to acquire fences in the next `Present()` to ensure
  // that the Present task waits for image import to complete.
  std::vector<zx::event> pending_create_image_fences_;

  // Wraps a LinkSystem::LinkToChild and the properties currently associated with that link.
  struct LinkToChildData {
    LinkSystem::LinkToChild link;
    fuchsia_ui_composition::ViewportProperties properties;
  };

  // A mapping from Flatland-generated TransformHandle to the LinkToChildData it represents.
  std::unordered_map<TransformHandle, LinkToChildData> links_to_children_;

  // The link from this Flatland instance to our parent.
  std::optional<LinkSystem::LinkToParent> link_to_parent_;

  // Instance name from SetDebugName().
  std::string debug_name_;

  // Represents a geometric transformation as three separate components applied in the following
  // order: translation (relative to the parent's coordinate space), orientation (around the new
  // origin as defined by the translation), and scale (relative to the new rotated origin).
  class MatrixData {
   public:
    void SetTranslation(fuchsia_math::wire::Vec translation);
    void SetOrientation(fuchsia_ui_composition::Orientation orientation);
    void SetScale(fuchsia_math::wire::VecF scale);

    // Returns this geometric transformation as a single 3x3 matrix using the order of operations
    // above: translation, orientation, then scale.
    glm::mat3 GetMatrix() const;

    static float GetOrientationAngle(fuchsia_ui_composition::Orientation orientation);

   private:
    // Applies the translation, then orientation, then scale to the identity matrix.
    void RecomputeMatrix();

    glm::vec2 translation_ = glm::vec2(0.f, 0.f);
    glm::vec2 scale_ = glm::vec2(1.f, 1.f);

    // Counterclockwise rotation angle, in radians.
    float angle_ = 0.f;

    // Recompute and cache the local matrix each time a component is changed to avoid recomputing
    // the matrix for each frame. We expect GetMatrix() to be called far more frequently (roughly
    // once per rendered frame) than the setters are called.
    glm::mat3 matrix_ = glm::mat3(1.f);
  };

  // A geometric transform for each TransformHandle. If not present, that TransformHandle has the
  // identity matrix for its transform.
  std::unordered_map<TransformHandle, MatrixData> matrices_;

  // A map of transform handles to opacity values where the values are strictly in the range
  // [0.f,1.f). 0.f is completely transparent and 1.f is completely opaque. If there is no explicit
  // value associated with a transform handle in this map, then Flatland will consider it to be
  // 1.f by default.
  std::unordered_map<TransformHandle, float> opacity_values_;

  // A map of transform handles to clip regions, where each clip region is a rect to which
  // all child nodes of the transform handle have their rectangular views clipped to.
  std::unordered_map<TransformHandle, TransformClipRegion> clip_regions_;

  // A map of transform handles to hit regions. Each transform's set of hit regions indicate which
  // parts of the transform are user-interactive.
  std::unordered_map<TransformHandle, std::vector<flatland::HitRegion>> hit_regions_;

  // Error reporter used for printing debug logs.
  std::unique_ptr<scenic_impl::ErrorReporter> error_reporter_;

  // One frame's images-to-be-released, waiting on that frame's release fence.
  // `Present()` populates a `PendingImageRelease` struct from the images accumulated in
  // `images_to_release_on_present_` since the last `Present()`.
  //
  // Safety: owned by the session: `~WaitOnce()` cancels a pending wait synchronously,
  // so no handler can run after `~Flatland()`.  Lives in a `std::pmr::list` because
  // `async::WaitOnce` is not movable and needs a stable address.
  struct PendingImageRelease {
    PendingImageRelease(zx::event fence_in, std::pmr::vector<allocation::GlobalImageId> ids_in)
        : fence(std::move(fence_in)), wait(fence.get(), ZX_EVENT_SIGNALED), ids(std::move(ids_in)) {
      FX_CHECK(!ids.empty()) << "PendingImageRelease with no images";
    }

    zx::event fence;  // declared first: `wait` references its handle
    async::WaitOnce wait;
    std::pmr::vector<allocation::GlobalImageId> ids;
  };
  std::pmr::list<PendingImageRelease> pending_image_releases_;

  // Images whose last ref was dropped since the previous `Present()`.
  // `Present()` moves the contents into a `PendingImageRelease` record for that frame.
  std::pmr::vector<allocation::GlobalImageId> images_to_release_on_present_;

  // Keeps the BufferCollectionImportToken alive for each active image. Dropping these tokens
  // triggers garbage collection of the associated BufferCollection in the Allocator. We keep them
  // here so that their lifetime is tied to the lifecycle of the Image resources themselves,
  // preventing the BufferCollection from being destroyed while asynchronous image imports are still
  // running.
  std::shared_ptr<std::unordered_map<allocation::GlobalImageId,
                                     fuchsia_ui_composition::wire::BufferCollectionImportToken>>
      import_tokens_;

  // Tracks API calls which have the potential to modify the view tree.  If true, the next-presented
  // frame will trigger view tree recomputation.
  bool view_tree_dirty_ = false;

  // Callbacks for registering View-bound protocols.
  fit::function<void(fidl::ServerEnd<fuchsia_ui_views::Focuser>, zx_koid_t)> register_view_focuser_;
  fit::function<void(fidl::ServerEnd<fuchsia_ui_views::ViewRefFocused>, zx_koid_t)>
      register_view_ref_focused_;
  fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::TouchSource>, zx_koid_t)>
      register_touch_source_;
  fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::MouseSource>, zx_koid_t)>
      register_mouse_source_;
  fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2>, zx_koid_t)>
      register_touch_source_v2_;
  fit::function<void(fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2>, zx_koid_t)>
      register_mouse_source_v2_;

  // The configuration for this Flatland instance.
  const FlatlandConfig config_;

  // Helper class that is responsible for managing the Flatland instance's FIDL connection, and also
  // provides an RAII approach to guaranteeing that `destroy_instance_function_` is invoked.
  class BindingData {
   public:
    BindingData(Flatland* flatland, async_dispatcher_t* dispatcher,
                fidl::ServerEnd<fuchsia_ui_composition::Flatland> server_end,
                std::function<void()> destroy_instance_function);

    // RAII: invokes `destroy_instance_function_`.
    ~BindingData();

    // Send `OnFramePresented` FIDL event to client.
    void SendOnFramePresented(fuchsia_scenic_scheduling::FramePresentedInfo info);

    // Send `OnNextFrameBegin` FIDL event to client.
    void SendOnNextFrameBegin(uint32_t additional_present_credits,
                              FuturePresentationInfos presentation_infos);

    void CloseConnection(fuchsia_ui_composition::FlatlandError error);

   private:
    // The FIDL binding for this Flatland instance, which references |this| as the implementation
    // and run on |dispatcher_|.
    fidl::ServerBinding<fuchsia_ui_composition::Flatland> binding_;

    // A function that, when called, will destroy this instance. Necessary because an async::Wait
    // can/ only wait on peer channel destruction, not "this" channel destruction, so the
    // FlatlandManager cannot detect if this instance closes |binding_|.
    std::function<void()> destroy_instance_function_;
  };

  std::unique_ptr<BindingData> binding_data_;

  // Flatland thread's executor.
  async::Executor executor_;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_H_
