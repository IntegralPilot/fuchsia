// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/test_base.h>
#include <lib/component/incoming/cpp/constants.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/directory.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/zx/time.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/lib/testing/predicates/status.h"
#include "src/lib/vulkan/flatland_view/flatland_view.h"

namespace {

constexpr uint32_t kWidth = 100;
constexpr uint32_t kHeight = 50;

class FakeFlatland : public fidl::testing::TestBase<fuchsia_ui_composition::Flatland>,
                     public fidl::testing::TestBase<fuchsia_ui_composition::ParentViewportWatcher> {
 public:
  // `dispatcher` must be non-null.
  explicit FakeFlatland(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
    ZX_DEBUG_ASSERT(dispatcher != nullptr);
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {}

  fidl::ProtocolHandler<fuchsia_ui_composition::Flatland> GetHandler(
      async_dispatcher_t* dispatcher) {
    return [this, dispatcher](fidl::ServerEnd<fuchsia_ui_composition::Flatland> request) {
      flatland_bindings_.AddBinding(dispatcher, std::move(request), this,
                                    fidl::kIgnoreBindingClosure);
    };
  }

  // `fuchsia_ui_composition::Flatland`:
  void CreateView2(CreateView2Request& request, CreateView2Completer::Sync& completer) override {
    parent_viewport_watcher_bindings_.AddBinding(dispatcher_,
                                                 std::move(request.parent_viewport_watcher()), this,
                                                 fidl::kIgnoreBindingClosure);
  }

  // `fuchsia_ui_composition::ParentViewportWatcher`:
  void GetLayout(GetLayoutCompleter::Sync& completer) override {
    fuchsia_ui_composition::LayoutInfo info = {{
        .logical_size = fuchsia_math::SizeU{{.width = kWidth, .height = kHeight}},
    }};
    completer.Reply({{.info = std::move(info)}});
  }

 private:
  fidl::ServerBindingGroup<fuchsia_ui_composition::Flatland> flatland_bindings_;
  fidl::ServerBindingGroup<fuchsia_ui_composition::ParentViewportWatcher>
      parent_viewport_watcher_bindings_;

  async_dispatcher_t* const dispatcher_;
};

}  // namespace

class FlatlandViewTest : public gtest::TestLoopFixture {
 public:
  void SetUp() override {
    TestLoopFixture::SetUp();
    fake_flatland_ = std::make_unique<FakeFlatland>(dispatcher());

    zx::result<> add_protocol_result =
        flatland_outgoing_.AddUnmanagedProtocol<fuchsia_ui_composition::Flatland>(
            fake_flatland_->GetHandler(dispatcher()));
    ASSERT_OK(add_protocol_result);

    auto [root_dir_client, root_dir_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    zx::result<> serve_result = flatland_outgoing_.Serve(std::move(root_dir_server));
    ASSERT_OK(serve_result);

    auto [svc_dir_client, svc_dir_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    zx_status_t open_status = fdio_open3_at(
        /*directory=*/root_dir_client.channel().get(), /*path=*/component::kServiceDirectory,
        /*flags=*/uint64_t{fuchsia_io::Flags::kProtocolDirectory | fuchsia_io::kPermReadable},
        /*request=*/std::move(svc_dir_server).TakeChannel().release());
    ASSERT_OK(open_status);

    view_incoming_ = std::move(svc_dir_client);
  }

  component::OutgoingDirectory flatland_outgoing_{dispatcher()};
  fidl::ClientEnd<fuchsia_io::Directory> view_incoming_;
  std::unique_ptr<FakeFlatland> fake_flatland_;
  std::unique_ptr<FlatlandView> view_;
  uint32_t width_ = 0;
  uint32_t height_ = 0;
};

TEST_F(FlatlandViewTest, Initialize) {
  FlatlandView::ResizeCallback resize_callback = [this](uint32_t width, uint32_t height) {
    width_ = width;
    height_ = height;
    QuitLoop();
  };

  zx::eventpair view_token_0, view_token_1;
  EXPECT_EQ(ZX_OK, zx::eventpair::create(0, &view_token_0, &view_token_1));

  auto [view_token, viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto view = FlatlandView::Create(view_incoming_.borrow(), std::move(view_token),
                                   std::move(resize_callback), dispatcher());
  ASSERT_TRUE(view);

  EXPECT_EQ(0.0, width_);
  EXPECT_EQ(0.0, height_);
  RunLoopFor(zx::sec(3));

  EXPECT_EQ(kWidth, width_);
  EXPECT_EQ(kHeight, height_);
}
