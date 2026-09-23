---
name: suspend-resume-integration
description: >
  Add or migrate suspend/resume support in a Fuchsia driver using Driver-
  Framework power elements. Use when a driver has no suspend/resume support
  yet and needs it added (integration). Covers the required component manifest
  (.cml), BUILD.gn/Bazel deps, and implementation in both C++ and Rust.
---

## Overview

This guide covers how to add support for suspend and resume in drivers. This
covers necessary updates to the component manifest, build files, and
implementation both in C++ and Rust.

## Steps

### Add suspend/resume support to your driver

Adding support for suspend/resume in your driver is simple. It is these steps:

* Update the component manifest
* Implement the fdf\_power::Suspendable mix-in (C++) or the fdf\_power::SuspendableDriver trait (Rust)
* Implement `SuspendEnabled` (C++) or `suspend_enabled` (Rust)
* Implement the driver's `Resume` and `Suspend` calls


implement_suspendable.md has information about how perform the actions called out in these steps.

#### Update the component manifest

Add `suspend_enabled: "true"` program section of the component manifest, [src/devices/block/drivers/sdmmc/meta/sdmmc.cml](https://cs.opensource.google/fuchsia/fuchsia/+/30ec5f992257d1fb2a646e100c18b16eb7108aee:src/devices/block/drivers/sdmmc/meta/sdmmc.cml;l=16) has an example of this.

If a driver already uses the Suspendable mix-in for C++ or the SuspendableDriver trait for Rust, that's all, you're done. The driver's `Suspend` and `Resume` calls will now be triggered by its power element.

#### Implement the fdf\_power::Suspendable mix-in (C++) or the fdf\_power::SuspendableDriver trait (Rust)

See implement_suspendable.md about how to do this.

#### Implement `SuspendEnabled` (C++) or `suspend_enabled` (Rust)

To do this

* Use a config capability in the manifest
* Tie the capability to a structured config type for the driver
* Import the structured config type into the driver implementation
* Add a field to the driver to store the structured config type
* Returning the suspend value from `SuspendEnabled` (C++) or `suspend_enabled` (Rust) call

implement_suspendable.md has more information about the steps above.

#### Implement the driver's `Resume` and `Suspend` calls

Here add whatever logic the driver needs to do on Suspend and Resume, for example devoting and voting clocks, getting GPIO pin states, etc. One implementation consideration is that in C++ if if there is asynchronous work to do, you need to move the relevant completer into the asynchronous lambda or `fit::promise` or whatever the code may use. This is extremely rare, but if the driver is in Rust and isn't straight-thru code with or without sync, you'll need to make sure the suspend/resume awaits whatever task may be spawned.
