# Introduction

Carnelian is a prototype framework for writing interactive Fuchsia components in
Rust.  It can display graphics either via Scenic's `fuchsia.ui.composition.*`
APIs, or by communicating directly with `fuchsia.hardware.display.Coordinator`.

# Building

To build the included samples, use the fx set line below to build the
workbench version of Fuchsia with the necessary additional packages to run
with Scenic.

```
    # Can also use `core.x64`, etc.  Not all products will support running in
    # Scenic and/or direct-display mode; see below.
    fx set workbench_eng.x64 \
        --with //src/lib/ui/carnelian:examples \
        --with //src/lib/ui/carnelian:carnelian-integration-test \
        --with //src/lib/ui/carnelian:carnelian-fb-integration-test \
        --with //src/lib/ui/carnelian:carnelian-tests \
        --with //src/lib/ui/carnelian:carnelian-layout-tests \
        --release \
        --args=rust_cap_lints='"warn"' \
        --cargo-toml-gen
```

## Configuring View Mode

Carnelian apps are configured to run in one of three view modes:
- `direct`: avoid Scenic and talk directly to the display coordinator
- `hosted`: display via `Flatland` API.
- `auto`: try `direct` first and fall back to `hosted`
  - this is the default

To use `direct` instead of `auto`, set the global build argument
`use_direct_for_carnelian_examples`.  If you're writing your own Carnelian app,
then you can add a dependency on `//src/lib/ui/carnelian:direct_config` (or do
something similar, but customized to your needs).

# Running using Scenic mode

To run the examples, use `ffx session add`.

Important: this requires a product that *has* a session.  For example, this
won't work with a `core.x64` build.

The basic workflow is:

1. `fx serve`
2. `ffx session add fuchsia-pkg://fuchsia.com/spinning-square-rs#meta/spinning-square-rs.cm`

To run a different example, replace the Fuchsia package URL with the desired
example; see the list of available examples below.

# Running using Direct Display mode

To run in direct display mode, the examples must be run in a realm that routes
the necessary capabilities (e.g. the `fuchsia.hardware.display.Service`
service).  The example component can be run as a "system test", as described
below.

The basic workflow is:
1. If your product includes a session, you must first stop the session and
Scenic: `ffx session stop; ffx component stop core/ui/scenic`.
2. `ffx component create /core/test_manager/system-tests:carnelian-square-rs fuchsia-pkg://fuchsia.com/spinning-square-rs#meta/spinning-square-rs.cm`
3. `ffx component start /core/test_manager/system-tests:carnelian-square-rs`

In `system-tests:carnelian-square-rs`, `system-tests` specifies the collection
where the component will be created, and therefore specifies which capabilities
will be routed.  `carnelian-square-rs` is an arbitrary name; if calling it
`foobar` makes you happier, do that.

To run a different example, replace the Fuchsia package URL with the desired
example; see the list of available examples below.

## Troubleshooting

If `/core/test_manager/system-tests` is unavailable, you're probably not running
on an eng build.  This collection is provided by `//src/sys/testing`, which is
included in `//bundles/assembly:testing_support`, which is included in eng builds.

The display coordinator only supports a single client.  If another client
is already connected, the Carnelian example will not be able to.  This is
typically because Scenic is running, but it is possible that your configuration
has another client that has claimed the display.  For example, you cannot run
a second Carnelian example without first killing the existing one.

# List of Examples

The examples directory contains a set of Carnelian example programs.  These are
described below, along with the the URLs that can be used to run them.

## Layout-based Examples

These examples demonstrate using scenes, facets and groups to layout user-interface.

### button

Package url: `fuchsia-pkg://fuchsia.com/button-rs#meta/button-rs.cm`

This example implements a single button which, when pressed, toggles a rectangle from red to green.

The 'M' key on an attached keyboard will cycle the main axis alignment of the row containing the
indicators. Pressing 'C' will cycle the cross axis alignment of that row. Pressing 'R' will cycle
the main alignment of the column containing both the button and the indicators.

### layout_gallery

Package url: `fuchsia-pkg://fuchsia.com/layout-gallery-rs#meta/layout-gallery-rs.cm`

This example is a gallery of the existing group layouts as well as housing tests for those layouts.

The 'M' key on an attached keyboard will cycle between layouts modes of stack, flex and button,
where the button layout is a test version of the layout used by the button example.

For stack, hitting the '1' key cycles the alignment of the stack.

For flex, hitting the '1', '2' and '3' keys cycles the various parameters of the flex layout.

Pressing the 'D' key will dump the bounds of the scene's facets for use in test validation.

### spinning_square

Package url: `fuchsia-pkg://fuchsia.com/spinning-square-rs#meta/spinning-square-rs.cm`

The original example, now improved to use layout and demonstrate facet draw order.

Press the space bar on an attached keyboard to toggle the square between sharp and rounded corners.

Press the 'B' key to move the square backwards in facet draw order, or 'F' to move it forwards.

## Scene-based Examples

These example demonstrate scenes, but rather than using layout they take responsbility for
positioning the facets.

### clockface

Package url: `fuchsia-pkg://fuchsia.com/clockface-rs#meta/clockface-rs.cm`

This example draws a clock face.

### font_metrics

Package url: `fuchsia-pkg://fuchsia.com/font-metrics-rs#meta/font-metrics-rs.cm`

This example displays metrics about certain fonts.

### gamma

Package url: `fuchsia-pkg://fuchsia.com/gamma-rs#meta/gamma-rs.cm`

This example demonstrate how Carnelian can be used to produce gamma correct output.

### rive

Package url: `fuchsia-pkg://fuchsia.com/rive-rs#meta/rive-rs.cm`

[Rive](https://rive.app) is a file format for animations. This example loads the example from file
and displays it. By default it will load `juice.riv`.

### shapes

Package url: `fuchsia-pkg://fuchsia.com/shapes-rs#meta/shapes-rs.cm`

This example shows how to use pointer input. Press and hold anywhere in the running app to create a
random shape. Drag the pointer to move this shape around. Release the pointer to let the shape fall
towards the bottom of the screen.

### svg

Package url: `fuchsia-pkg://fuchsia.com/svg-rs#meta/svg-rs.cm`

This example loads a vector file in `shed` format and displays it. Press and hold to drag the image
around the screen.

## Render-based Examples

These examples use the render mechanism directly, instead of using scenes and facets.

### ink

Package url: `fuchsia-pkg://fuchsia.com/ink-rs#meta/ink-rs.cm`

This example demonstrate efficient drawing on devices with stylus support.

### png

Package url: `fuchsia-pkg://fuchsia.com/png-rs#meta/png-rs.cm`

This example loads a PNG file from disk and displays it. The scene system does not support the post-copy
mechanism used to display pixel, rather than vector, data. Eventually pixel data will be supported
more directly and this sample can be converted to use scenes.

# Future Areas

## Command Handling

Mature application frameworks usually have some mechanism for commands that might apply to
multiple items in the view hierarchy to be handled by the most specific first and proceeding
to less specific items. This command handling structure can also be used to show/enable menu
items if Fuchsia ever has such a menu.

## Animation

Design and implement a simple animation facility.
