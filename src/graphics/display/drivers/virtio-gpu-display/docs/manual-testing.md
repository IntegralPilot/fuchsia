# Manual tests for the virtio-gpu-display drivers

## Bouncing squares demo

1. Follow the [driver change validation guide][change-validation-guide] steps to
   launch the `squares` demo in the `display-tool` test utility. The last
   command will be:

    ```posix-terminal
    ffx target ssh -- display-tool squares
    ```

2. Verify that the QEMU window shows bouncing squares.

3. Press Ctrl+C to exit the test utility.

4. Add the following footer to your CL description, to document having performed
   the test.

   ```
   Test: ffx target ssh -- display-tool squares
   ```

[change-validation-guide]: ./change-validation.md
