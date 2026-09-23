# Starnix Libraries

This directory contains libraries used by Starnix.

## Dependency Rules

Code in this directory **must not** depend on `//src/starnix/kernel/core`
(target `starnix_core`). This separation ensures that base libraries remain
decoupled from the core kernel logic.

## Placement Guide

If your code needs to depend on `starnix_core`, it belongs in one of the
following locations:

*   **`//src/starnix/modules`**: If the code implements a distinct feature,
    file system, or optional module. This is the preferred location for most
    features.
*   **`//src/starnix/kernel`**: If the code is fundamental to the kernel's
    operation and tightly coupled with the core.

If your code is generic and not specific to Starnix at all, consider placing it
in `//src/lib`.
