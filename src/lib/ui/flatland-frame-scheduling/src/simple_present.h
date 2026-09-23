// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UI_FLATLAND_FRAME_SCHEDULING_SRC_SIMPLE_PRESENT_H_
#define SRC_LIB_UI_FLATLAND_FRAME_SCHEDULING_SRC_SIMPLE_PRESENT_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/channel.h>

#include <memory>
#include <queue>

namespace simple_present {
using OnFramePresentedCallback = fit::function<void(zx_time_t actual_presentation_time)>;
using OnErrorCallback = fit::function<void()>;

// This class is meant to help clients use the Flatland Present API correctly.
class FlatlandConnection final : public fidl::AsyncEventHandler<fuchsia_ui_composition::Flatland> {
 public:
  ~FlatlandConnection();
  FlatlandConnection(const FlatlandConnection&) = delete;
  FlatlandConnection& operator=(const FlatlandConnection&) = delete;

  // Creates a flatland connection using component::Connect.
  static std::unique_ptr<FlatlandConnection> Create(async::Loop* loop,
                                                    const std::string& debug_name);
  // Creates a flatland connection using the provided client end.
  static std::unique_ptr<FlatlandConnection> Create(
      async_dispatcher_t* dispatcher, fidl::ClientEnd<fuchsia_ui_composition::Flatland> flatland,
      const std::string& debug_name);

  fidl::Client<fuchsia_ui_composition::Flatland>& FlatlandClient();

  // fidl::AsyncEventHandler
  void on_fidl_error(fidl::UnbindInfo error) override;
  // fidl::AsyncEventHandler
  void OnError(fidl::Event<fuchsia_ui_composition::Flatland::OnError>& error) override;
  // fidl::AsyncEventHandler
  void OnNextFrameBegin(
      fidl::Event<fuchsia_ui_composition::Flatland::OnNextFrameBegin>& e) override;
  // fidl::AsyncEventHandler
  void OnFramePresented(
      fidl::Event<fuchsia_ui_composition::Flatland::OnFramePresented>& e) override;

  void SetErrorCallback(OnErrorCallback callback);

  // Safe attempt to Present(). It goes through with default present args if present credits are
  // available.
  void Present();

  // This version of Present can be readily used for steady-state rendering. Inside |callback|
  // clients may process any input, submit Flatland commands, and finally re-Present(), perpetuating
  // the loop.
  void Present(fuchsia_ui_composition::PresentArgs present_args, OnFramePresentedCallback callback);

 private:
  FlatlandConnection(async_dispatcher_t* dispatcher,
                     fidl::ClientEnd<fuchsia_ui_composition::Flatland> flatland,
                     const std::string& debug_name);

  fidl::Client<fuchsia_ui_composition::Flatland> flatland_;
  uint32_t present_credits_ = 1;

  struct PendingPresent {
    PendingPresent(fuchsia_ui_composition::PresentArgs present_args,
                   OnFramePresentedCallback callback);
    ~PendingPresent();

    PendingPresent(PendingPresent&& other);
    PendingPresent& operator=(PendingPresent&& other);

    fuchsia_ui_composition::PresentArgs present_args;
    OnFramePresentedCallback callback;
  };
  std::queue<PendingPresent> pending_presents_;
  std::vector<zx::event> previous_present_release_fences_;
  std::queue<OnFramePresentedCallback> presented_callbacks_;
  OnErrorCallback error_callback_;
};

}  // namespace simple_present

#endif  // SRC_LIB_UI_FLATLAND_FRAME_SCHEDULING_SRC_SIMPLE_PRESENT_H_
