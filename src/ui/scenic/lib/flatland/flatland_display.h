// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_DISPLAY_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_DISPLAY_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>

#include <functional>
#include <memory>

#include "src/ui/scenic/lib/display/display.h"
#include "src/ui/scenic/lib/flatland/flatland_presenter.h"
#include "src/ui/scenic/lib/flatland/link_system.h"
#include "src/ui/scenic/lib/flatland/transform_graph.h"
#include "src/ui/scenic/lib/flatland/transform_handle.h"
#include "src/ui/scenic/lib/flatland/uber_struct_system.h"
#include "src/ui/scenic/lib/utils/dispatcher_holder.h"
#include "src/ui/scenic/lib/utils/object_linker.h"

namespace flatland {

// FlatlandDisplay implements the FIDL API of the same name.  It is the glue between a physical
// display and a tree of Flatland content attached underneath.
class FlatlandDisplay : public fidl::Server<fuchsia_ui_composition::FlatlandDisplay>,
                        public std::enable_shared_from_this<FlatlandDisplay> {
 public:
  // Creates and binds a new `FlatlandDisplay` to the provided `server_end` channel on the
  // dispatcher thread.
  static std::shared_ptr<FlatlandDisplay> New(
      std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
      fidl::ServerEnd<fuchsia_ui_composition::FlatlandDisplay> server_end,
      scheduling::SessionId session_id, std::shared_ptr<display::Display> display,
      std::function<void()> destroy_display_function,
      std::shared_ptr<FlatlandPresenter> flatland_presenter,
      std::shared_ptr<LinkSystem> link_system,
      std::shared_ptr<UberStructSystem::UberStructQueue> uber_struct_queue);

  // Because this object captures its "this" pointer in internal closures, it is unsafe to copy or
  // move it. Disable all copy and move operations.
  FlatlandDisplay(const FlatlandDisplay&) = delete;
  FlatlandDisplay& operator=(const FlatlandDisplay&) = delete;
  FlatlandDisplay(FlatlandDisplay&&) = delete;
  FlatlandDisplay& operator=(FlatlandDisplay&&) = delete;

  ~FlatlandDisplay() override;

  // `fuchsia_ui_composition::FlatlandDisplay`
  void SetContent(SetContentRequest& request, SetContentCompleter::Sync& completer) override;
  void SetContent(fuchsia_ui_views::ViewportCreationToken token,
                  fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> child_view_watcher);

  // `fuchsia_ui_composition::FlatlandDisplay`
  void SetDevicePixelRatio(SetDevicePixelRatioRequest& request,
                           SetDevicePixelRatioCompleter::Sync& completer) override;
  void SetDevicePixelRatio(fuchsia_math::VecF device_pixel_ratio);

  // Binds the FIDL ServerEnd on the dispatcher thread.
  // Must not be called while there's an active FIDL connection.
  void Bind(fidl::ServerEnd<fuchsia_ui_composition::FlatlandDisplay> server_end);

  TransformHandle root_transform() const { return root_transform_; }
  display::Display* display() const { return display_.get(); }

  scheduling::SessionId session_id() const { return session_id_; }

 private:
  FlatlandDisplay(std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
                  scheduling::SessionId session_id, std::shared_ptr<display::Display> display,
                  std::function<void()> destroy_display_function,
                  std::shared_ptr<FlatlandPresenter> flatland_presenter,
                  std::shared_ptr<LinkSystem> link_system,
                  std::shared_ptr<UberStructSystem::UberStructQueue> uber_struct_queue);

  void OnFidlClosed(fidl::UnbindInfo unbind_info);

  // The dispatcher this Flatland display is running on.
  async_dispatcher_t* dispatcher() const { return dispatcher_holder_->dispatcher(); }
  std::shared_ptr<utils::DispatcherHolder> dispatcher_holder_;

  // The FIDL binding for this FlatlandDisplay, which references `this` as the implementation and
  // run on `dispatcher_`.
  std::optional<fidl::ServerBinding<fuchsia_ui_composition::FlatlandDisplay>> binding_;

  // The unique SessionId for this FlatlandDisplay. Used to schedule Presents and register
  // UberStructs with the UberStructSystem.
  const scheduling::SessionId session_id_;

  // Physical display that this FlatlandDisplay connects to a tree of Flatland content.
  const std::shared_ptr<display::Display> display_;

  // A function that, when called, will destroy this display.
  std::function<void()> destroy_display_function_;

  // A FlatlandPresenter shared between Flatland sessions. Flatland uses this interface to get
  // PresentIds when publishing to the UberStructSystem.
  std::shared_ptr<FlatlandPresenter> flatland_presenter_;

  // A link system shared between Flatland instances, so that links can be made between them.
  const std::shared_ptr<LinkSystem> link_system_;

  // An UberStructSystem shared between Flatland instances. Flatland publishes local data to the
  // UberStructSystem in order to have it seen by the global render loop.
  const std::shared_ptr<UberStructSystem::UberStructQueue> uber_struct_queue_;

  TransformGraph transform_graph_;

  const TransformHandle root_transform_;

  std::optional<LinkSystem::LinkToChild> link_to_child_;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_DISPLAY_H_
