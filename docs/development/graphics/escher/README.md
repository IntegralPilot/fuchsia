# Escher

## Overview

Escher is a C++/Vulkan library that is used primarily by the Scenic implementation; it is not
exposed by the Fuchsia SDK. It provides utilities for:

1. Abstracting and optimizing simple operations with Vulkan, such as:

    - Pipeline reuse
    - Color correction with less memory bandwidth
      - Specification of sub-passes (and render-passes in general) is a bit more convenient in Escher than raw Vulkan, but it's pretty much the same idea, not significantly abstracted.
      - For example, Escher supports sub-passes within a single Vulkan render-pass. That is, when a second sub-pass needs to access a pixel at the same location, then it performs the transform inside the cached data without leaving the GPU memory hierarchy.
    - Updating buffers and sending draw commands directly
    - Resource lifetime management
      - For example, if you submit a Vulkan command-buffer which renders using a texture, and then delete the texture before rendering is complete, the result is undefined; a crash is likely. Escher doesn't delete the texture (or buffer) until it is no longer referenced by any active command buffers.

2. Higher-level domain-specific renderers specific to Scenic, such as:

    a. `RectangleCompositor`, a simple compositor used by Flatland's `VkRenderer` to load display list contents into a single output image known as a framebuffer
    - RectangleCompositor leverages:
      - Flatland-specific Color Correction Vertex and Fragment Shaders, for use when Scenic performs color-correction using Vulkan
      - Flatland-specific Vertex and Fragment Shaders, for use when color correction is delegated to the display driver or not required at all
    - See the [`rainfall` example](/src/ui/examples/escher/README.md) for use of RectangleCompositor.

    b. `PaperRenderer`, a deprecated compositor used by Gfx which builds the abstraction for performing efficient 3D UX computations that were envisioned at the time. See the [`waterfall`](/src/ui/examples/escher/README.md) example for use of PaperRenderer.

Together, these libraries support rapid iteration and resource lifecycle management beyond what would be sustainably achievable when using Vulkan alone.

## Cross-platform support

With a few exceptions, Escher is not platform-dependent. For example, the Escher examples can also
be run on Linux. This provides two distinct benefits:

- Faster iteration in development. Recompiling Escher and running to debug is much faster on Linux than on Fuchsia.
- On other platforms, the Vulkan ecosystem provides a wide variety of development tools which are not available on Fuchsia. For example, the [RenderDoc graphics debugger](http://renderdoc.org) was very useful during the development of Escher.

## GLSL support

Vulkan only accepts shader source code in the SPIR-V format. Escher optionally supports consuming GLSL, the human-readable format, then calls `shaderc::Compiler` to compile it to SPIR-V before loading into Vulkan.

## Source Code

The source code lives in [//src/ui/lib/escher/][escher_src].

[escher_src]: /src/ui/lib/escher
