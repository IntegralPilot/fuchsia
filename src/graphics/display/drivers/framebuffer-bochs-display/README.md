# Bochs Framebuffer Display Driver

This is a display driver for display engines compatible with the [Bochs display
engine API][bochs-display-engine-api].

The Bochs display engine (also known as Bochs Graphics Adapter, BGA) is a
display engine that supports the VESA Bios Extension (VBE) standard, originally
used in the [Bochs][bochs] emulator. Some other emulators, including
[QEMU][qemu], also implement a `bochs-display` engine that is fully compatible
with the Bochs VBE Display API.

## Manual testing

We do not currently have automated integration tests. Behavior changes in this
driver must be validated using this manual test.

1. Launch a QEMU-based emulator.

   ```posix-terminal
   fx qemu -g -N --no-virtio
   ```

2. Launch the `squares` demo in the `display-tool` test utility.

   ```posix-terminal
   ffx target ssh display-tool squares
   ```

3. Add the following footer to your CL description, to document having performed
   the test.

   ```
   Test: ffx target ssh display-tool squares
   ```

These instructions will work with a `workbench_eng.x64` build that includes the
`//src/graphics/display:tools` GN target. The `//src/graphics/display:tests`
target is also recommended, as it builds the automated unit tests. Debug
assertions, which are extensively used in display drivers, are only enabled in
debug builds.

```posix-terminal
fx set workbench_eng.x64 --debug --with //src/graphics/display:tools \
    --with //src/graphics/display:tests
```

[bochs-display-engine-api]: https://github.com/bochs-emu/VGABIOS/blob/v0_9b/vgabios/vbe_display_api.txt
[bochs]: https://bochs.sourceforge.io/
[qemu]: https://www.qemu.org/
