# escher_flatland

This is a library that initializes Escher and uses Flatland to render text.
The use of Flatland and sysmem are encapsulated within the Vulkan swapchain, which
is provided by the `VK_LAYER_FUCHSIA_imagepipe_swapchain` layer.

See the example [escher-flatland](//src/ui/examples/escher-flatland/README.md) component for how
to use this library to create a text-based graphical component.