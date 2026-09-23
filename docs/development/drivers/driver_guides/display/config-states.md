# Display Coordinator terms for configuration states

This document covers the terminology used for describing the display
configuration states in the Display Coordinator.

This document describes the desired future shape of our codebase. To facilitate
migrating to new terms, each term's description includes old terms used to refer
to the same concept. This paragraph will be removed once we finish migrating our
code to the new terms.

## Background

[The display drivers stack README][display-stack-overview] gives an overview of
our stack. [The hardware overview document][display-hardware-overview] covers
many terms used here. The following simplified model should be sufficient for
understanding the rest of this document.

Display configurations specify the content that gets composited onto displays.
Aside: The codebase uses configuration and config inconsistently. Addressing the
inconsistency is outside the scope of this document, and will be tackled
separately.

Each display configuration has a list of layers. Each layer may use an image.
Each display configuration layer has a list of images, where each image may be
associated with a wait event (also called "ready fence"). If present, the wait
event is signaled when the image’s data is ready to be fetched by the display
hardware.

Conceptually, display engine hardware runs a loop where it processes a display
configuration, meaning it fetches image data from DRAM, composites images, and
transmits the result over a display connection.

Most display engines use a double-buffering scheme where they have two sets of
registers for storing display configurations. The *active* set of registers
serves as the input to the processing loop. The *shadow* set of registers is
modified by the engine driver software. At the beginning of a loop iteration,
the display engine hardware performs an operation known as a *flip* or a
*flush*, where the values in the shadow registers are atomically written to
their active counterparts.

## Terms used to describe display configuration states

This section covers three types of terms. All terms are non-comparable
adjectives, and describe conditions that are either met or not met by
configurations. Here are the types of terms.

1. state - mutually exclusive with all other states
2. category of states - mutually exclusive with all other states outside the
   category
3. attribute - may apply to other states

This is a summary of the terms introduced below.

* state: "draft"
* category of states: "committed"
  * state: "waiting"
  * category of states: "submitted" - applies to all terms below
    * state: "queued"
    * state: "latched"
    * state: "retired"
    * attribute: "displayed" - applies to "latched" and "retired"

**Draft configurations** are mutable. Coordinator clients modify the
configurations via `Coordinator.Set*()` FIDL calls. This term describes a state,
and is mutually exclusive with all the other terms below. (Old names: *pending
configs*)

**Committed configurations** are no longer mutable. Coordinator clients commit
draft configurations via Coordinator.CommitConfig() FIDL calls. This term
describes a category that includes all other states and categories below.  (Old
names: *applied configs*, *Coordinator.ApplyConfig*)

**Waiting configurations** are committed configurations that use images that
haven't had their wait event signaled. This term describes a state that is
mutually exclusive with all the other terms described below. (Old names:
*pending applied configs*, *applied configs*)

**Submitted configurations** are committed configurations that were transmitted
by the Coordinator to the engine drivers, via Engine.SubmitConfig() FIDL calls.
This term describes a category that includes all other states and categories
below. (Old names: *applied configs*, *Engine.ApplyConfig*)

**Queued configurations** are submitted configurations that are waiting to be
used by the display engine hardware. When a configuration is submitted, it first
becomes queued. Assuming typical hardware, at most two configurations can be
queued: one queued configuration is stored in the display engine’s shadow (as
opposed to active) registers, and the other queued configuration is processed by
the display engine driver, or in transit from the Coordinator to the engine
driver. This term describes a state that is mutually exclusive with all the
other terms below.

The **latched configuration** is the submitted configuration that is currently
processed by the display engine hardware. This configuration’s details are
loaded in the display engine’s active registers. The images in this
configuration are having their data scanned (fetched from DRAM) by the display
engine hardware. When the display engine hardware starts a new loop iteration,
it latches one of the queued configurations, or the previously latched
configuration. This term describes a state that is mutually exclusive with all
the other terms below, except for *displayed*. (Old names: *active configs*,
*applied configs*)

**Retired configurations** are submitted configurations that will never be
latched in the future. A latched configuration becomes retired after the display
engine hardware latches a different configuration. A queued configuration can
become retired without ever being latched, if a newer queued configuration gets
written to the display engine’s shadow (as opposed to active) registers. This
term describes a state that is compatible with the *display* attribute below.

The **displayed configuration** is the configuration whose output is shown by
the display hardware to the user. A latched configuration becomes displayed
after the display engine finishes scanning it and transmitting it via the
display link, and the display panel finishes changing its physical surface to
match the configuration’s output. A configuration may be retired by the time it
becomes displayed, if the display engine has already latched a different
configuration. This term is an attribute that is compatible with the states
*latched* and *retired*. (Old names: *active configs*, *applied configs*)

## Latching is racy

*This section is here to help reason through the terms described above.
Improving the raciness situation is outside the scope of this document.*

There is no generic way to know precisely when a configuration becomes latched
or retired. This makes it difficult to reason about when it’s safe to modify the
image buffers used by a configuration, and when it’s safe to release the
resources used by a configuration.

Intuitively, it may seem safe to assume that the display engine always latches
the latest queued configuration. Unfortunately, this is not true. Queued
configurations may be in transit between the Coordinator and the display engine
driver, or may be undergoing processing by the display engine driver. So, the
display engine may latch the previously latched configuration, or it may latch
an older queued configuration.

We don’t currently know precisely when a configuration becomes latched. The next
best thing we currently have is VSync configuration time stamps. TODO: This is
probably racy too, because it may take a while to deliver a VSync interrupt to
the display engine driver.

## Writing to memory used by a latched configuration causes undefined behavior

*This section is here to help reason through the terms proposed above.*

The Fuchsia display drivers stack’s API contract will state that modifying
memory used by latched configurations causes UB (Undefined Behavior). Most folks
have used framebuffer-based hardware at least once, and assume that the worst
thing that can happen is "glitchy" pixels on display.

We will stick with UB, to account for the complexities of ever-evolving
hardware. We provide an example supporting our position below.

Modern SoC (system-on-chip) hardware has a complex communication network (known
as fabric, or NoC / network-on-chip) that routes memory operations between the
DRAM controller, caches, and the rest of the chip. Due to ever-increasing
complexity, the fabric can have design bugs that result in deadlocks. Deadlocks
are more common in early revisions (A0, B0) and less common in mass-produced
(MP) chips. Still, MP SoC hardware may require software workarounds to avoid
fabric deadlocks.

In today’s software ecosystem, it’s likely that all modern hardware is tested to
support framebuffer operation for the display engine. However, at some point in
the future, it’s possible that supporting framebuffer operation becomes
low-priority enough that it’s not tested or not supported by the fabric. If that
happens, writing to a latched configuration’s memory image may deadlock the
fabric, bringing the entire system to a halt.

[display-hardware-overview]: /docs/development/drivers/driver_guides/display/hardware.md
[display-stack-overview]: /src/graphics/display/README.md
