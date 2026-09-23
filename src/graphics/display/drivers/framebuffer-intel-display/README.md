# Intel Framebuffer Display Driver

This is a simple display driver for Intel GPUs that uses the framebuffer
configured by the bootloader. It is intended for early-boot use before
`intel-display` is available.

## Manual testing

We do not currently have automated integration tests. Behavior changes in this
driver must be validated using this manual test.

Start with a [supported Intel device][fuchsia-hardware-support].

1. Replace the PCI device IDs from
   `src/graphics/display/drivers/intel-display/meta/intel-display.bind` with
   `0xFFFF` so the display device won't bind to `intel-display`:

   ```
   primary parent "pci" {
     fuchsia.Service == "fuchsia.hardware.pci.Service";
     fuchsia.BIND_PCI_VID == fuchsia.pci.BIND_PCI_VID.INTEL;

     accept fuchsia.BIND_PCI_DID {
       0xFFFF, // Only keep a placeholder. No Intel display device will bind to this driver.
     }
   }
   ```

2. Remove the rejected PCI device IDs from the framebuffer driver bind rules in
   `src/graphics/display/drivers/framebuffer-intel-display/meta/framebuffer-intel-display.bind`:

   ```
   primary parent "pci" {
     fuchsia.Service == "fuchsia.hardware.pci.Service";
     fuchsia.BIND_PCI_VID == fuchsia.pci.BIND_PCI_VID.INTEL;
     fuchsia.BIND_PCI_CLASS == fuchsia.pci.BIND_PCI_CLASS.DISPLAY;

     // The rejected PCI device list is removed.
   }
   ```

3. Build Fuchsia, and flash or OTA the target.

   ```posix-terminal
   fx build
   ffx target flash # or `fx ota`
   ```

4. Launch the `squares` demo in the `display-tool` test utility.

   ```posix-terminal
   ffx target ssh -- display-tool squares
   ```

5. Add the following footer to your CL description, to document having
   performed the test.

   ```
   Test: ffx target ssh -- display-tool squares
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

[fuchsia-hardware-support]: https://fuchsia.dev/fuchsia-src/reference/hardware/support-system-config
