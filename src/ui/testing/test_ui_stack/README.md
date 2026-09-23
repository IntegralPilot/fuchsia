# Test UI Stack

## Overview

The Test UI Stack is a Fuchsia package that leverages the
[UITestRealm](../ui_test_realm/README.md) library to streamline UI realm setup
for tests that depend on or interoperate with a UI layer, but that may not be
concerned with the details of setting up that UI layer.

The Test UI Stack provides UI-specific testing capabilities to test clients both in-
and out-of-tree. See the
[Test UI Stack RFC](///docs/contribute/governance/rfcs/0180_test_ui_stack.md)
for more details.

## Scene Setup

The Test UI Stack always creates a Scene Manager-owner scene with Scenic,
accessibility protocols through a Fake A11y Manager, and production-like input
support through the Input Pipeline library (integrated as a part of Scene Manager).

## Configurability

The Test UI Stack supports configuring the underlying UITestRealm instance
using the following arguments via Structured Configuration:

- **display_rotation**: The rotation of the display, counter-clockwise, in 90-degree increments.
- **display_width_px**: Width of the display, in pixels.
- **display_height_px**: Height of the display, in pixels.
- **display_refresh_rate_millihertz**: Refresh rate of the display, in millihertz (1/1000 Hz).
- **device_pixel_ratio**: The conversion factor between logical and physical coordinates.

## Example Usage

The pseudo-C++ snippet below outlines a basic touch input test using the Test UI
Stack component.

```
// Client test code creates a RealmBuilder instance.
component_testing::RealmBuilder realm_builder;

// Instantiate Test UI Stack component by absolute URL in the test realm.
realm_builder.AddChild("test-ui-stack",
            "fuchsia-pkg://fuchsia.com/test-ui-stack#meta/test-ui-stack.cm");

// Add a test view component to the test realm, and route required UI services
// to it.
realm_builder.AddChild("test-view", ...);
realm_builder.AddRoute({
    .capabilities = {Protocol{fuchsia::ui::scenic::Scenic::Name_}},
    .source = ChildRef{"test-ui-stack"},
    .targets = {"test-view"}});

// Expose fuchsia.ui.app.ViewProvider from the test view.
realm_builder.AddRoute({
    .capabilities = {Protocol{fuchsia::ui::app::ViewProvider::Name_}},
    .source = ChildRef{"test-view"},
    .targets = {ParentRef()}});

// Build the test realm.
RealmRoot realm_root = realm_builder_.Build();

// Connect to the scene provider "helper service", and request to attach a
// test view to the scene.
std::optional<zx_koid_t> client_view_ref_koid;
fuchsia::ui::observation::geometry::Provider geometry_provider;
auto scene_provider = realm_root->Connect<fuchsia::ui::test::scene::Provider>();
auto view_provider = realm_root_->Connect<fuchsia::ui::app::ViewProvider>();
scene_provider->AttachView(std::move(view_provider), geometry_provider.NewRequest(),
  [&client_view_ref_koid](auto view_ref_koid) {
    // Save the client's ViewRef koid.
    client_view_ref_koid = view_ref_koid;
  });

// Wait for client view ref koid to become available.
RunLoopUntil([&client_view_ref_koid] {
  return client_view_ref_koid.has_value();
});

// Use registered geometry provider to wait for client view to render.
ASSERT_TRUE(geometry_provider.is_bound());
geometry_provider.Watch(...);
RunLoopUntil(...);

// Connect to input synthesis helper service, and use to inject input.
auto input_synthesis = realm_root->Connect<fuchsia::ui::test::input::Touch>();
input_synthesis->InjectTap(...);
```
