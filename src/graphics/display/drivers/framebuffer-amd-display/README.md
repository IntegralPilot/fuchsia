# AMD Framebuffer Display Driver

This is a simple display driver for AMD GPUs that uses the framebuffer configured by
the bootloader. It is intended for early-boot use before a full AMD display driver is
available.

## Supported hardware

This driver does not implement any AMD-specific hardware programming and it relies
entirely on the framebuffer that the bootloader (UEFI/Gigaboot) has already
configured. It will bind to any PCI device with AMD's vendor ID and PCI `Display`
class, and reads the framebuffer through PCI BAR 0 (a convention that holds
across AMD's discrete and integrated GPUs).

The driver has only been validated on the following hardware:

* AMD Strix Halo (Ryzen AI Max+ 395)

## Manual testing

We do not currently have automated integration tests. Behavior changes in this driver
must be validated using this manual test on a supported AMD device.

1. Launch the `squares` demo in the `display-tool` test utility.

   ```posix-terminal
   ffx target ssh display-tool squares
   ```

2. Add the following footer to your CL description, to document having performed
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
