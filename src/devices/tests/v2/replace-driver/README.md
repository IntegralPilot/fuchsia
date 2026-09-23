# Replace Driver Test

This test checks that we can disable a driver and replace the driver by matching again with another
driver. It uses the same drivers and topology described in the `reload-driver` test with the
following changes:

- target 1 replacement driver will create a child called 'Z' (instead of 'I')
- target 2 replacement driver will create a child called 'Y' (instead of 'K')
- The composite replacement will create a child called 'J_replaced' (instead of 'J')
- The composite replacement will have E as the primary node (instead of 'D').

```
               dev(root)
               /       \
              /         \
             X           X
            /             \
           /               \
    B(left-parent)      C(right-parent)
           |                 / | \
           |                /  |  \
           |               /   |   X
           |              /    |    \
           |             /     |     \
          D()           E()    F()    G(target-1)
           |             |     |       |
           |             |     |       |
           X--------------------       X
           |                           |
           |                           |
      H(composite)                    I(leaf)
           |
           |
           |
           |
           |
        J(target-2)
           |
           |
           X
           |
           |
        K(leaf)
```

See `//src/devices/tests/v2/reload-driver/README.md`.


# Target 1 replacement

This driver will be a fallback driver that should be selected immediately when rematching, after
the main one has been disabled.

# Target 2 replacement

This driver will be registered after the initial driver is disabled and its node goes to the
available nodes. Once registered, it should get bound to the replacement.

# Composite replacement

This driver is a composite driver and will ensure that fully replacing a composite driver with
different primary nodes and node names works.