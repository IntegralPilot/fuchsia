// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTING_UTIL_PORTABLE_UI_TEST_H_
#define SRC_UI_TESTING_UTIL_PORTABLE_UI_TEST_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.scene/cpp/fidl.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <zircon/status.h>

#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <src/lib/testing/loop_fixture/real_loop_fixture.h>
#include <src/ui/testing/util/logging_event_loop.h>
#include <src/ui/testing/util/screenshot_helper.h>

namespace ui_testing {

using component_testing::Route;
using fuchsia_ui_composition::ScreenshotFormat;
using ChildName = std::string;

class PortableUITest : public LoggingEventLoop, public ::testing::Test {
 public:
  static constexpr auto kTestUIStack = "ui";
  static constexpr auto kTestUIStackRef = component_testing::ChildRef{kTestUIStack};

  PortableUITest() : realm_builder_(component_testing::RealmBuilder::Create()) {}
  explicit PortableUITest(component_testing::RealmBuilder realm_builder)
      : realm_builder_(std::move(realm_builder)) {}

  void SetUp() override;
  void TearDown() override;

  // Attaches a client view to the scene, and waits for it to render.
  void LaunchClient();

  // Attaches a client view that embeds a child view to the scene, and waits for
  // both to render.
  void LaunchClientWithEmbeddedView();

  // Returns when a view has fully connected to the scene.
  void WaitForViewPresentation();

  // Returns true when the specified view is fully connected to the scene AND
  // has presented at least one frame of content.
  bool HasViewConnected(zx_koid_t view_ref_koid);

  // Helper method to take a screenshot.
  Screenshot TakeScreenshot(ScreenshotFormat format = ScreenshotFormat::kBgraRaw);

  // Helper method to take a screenshot until predicate is true. Returns false if
  // |predicate_timeout| is reached.
  bool TakeScreenshotUntil(fit::function<bool(const ui_testing::Screenshot&)> predicate,
                           zx::duration predicate_timeout, zx::duration step = zx::msec(10),
                           ScreenshotFormat format = ScreenshotFormat::kBgraRaw);

  // Return display size by connecting to |fuchsia::ui::display::singleton::Info| protocol.
  fuchsia_math::SizeU display_size();

  // Return display height by connecting to |fuchsia::ui::display::singleton::Info| protocol.
  uint32_t display_height();

  // Return display width by connecting to |fuchsia::ui::display::singleton::Info| protocol.
  uint32_t display_width();

  // Registers a fake touch screen device with an injection coordinate space
  // spanning [-1000, 1000] on both axes.
  void RegisterTouchScreen();

  // Simulates a tap at location (x, y).
  void InjectTap(int32_t x, int32_t y);

  // Injects an input event, and posts a task to retry after `kTapRetryInterval`.
  //
  // We post the retry task because the first input event we send to Flutter may be lost.
  // The reason the first event may be lost is that there is a race condition as the scene
  // owner starts up.
  //
  // More specifically: in order for our app
  // to receive the injected input, two things must be true before we inject touch input:
  // * The Scenic root view must have been installed, and
  // * The Input Pipeline must have received a viewport to inject touch into.
  //
  // The problem we have is that the `is_rendering` signal that we monitor only guarantees us
  // the view is ready. If the viewport is not ready in Input Pipeline at that time, it will
  // drop the touch event.
  //
  // TODO(https://fxbug.dev/42179163): Improve synchronization and remove retry logic.
  void InjectTapWithRetry(int32_t x, int32_t y);

  // Injects a swipe from the given starting location to the given end location
  // in injector coordinate space.
  void InjectSwipe(int start_x, int start_y, int end_x, int end_y, int move_event_count);

  // Injects a touch event.
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  void InjectTouchEvent(fuchsia_ui_test_input::TouchInputReport report);
#else
  void InjectTouchEvent(fuchsia_input_report::TouchInputReport report);
#endif

  // Registers a fake mouse device, for which mouse movement is measured on a
  // scale of [-1000, 1000] on both axes and scroll is measured from [-100, 100]
  // on both axes.
  void RegisterMouse();

  // Helper method to simulate combinations of button presses/releases and/or
  // mouse movements.
  void SimulateMouseEvent(const std::vector<fuchsia_ui_test_input::MouseButton>& pressed_buttons,
                          int movement_x, int movement_y);

  // Helper method to simulate a mouse scroll event.
  //
  // Set `use_physical_units` to true to specify scroll in physical pixels and
  // false to specify scroll in detents.
  void SimulateMouseScroll(const std::vector<fuchsia_ui_test_input::MouseButton>& pressed_buttons,
                           int scroll_x, int scroll_y, bool use_physical_units = false);

  // Register a fake keyboard.
  void RegisterKeyboard();

  // Helper method to simulate a string in us ascii to the fake keyboard.
  void SimulateUsAsciiTextEntry(const std::string& str);

 protected:
  component_testing::RealmBuilder& realm_builder() { return realm_builder_; }
  std::optional<component_testing::RealmRoot>& realm_root() { return realm_; }

  const std::optional<zx_koid_t>& client_root_view_ref_koid() { return client_root_view_ref_koid_; }

  int touch_injection_request_count() const { return touch_injection_request_count_; }

  // Methods to control the test-ui-stack parameters. Override as necessary.
  virtual float device_pixel_ratio() { return 1.f; }

  // `display_width_px()` and `display_height_px()` must be either both zero
  // or both non-zero.
  virtual uint32_t display_width_px() { return 0; }
  virtual uint32_t display_height_px() { return 0; }
  virtual uint32_t display_refresh_rate_millihertz() { return 0; }
  virtual uint32_t display_rotation() { return 0; }
  virtual bool suspend_enabled() { return false; }

 private:
  void SetUpRealmBase();

  // Configures the test-specific component topology.
  virtual void ExtendRealm() {}

  // Subclass should implement these methods to add eager components to the base test realm.
  // This is useful for components that connect to the Scene graph using
  // fuchsia.element.GraphicalPresenter/PresentView.
  virtual std::vector<std::pair<ChildName, std::string>> GetEagerTestComponents() { return {}; }

  // Subclass should implement these methods to add components to the base test realm.
  virtual std::vector<std::pair<ChildName, std::string>> GetTestComponents() { return {}; }

  // Subclass should implement this method to add capability routes to the test realm.
  // This is to provide routing for the components added via `GetEagerTestComponents`,
  // `GetTestComponents`, or `ExtendRealm`.
  virtual std::vector<Route> GetTestRoutes() { return {}; }

  // Returns the test-ui-stack component url to use in this test.
  virtual std::string GetTestUIStackUrl() = 0;

  // Helper method to set up the scene provider.
  void SetUpSceneProvider();

  // Helper method to register ViewTreeWatcher to scene_provider.
  void RegisterViewTreeWatcher();

  // Helper method to watch watch for view geometry updates.
  void WatchViewGeometry();

  // Helper method to process a view geometry update.
  void ProcessViewGeometryResponse(fuchsia_ui_observation_geometry::WatchResponse response);

  // Helper to connect input registry.
  void ConnectInputRegistry();

  fidl::SyncClient<fuchsia_ui_test_input::Registry> input_registry_;
  fidl::SyncClient<fuchsia_ui_test_input::TouchScreen> fake_touchscreen_;
  fidl::SyncClient<fuchsia_ui_test_input::Mouse> fake_mouse_;
  fidl::SyncClient<fuchsia_ui_test_input::Keyboard> fake_keyboard_;
  fidl::SyncClient<fuchsia_ui_test_scene::Controller> scene_provider_;
  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> view_tree_watcher_;
  fidl::SyncClient<fuchsia_ui_composition::Screenshot> screenshotter_;

  component_testing::RealmBuilder realm_builder_;
  std::optional<component_testing::RealmRoot> realm_;

  // Counts the number of completed requests to inject touch reports into input
  // pipeline.
  int touch_injection_request_count_ = 0;

  // The KOID of the client root view's `ViewRef`.
  std::optional<zx_koid_t> client_root_view_ref_koid_;

  // Holds the display size.
  std::optional<fuchsia_math::SizeU> display_size_;

  // Holds the most recent view tree snapshot received from the view tree
  // watcher.
  //
  // From this snapshot, we can retrieve relevant view tree state on demand,
  // e.g. if the client view is rendering content.
  std::optional<fuchsia_ui_observation_geometry::ViewTreeSnapshot> last_view_tree_snapshot_;

  // The typical latency on devices we've tested is ~60 msec. The retry interval is chosen to be
  // a) Long enough that it's unlikely that we send a new tap while a previous tap is still being
  //    processed. That is, it should be far more likely that a new tap is sent because the first
  //    tap was lost, than because the system is just running slowly.
  // b) Short enough that we don't slow down tryjobs.
  //
  // The first property is important to avoid skewing the latency metrics that we collect.
  // For an explanation of why a tap might be lost, see the documentation for TryInject().
  static constexpr auto kTapRetryInterval = zx::sec(1);

  // PortableUITest should stop view_tree_watcher_ and ignore errors from the watcher when test tear
  // down begins.
  bool begin_tear_down_ = false;
};

}  // namespace ui_testing

#endif  // SRC_UI_TESTING_UTIL_PORTABLE_UI_TEST_H_
