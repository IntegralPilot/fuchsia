# Reload Driver Test

This test checks that DFv2 reload works.

## Scenario

This is the topology that will be tested. The conventions for the graph below are:
 - `X` in the edges indicates `colocation=false` in the child. Otherwise it is `true`.
 - Primary parent of the composite is the parent it is directly under.
 - Node markings are in the form `NodeName(optional bound driver)`

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
           |               /   |   \
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

There are two target drivers for reload in this topology.

This is meant to ensure the basic scenarios work, for example:
 - All child nodes of the target driver are taken down.
 - All parents colocated with the target driver (and their children) are taken down.

This is also meant to cover a variety of edge cases with composites like:
 - A composite node that is taken down can be re-created.
 - If a composite's parent is taken down (even if not all of them) the composite can be re-created.

## How the test works
Each driver has access to a `fuchsia.reloaddriver.test.Waiter` protocol that is hosted by the test.
On startup, each driver will send an `Ack` through the Waiter protocol, and in there they include
the node name that they have binded to.

On the test side, we maintain a mapping of node names to whether we have seen an ack from them.
Along with that information, we also maintain the driver host koid that the node is in (this we
get from the `fuchsia.driver.development.DeviceInfo` protocol which is hosted by driver manager).

Then for each target driver we setup the list of nodes that should change driver hosts if the
restart is behaving correctly. Then we call restart and validate that the nodes that should have
changed hosts, have a new host, and also validate that ones that should not have changed hosts,
have the same host as before.

## Expectation

When `target-1` is reloaded, the following nodes should be reloaded:
 - C
 - E
 - F
 - G
 - H
 - I
 - J
 - K

 When `target-2` is reloaded, the following nodes should be reloaded:
  - H
  - J
  - K

## Example

```
[dev] pid=683966 realm-builder://4/root#meta/root.cm
  [B] pid=684618 fuchsia-boot:///#meta/left_parent.cm
    [D] pid=684618 unbound
      [H] pid=685322 fuchsia-boot:///#meta/composite.cm
        [J] pid=685322 fuchsia-boot:///#meta/target_2.cm
          [K] pid=686016 fuchsia-boot:///#meta/leaf.cm
  [C] pid=684735 fuchsia-boot:///#meta/right_parent.cm
    [E] pid=684735 unbound
      [H] pid=685322 fuchsia-boot:///#meta/composite.cm
        [J] pid=685322 fuchsia-boot:///#meta/target_2.cm
          [K] pid=686016 fuchsia-boot:///#meta/leaf.cm
    [F] pid=684735 unbound
      [H] pid=685322 fuchsia-boot:///#meta/composite.cm
        [J] pid=685322 fuchsia-boot:///#meta/target_2.cm
          [K] pid=686016 fuchsia-boot:///#meta/leaf.cm
    [G] pid=684735 fuchsia-boot:///#meta/target_1.cm
      [I] pid=685527 fuchsia-boot:///#meta/leaf.cm

ffx driver restart fuchsia-boot:///#meta/target_1.cm

[dev] pid=683966 realm-builder://4/root#meta/root.cm
      [B] pid=684618 fuchsia-boot:///#meta/left_parent.cm
        [D] pid=684618 unbound
          [H] pid=694197 fuchsia-boot:///#meta/composite.cm
            [J] pid=694197 fuchsia-boot:///#meta/target_2.cm
              [K] pid=694893 fuchsia-boot:///#meta/leaf.cm
      [C] pid=693801 fuchsia-boot:///#meta/right_parent.cm
        [E] pid=693801 unbound
          [H] pid=694197 fuchsia-boot:///#meta/composite.cm
            [J] pid=694197 fuchsia-boot:///#meta/target_2.cm
              [K] pid=694893 fuchsia-boot:///#meta/leaf.cm
        [F] pid=693801 unbound
          [H] pid=694197 fuchsia-boot:///#meta/composite.cm
            [J] pid=694197 fuchsia-boot:///#meta/target_2.cm
              [K] pid=694893 fuchsia-boot:///#meta/leaf.cm
        [G] pid=693801 fuchsia-boot:///#meta/target_1.cm
          [I] pid=694387 fuchsia-boot:///#meta/leaf.cm

ffx driver restart fuchsia-boot:///#meta/target_2.cm

[dev] pid=683966 realm-builder://4/root#meta/root.cm
  [B] pid=684618 fuchsia-boot:///#meta/left_parent.cm
    [D] pid=684618 unbound
      [H] pid=706457 fuchsia-boot:///#meta/composite.cm
        [J] pid=706457 fuchsia-boot:///#meta/target_2.cm
          [K] pid=706848 fuchsia-boot:///#meta/leaf.cm
  [C] pid=693801 fuchsia-boot:///#meta/right_parent.cm
    [E] pid=693801 unbound
      [H] pid=706457 fuchsia-boot:///#meta/composite.cm
        [J] pid=706457 fuchsia-boot:///#meta/target_2.cm
          [K] pid=706848 fuchsia-boot:///#meta/leaf.cm
    [F] pid=693801 unbound
      [H] pid=706457 fuchsia-boot:///#meta/composite.cm
        [J] pid=706457 fuchsia-boot:///#meta/target_2.cm
          [K] pid=706848 fuchsia-boot:///#meta/leaf.cm
    [G] pid=693801 fuchsia-boot:///#meta/target_1.cm
      [I] pid=694387 fuchsia-boot:///#meta/leaf.cm
```