# `flatland-rainbow` example app

There are two components defined here (plus tests):
- `flatland-rainbow.cm`
- `flatland-rainbow-vulkan.cm`

`flatland-rainbow.cm` is a simple example app which uses Flatland to display
content.  It displays colored squares which rotate colors through time.  It also listens
to touch events and echoes them to the syslog.

`flatland-rainbow-vulkan.cm` is the same, except it uses Vulkan to render, instead
of negotiating CPU-writable buffers with sysmem.  Also, this component connects directly
to the `fuchsia.element.GraphicalPresenter` protocol, rather than serving the
`fuchsia.ui.app.ViewProvider` protocol and waiting for a request.

The same binary is used by the two components; the difference is that the latter
passes `--use-vulkan` and `--use-graphical-presenter` to the program.

TODO(https://fxbug.dev/42055867): the Vulkan version uses a single filled rect, instead of rendering
each quadrant in a different color.

To launch:
- `ffx session add fuchsia-pkg://fuchsia.com/flatland-examples#meta/flatland-rainbow.cm`
- `ffx session add fuchsia-pkg://fuchsia.com/flatland-examples#meta/flatland-rainbow-vulkan.cm`
