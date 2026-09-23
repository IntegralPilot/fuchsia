# Lavapipe ICD component

This directory defines two artifacts:

* A shared library ICD which provides a Lavapipe-based software Vulkan implementation
* A component which makes this ICD available to the [Fuchsia Vulkan loader service][fuchsia-vulkan-loader],
  and therefore to Vulkan client apps.
  * The [Vulkan Loader RFC-0205][fuchsia-vulkan-loader-rfc] describes how the parts fit together.

[fuchsia-vulkan-loader]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/graphics/bin/vulkan_loader/README.md
[fuchsia-vulkan-loader-rfc]:https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0205_vulkan_loader
