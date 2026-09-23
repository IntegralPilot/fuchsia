# Life of a touch event

This document explains how events from touch devices (touch screen or touchpad)
are processed on the fuchsia platform.

## Overview

At a high level, the event path is divided amongst multiple components that
operate on different levels of abstraction and state.

- The driver is aware of the raw touch events coming in from the touch device,
  but is not aware of the view tree.
- The Input Pipeline is aware of the view system and device specifics, but is
  not aware of specific view clients in the view tree.
- Scenic owns the UI clients (aka Flatland clients) and the view tree data
structure, which includes critical details like hit regions. Thus Scenic
performs hit testing to determine the actual destination for a touch event.
- The client knows about the events it receives, but not about events sent to
  other clients.

This division allows specific global properties that can be asserted about touch
event dispatch.

- The driver's job is very simple, and will not involve any UI client
  interaction.
- The Input Pipeline, through the [`fuchsia.ui.pointerinjector`][1] protocol,
must be necessarily aware of the view tree hierarchy. In this way, it can
enforce containment of touch events to a specific set of clients. It can even
guarantee Confidentiality, Integrity, and Availability to a specific UI client
(via exclusive-mode injection).
- Scenic is not aware of physical devices and their quirks. Instead, it operates
on abstract devices via the injector protocol, created by Input Pipeline.

These divisions are shearing layers that allow the platform or product to
introduce different behavior without modifying the entire stack. For example, a
simple touchpad can be set up to emulate a physical mouse device. Fake touch
screens can be set up in a testing environment. New input modality's can be
temporarily set up as a legacy device, if a new modality abstraction (such as
stylus) is not yet available for UI clients to use.

## UI client protocol

The core protocol for UI clients, what a typical Fuchsia app would see, is the
[`fuchsia.ui.pointer.TouchSource`][2] protocol. It delivers touch events to a UI
client in a scale-invariant `injector viewport` abstraction, and Flatland
furnishes a transformation matrix to convert these `injector viewport`
coordinates into view-specific coordinates. In this way, a UI client can
recognize physical-scale gestures while magnified, but also allow a gesture to
directly manipulate one of its UI elements that is positioned within the view's
coordinate system.

The events can be batched as a vector, for efficiency. There is no guarantee
that events will or will not be batched.

The timestamps on the events are generated in the driver and preserved to this
point. Hence gesture velocity calculations will not be adversely affected by
FIDL event delays, prior to reaching the UI client.

## Driver

[`hid-input-report`][3] parses USB Touch HID input reports and translates them
to FIDL [`sdk/fidl/fuchsia.input.report/touch`][4].

Parsing HID reports is a pain, so UI team decided to avoid exporting HID beyond
the driver implementation. Instead, UI team defined a clean FIDL for common
events like touch, mouse, and keyboard. This makes it easy for Input Pipeline
and other components to interact with the driver.

### What about uncommon HID devices?

Fuchsia may offer the HID connection directly to a client that actually knows a
lot about that USB HID device, and let it process it directly. However, driver
authors may choose to define events in terms of easier-to-use FIDL, instead of
HID.

## Input Pipeline

[`Input Pipeline`][7] can process 2 types of touch device and translate them to
different events to [`scenic`][8].

### Touch screen

Input Pipeline gets the contact location in device coordinate (from device
descriptor), Input Pipeline normalizes them to physical pixel in display
coordinate. Input Pipeline also watch for the viewport updates from
scene_manager to have the `injector viewport` of the UI client to attach to the
event.

In Input Pipeline, the `injector viewport` is conventionally in physical pixels
and display coordinates. This makes it easier to communicate with subsequent
event processing.

### Touchpad

Input Pipeline identifies device as touchpad when device reports HID collections
including Windows Precision Touchpad collection which is a widely-used touchpad
standard.

Touchpad events are transformed into mouse events, eg. Two-finger vertical or
horizontal movement is interpreted as a mouse scroll event. This doc won't
describe the rest of touchpad here. Please check the [mouse doc][5].

## Injector viewport

The `injector viewport` abstraction is a key part of relaying scale-invariant
gesture data to UI clients, so that pointer movement can be interpreted
correctly under effects like magnification. The structure defines the viewport's
minimal  and maximal extents in the viewport coordinate system.

The boundary of the viewport, a rectangle, is axis aligned with the viewport
coordinate system; however it may otherwise be freely positioned ("float")
within it: there is translation and scaling, but no rotation. Floating gives the
injector some choice in how to convey coordinates, such as in Vulkan NDC, or in
display pixel coordinates. The viewport rectangle defines a latch region used in
dispatch.

A UI client receives a pointer's coordinates in the viewport coordinate system,
along with a matrix to convert coordinates from the viewport coordinate system
to the UI client's coordinate system.

## Hit regions

The [Flatland protocol][6] defines invisible "hit regions" to describe where a
user can interact, inside a UI client's view space. Typically the entire view is
interactive, so Flatland furnishes a default hit region that entirely covers the
view.

Certain advanced scenarios require more setup of the hit region. For example, if
a parent view embeds a child view, and the parent view also wants to "pop up" an
overlay that partially obscures the child view, then the parent UI client must
describe the size of that overlay as a hit region, to Flatland.

Turning off all hit regions in a view means the UI client's graphical content
will be visible to the user, but the user's touch will not get dispatched to
this UI client. Also, accessibility will not be able to associate the UI
client's semantic content with a user's gesture.

## Gesture disambiguation

The UI client typically participates in a gesture disambiguation protocol to
determine a gesture owner for each gesture. Flatland will notify whether the UI
client has been granted or denied the gesture. The GD protocol is cooperative,
so a denied gesture must be treated as a cancellation. If the UI client cannot
guarantee that a gesture can be treated as cancelled, a workaround is to simply
buffer the touch events until granted ownership, and discard the events
otherwise. Certain clients, such as Chrome, make use of this pessimistic model,
so that Fuchsia's touch events do not actually turn into Javascript events
unless warranted with explicit gesture ownership from Flatland.

## Accessibility's LocalHit protocol upgrade

Accessibility's UI client requires extra data that regular UI clients should not
receive. This extra data is provided in a platform-private protocol,
fuchsia.ui.pointer.augment.LocalHit, which "upgrades" the client's TouchSource
endpoint into a fancier version with more data fields.

These data fields describe the result of hit-testing for each pointer event,
giving very fine-grained data on which view and view-internal location each
touch event is positioned over.

Accessibility makes use of this hit data to perform "explore mode".

<!-- xrefs -->

[1]: https://fuchsia.dev/reference/fidl/fuchsia.ui.pointerinjector
[2]: https://fuchsia.dev/reference/fidl/fuchsia.ui.pointer#TouchSource
[3]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/input/lib/hid-input-report/
[4]: https://fuchsia.dev/reference/fidl/fuchsia.input.report
[5]: /docs/concepts/ui/input/mouse.md
[6]: https://fuchsia.dev/reference/fidl/fuchsia.ui.composition
[7]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/lib/input_pipeline
[8]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/ui/scenic
