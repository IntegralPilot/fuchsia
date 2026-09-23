# Kernel images

This directory contains the officially supported _kernel images_, i.e.,
(bootable) ZBIs that contain a `KERNEL` item. Each kernel image of name `$name`
gets its own subdirectory of the same name and defines a
[`kernel_image()`](/zircon/kernel/kernel_image.gni).

## GN target API

- `//zircon/kernel/image/$name:$name`: The `kernel_image()` target for the
  kernel image of name `$name`.

- `//zircon/kernel/image/$name:$name.test-data-deps`: The associated data deps
  for use in (ZBI) tests.
