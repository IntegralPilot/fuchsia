# Goldfish Display Driver

This is a display driver for the "goldfish" virtual display hardware in
[FEMU (the Fuchsia Emulator)][femu], which is based on
[AEMU (the Android Emulator)][aemu].

The "goldfish" virtual hardware is documented in
[GOLDFISH-VIRTUAL-HARDWARE.TXT][goldfish-virtual-hardware-txt] in the AEMU
source code. This display driver targets GPU emulation mode, which uses the
pipe mechanism documented in [ANDROID-QEMU-PIPE.TXT][aemu-pipe-txt].

## Manual testing

We do not currently have automated integration tests. Behavior changes in this
driver must be validated using this manual test.

1. Launch a FEMU-based emulator.

   ```posix-terminal
   ffx emu start --engine femu
   ```

2. Stop Scenic if it's running. This frees up the Display Coordinator's
   primary client connection.

   ```posix-terminal
   ffx component stop core/ui/scenic
   ```

3. Launch the `squares` demo in the `display-tool` test utility.

   ```posix-terminal
   ffx target ssh display-tool squares
   ```

4. Add the following footer to your CL description, to document having performed
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

[aemu]: https://developer.android.com/studio/run/emulator
[aemu-pipe-txt]: https://android.googlesource.com/platform/external/qemu/+/refs/heads/emu-master-dev/android/docs/ANDROID-QEMU-PIPE.TXT
[femu]: /docs/development/build/emulator.md
[goldfish-virtual-hardware-txt]: https://android.googlesource.com/platform/external/qemu/+/refs/heads/emu-master-dev/android/docs/GOLDFISH-VIRTUAL-HARDWARE.TXT
