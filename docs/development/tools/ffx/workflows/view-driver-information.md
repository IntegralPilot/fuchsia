# View driver information

The `ffx driver` command can retrieve various types of information about drivers
on a Fuchsia device.

## Concepts {:#concepts}

The `ffx driver` command can retrieve information related to drivers that are
currently [available](#view-drivers) or [running](#view-running-drivers) on
your target [Fuchsia device][flash-a-device] (or [emulator][fuchsia-emulator]).
However, the `ffx driver` command expects that you can establish an
[SSH connection][ssh-connection] to the target Fuchsia device from your host
machine. To verify this connection to the device, you can run the
[`ffx target show`][ffx-target-show] command.

Before using the `ffx driver` command, it is recommended that you familiarize
yourself with the fundamental concepts in [Driver framework (DFv2)][dfv2],
particularly the following:

- [Device nodes][device-nodes] - A device node represents a hardware component,
  a virtual device, or a part of a hardware device.
- [Node topology][node-topology] - A node topology describes the parent-child
  relationships between device nodes in the system.
- [Driver host][driver-host] - A driver host, which runs as a Fuchsia component,
  provides isolation between drivers in a Fuchsia system. In Fuchsia, every
  driver lives in a driver host, and more than one driver can be co-located
  within a single driver host.

## View drivers {:#view-drivers}

To view all drivers available on your Fuchsia device, run the following command:

```posix-terminal
ffx driver list
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver list
fuchsia-boot:///alc5663#meta/alc5663.cm
fuchsia-boot:///asix-88179#meta/asix-88179.cm
fuchsia-boot:///asix-88772b#meta/asix-88772b.cm
fuchsia-boot:///block-core#meta/block.core.cm
fuchsia-boot:///bt-transport-usb#meta/bt-transport-usb.cm
fuchsia-boot:///bus-pci#meta/bus-pci.cm
fuchsia-boot:///buttons#meta/buttons.cm
fuchsia-boot:///clock#meta/clock.cm
fuchsia-boot:///ctaphid#meta/ctaphid.cm
fuchsia-boot:///display-coordinator#meta/display-coordinator.cm
fuchsia-boot:///e1000#meta/e1000.cm
fuchsia-boot:///ftdi#meta/ftdi.cm
fuchsia-boot:///fvm#meta/fvm.cm
...
```

To view all drivers available on your Fuchsia device with more detailed
information, run the command with the -`v` flag:

```posix-terminal
ffx driver list -v
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver list -v
URL       : fuchsia-pkg://fuchsia.com/iwlwifi#meta/iwlwifi.cm
DF Version: 2
Device Categories: [misc]
Bind rules bytecode:
  fuchsia.BIND_FIDL_PROTOCOL == 4
  fuchsia.BIND_PCI_VID == 32902
  Jump if fuchsia.BIND_PCI_DID == 2394 to ??
  Jump if fuchsia.BIND_PCI_DID == 2395 to ??
  Jump if fuchsia.BIND_PCI_DID == 9469 to ??
  Jump if fuchsia.BIND_PCI_DID == 9510 to ??
  Jump if fuchsia.BIND_PCI_DID == 41200 to ??
  Abort
  Label ??
  fuchsia.BIND_COMPOSITE == 1

URL       : fuchsia-boot:///virtio_rng#meta/virtio_rng.cm
DF Version: 1
Device Categories: [misc]
Bind rules bytecode:
Node (primary): pci
  fuchsia.BIND_FIDL_PROTOCOL == 4
  fuchsia.BIND_PCI_VID == 6900
  Jump if fuchsia.BIND_PCI_DID == 4164 to ??
  Jump if fuchsia.BIND_PCI_DID == 4101 to ??
...
```

### View running drivers {:#view-running-drivers}

To view all drivers that are currently running (loaded) on your Fuchsia
device, run the command with the -`loaded` flag:

```posix-terminal
ffx driver list --loaded
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver list --loaded
fuchsia-boot:///#meta/block.core.cm
fuchsia-boot:///#meta/bus-pci.cm
fuchsia-boot:///#meta/display.cm
fuchsia-boot:///#meta/fvm.cm
fuchsia-boot:///#meta/goldfish-display.cm
fuchsia-boot:///#meta/goldfish.cm
fuchsia-boot:///#meta/goldfish_address_space.cm
fuchsia-boot:///#meta/goldfish_control.cm
fuchsia-boot:///#meta/goldfish_sensor.cm
fuchsia-boot:///#meta/goldfish_sync.cm
fuchsia-boot:///#meta/hid-input-report.cm
fuchsia-boot:///#meta/hid.cm
fuchsia-boot:///#meta/intel-hda.cm
...
```

## View driver hosts {:#view-driver-hosts}

To view all [driver hosts][driver-host] running on your Fuchsia device
as well as the drivers they host, run the following command:

```posix-terminal
ffx driver list-hosts
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver list-hosts
Driver Host: 5416
    fuchsia-boot:///#meta/bus-pci.cm
    fuchsia-boot:///#meta/display.cm
    fuchsia-boot:///#meta/goldfish-display.cm
    fuchsia-boot:///#meta/goldfish.cm
    fuchsia-boot:///#meta/goldfish_control.cm
...

Driver Host: 8248
    fuchsia-boot:///#meta/intel-rtc.cm

Driver Host: 8317
    fuchsia-boot:///#meta/pc-ps2.cm

Driver Host: 9604
    fuchsia-boot:///#meta/block.core.cm
    fuchsia-boot:///#meta/fvm.cm
    fuchsia-boot:///#meta/virtio_block.cm
...
```

## View the node topology {:#view-the-node-topology}

To view the entire [node topology][node-topology] of your Fuchsia device,
run the following command:

```posix-terminal
ffx driver dump
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver dump
[dev] pid=5521 fuchsia-boot:///platform-bus#meta/platform-bus.cm
  [sys] pid=None unbound
    [platform] pid=None unbound
      [ram-disk] pid=7584 fuchsia-boot:///ramdisk#meta/ramdisk.cm
        [ramctl] pid=None unbound
      [ram-nand] pid=None unbound
      [virtual-audio] pid=25117 fuchsia-pkg://fuchsia.com/virtual_audio#meta/virtual_audio_driver.cm
        [virtual_audio] pid=None unbound
      [bt-hci-emulator] pid=None unbound
      [fake-battery] pid=24707 fuchsia-pkg://fuchsia.com/fake-battery#meta/fake_battery.cm
        [fake-battery] pid=None unbound
        [power-simulator] pid=None unbound
      [pt] pid=5521 fuchsia-boot:///platform-bus-x86#meta/pl
...
```

### View the node topology under a specific node {:#view-the-node-topology-under-a-specific-node}

To view only a subgraph of the node topology under a specific node,
run the following command:

```posix-terminal
ffx driver dump <NODE_NAME>
```

Replace `NODE_NAME` with the name of your target node, for example:

```none {:.devsite-disable-click-to-copy}
$ ffx driver dump goldfish-control
[goldfish-control] pid=5521 fuchsia-boot:///goldfish_display#meta/goldfish-display.cm
  [goldfish-display] pid=5521 fuchsia-boot:///display-coordinator#meta/display-coordinator.cm
    [display-coordinator] pid=None unbound
...
```

### Graph the node topology {:#graph-the-node-topology}

To graph the node topology, run the following command:

```posix-terminal
ffx driver node graph
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver node graph
digraph {
    forcelabels = true; splines="ortho"; ranksep = 5; nodesep = 1;
    node [ shape = "box" color = " #0a7965" penwidth = 2.25 fontname = "prompt medium" fontsize = 10 margin = 0.22 ];
    edge [ color = " #283238" penwidth = 1 style = solid fontname = "roboto mono" fontsize = 10 ];
    rankdir = "TB"
    subgraph "cluster_19434" {
        label = "Host 19434";
        style = "filled,rounded";
        fillcolor = " #b1b9be";
        subgraph "cluster_19434_virtual-audio-driver.cm" {
            label = "virtual-audio-driver.cm";
            style = "filled,rounded";
            fillcolor = " #dce0e3";
            "4358549555312" [label="virtual-audio", id = "4358549555312"]
            "4358549791216" [label="virtual-audio", id = "4358549791216"]
        }
    }
...
```

#### Filtering {:#graph-the-node-topology-filtering}

A full Fuchsia system graph can be quite large. Use the `--only` (or `-o`)
flag to filter the graph to specific sections. You can also use this flag
to filter for only bound or unbound nodes.
For example, the following command only graphs the relatives
of the 'virtio-input' node:

```posix-terminal
ffx driver node graph -o relatives:PCI0.bus.00_03_0.00_03_0.virtio-input
```

Use the following filters with the `--only` flag
(the plural form, i.e., `ancestors`, is also accepted and has the same effect):

-   `--only bound`
    Shows only bound nodes. A node is bound if it meets any of the following conditions:
    -   A driver binds to it directly.
    -   It is a parent to a composite node.
    -   Its parent node explicitly owns it.
-   `--only unbound`
    Shows only unbound nodes (nodes that meet none of the bound conditions).
-   `--only ancestor(s):dev.sys.xyz`
    Shows only the ancestors of the specified node.
-   `--only descendant(s):dev.sys.xyz`
    Shows only the descendants of the specified node.
-   `--only relative(s):dev.sys.xyz`
    Shows only the relatives of the specified node. Relatives are both ancestors and descendants.
-   `--only sibling(s):dev.sys.xyz`
    Shows only the siblings of the specified node.
-   `--only primary_ancestor(s):dev.sys.xyz`
    Same as ancestor, but for composite nodes, it only traverses the primary parent.
-   `--only primary_relative(s):dev.sys.xyz`
    Same as relative, but for composite nodes, it only traverses the primary parent.
-   `--only primary_sibling(s):dev.sys.xyz`
    Same as sibling, but if the filter target is a composite node, this filter only
    includes children from its primary parent.

#### Showing service routes {:#graph-the-node-topology-services}

You can add service routes to the graph with the `--services` flag. The graph
displays these routes with an arrowhead to distinguish them from
parent/child relationships.

Adding service routes can create a very large graph that can overwhelm the graphic engine.
To avoid this, always use this flag with a filter. For example:


```posix-terminal
ffx driver node graph --services -o relatives:PCI0.bus.00_03_0.00_03_0.virtio-input
```

#### Creating the graph image {:#graph-the-node-topology-creating}

To convert this output to a `png` file, you can install the
[`dot`][dot]{:.external} command and pass the output to generate
the `png` file. Alternatively, you can paste the output to
[GraphViz][graphviz]{:.external}.

#### Generating an interactive graph {:#graph-the-node-topology-interactive}

You can generate an interactive HTML page from the SVG output to more easily
explore the graph. This HTML page lets you hover over and highlight service or parent/child routes.

To create the HTML file with a single command, pipe the graph output through `dot`
and then pipe the resulting SVG back into the graph command with the `--html` flag.

For example:

```posix-terminal
ffx driver node graph --services -o relatives:PCI0.bus.00_03_0.00_03_0.virtio-input | \
    dot -Tsvg | \
    ffx driver node graph --html > local/graph.html
```

If you use `GraphViz`, you can provide an existing SVG file with the `--svg` flag
instead of piping the output from `dot`.

## View device nodes {:#view-device-nodes}

To view the properties of all [device nodes][device-nodes] on your
Fuchsia device, run the following command:

```posix-terminal
ffx driver node list
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver node list
root
dev.sys
dev.sys.platform
ram-disk
ram-nand
virtual-audio
bt-hci-emulator
fake-battery
board
00_00_1b
ram-disk.ramctl
virtual-audio.virtual_audio
fake-battery.fake-battery
fake-battery.power-simulator
PCI0
acpi
00_00_1b.sysmem
PCI0.bus
acpi.acpi-_SB_
acpi.acpi-_TZ_
00_00_1b.sysmem.sysmem-banjo
00_00_1b.sysmem.sysmem-fidl
PCI0.bus.00_00_0
PCI0.bus.00_01_0
PCI0.bus.00_02_0
...
```

To view the state and owner information of device nodes, run the command with the -`v` flag:

```posix-terminal
ffx driver node list -v
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver node list -v
...
 State          Moniker                                                          Owner
 Bound        root                                                              fuchsia-boot:///platform-bus#meta/platform-bus.cm
 Bound        dev.sys                                                           parent
 Bound        dev.sys.platform                                                  parent
 Bound        board                                                             fuchsia-boot:///platform-bus-x86#meta/platform-bus-x86.cm
 Bound        virtual-audio                                                     fuchsia-pkg://fuchsia.com/virtual-audio#meta/virtual-audio-driver.cm
 Bound        virtual-audio-legacy                                              fuchsia-pkg://fuchsia.com/virtual-audio-legacy#meta/virtual-audio-legacy-driver.cm
 Bound        fake-battery                                                      fuchsia-pkg://fuchsia.com/fake_battery#meta/fake_battery.cm
 Bound        PCI0                                                              fuchsia-boot:///bus-pci#meta/bus-pci.cm
 Bound        acpi                                                              parent
 Bound        virtual-audio.virtual-audio                                       parent
 Bound        virtual-audio-legacy.virtual-audio-legacy                         parent
 Bound        PCI0.bus                                                          parent
 Bound        acpi._SB_                                                         parent
 Bound        acpi._TZ_                                                         parent
 Unbound      PCI0.bus.00_00_0                                                  none
...
```

To view the properties of a specific device node with more detailed information use `ffx driver node show`:

```posix-terminal
ffx driver node show virtual-audio
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver node show virtual-audio
          Name:  virtual-audio
       Moniker:  virtual-audio
         Owner:  fuchsia-pkg://fuchsia.com/virtual-audio#meta/virtual-audio-driver.cm
    Node State:  Bound
     Host Koid:  19434
  Parent Count:  1
   Child Count:  1

  Bus Topology:  Bus Type  Stability  Address
                 Platform  Stable     virtual-audio

  Node Properties:  Key                                       Value
                    fuchsia.BIND_PLATFORM_DEV_VID             0
                    fuchsia.BIND_PLATFORM_DEV_PID             0
                    fuchsia.BIND_PLATFORM_DEV_DID             57
                    fuchsia.BIND_PLATFORM_DEV_INSTANCE_ID     0
                    fuchsia.BIND_PROTOCOL                     85
                    fuchsia.resource.MMIO_COUNT               0
                    fuchsia.resource.INTERRUPT_COUNT          0
                    fuchsia.resource.BTI_COUNT                0
                    fuchsia.resource.SMC_COUNT                0
                    fuchsia.Service                           fuchsia.hardware.platform.device.Service

  Node Offers:  Service                                   Source  Instances
                fuchsia.hardware.platform.device.Service  dev     default


```

### Filter for specific device nodes {:#view-device-nodes-filtering}

You can filter the list of nodes using the same filters described in the
[filtering](#graph-the-node-topology-filtering) section.

For example, this example only lists the descendants of the specified node:

```posix-terminal
ffx driver node list -o descendants:<NODE>
```

Replace `<NODE>` with a component moniker.

This example filters the results to only show the descendants of the `acpi` node:

```none {:.devsite-disable-click-to-copy}
$ ffx driver node list -o descendants:acpi
acpi
acpi.acpi-_SB_
acpi.acpi-_TZ_
acpi.acpi-_SB_.pt
acpi.acpi-_SB_.acpi-PCI0
acpi.acpi-_SB_.acpi-HPET
acpi.acpi-_SB_.acpi-LNKE
acpi.acpi-_SB_.acpi-LNKF
acpi.acpi-_SB_.acpi-LNKG
acpi.acpi-_SB_.acpi-LNKH
acpi.acpi-_SB_.acpi-GSIE
acpi.acpi-_SB_.acpi-GSIF
acpi.acpi-_SB_.acpi-GSIG
acpi.acpi-_SB_.acpi-GSIH
...
```

## View composite nodes {:#view-composite-nodes}

To view all composite nodes on your Fuchsia device,
run the following command:

```posix-terminal
ffx driver list-composites
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver list-composites
...
acpi-GFRO-composite
acpi-CPUS-composite
acpi-_TZ_-composite
goldfish-control-2
00:00.0
00:01.0
00:02.0
00:03.0
00:04.0
00:05.0
00:06.0
00:0b.0
...
```

To view composite nodes with more detailed information,
run the command with the -`v` flag:

```posix-terminal
ffx driver list-composites -v
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver list-composites -v
...
Name     : 00_02_0
Driver   : fuchsia-boot:///#meta/virtio_block.cm
Device   : dev/sys/platform/pt/PCI0/bus/00:02.0/00_02_0
Parents  : 3
Parent 0 : sysmem
   Device : dev/sys/platform/00:00:1b/sysmem/sysmem-fidl
Parent 1 : pci (Primary)
   Device : dev/sys/platform/pt/PCI0/bus/00:02.0
Parent 2 : acpi
   Device : dev/sys/platform/pt/acpi/acpi-_SB_/acpi-PCI0/acpi-S10_/pt
...
```

## View composite node specifications {:#view-composite-node-specifications}

To view all composite node specifications on your Fuchsia
device, run the following command:

```posix-terminal
ffx driver list-composite-node-specs
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver list-composite-node-specs
...
00_1f_0             : None
00_1f_2             : fuchsia-boot:///ahci#meta/ahci.cm
00_06_0             : None
00_02_0             : fuchsia-boot:///virtio_block#meta/virtio_block.cm
00_1f_3             : None
00_05_0             : fuchsia-boot:///virtio_input#meta/virtio_input.cm
00_0b_0             : fuchsia-boot:///goldfish_address_space#meta/goldfish_address_space.cm
00_01_0             : fuchsia-boot:///intel-hda#meta/intel-hda.cm
00_03_0             : fuchsia-boot:///virtio_input#meta/virtio_input.cm
00_04_0             : fuchsia-boot:///virtio_netdevice#meta/virtio_netdevice.cm
00_00_0             : None
...
```

To view composite node specifications with more detailed
information, run the command with the -`v` flag:

```posix-terminal
ffx driver list-composite-node-specs -v
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx driver list-composite-node-specs -v
...
Name      : ft3x27_touch
Driver    : fuchsia-boot:///#meta/focaltech.cm
Nodes     : 2
Node 0    : "i2c" (Primary)
  3 Bind Rules
  [ 1/ 3] : Accept "fuchsia.BIND_FIDL_PROTOCOL" { 0x000003 }
  [ 2/ 3] : Accept "fuchsia.BIND_I2C_BUS_ID" { 0x000001 }
  [ 3/ 3] : Accept "fuchsia.BIND_I2C_ADDRESS" { 0x000038 }
  2 Properties
  [ 1/ 2] : Key "fuchsia.BIND_FIDL_PROTOCOL"   Value 0x000003
  [ 2/ 2] : Key "fuchsia.BIND_I2C_ADDRESS"     Value 0x000038
Node 1    : "gpio-int"
  2 Bind Rules
  [ 1/ 2] : Accept "fuchsia.BIND_PROTOCOL" { 0x000014 }
  [ 2/ 2] : Accept "fuchsia.BIND_GPIO_PIN" { 0x000004 }
  2 Properties
  [ 1/ 2] : Key "fuchsia.BIND_PROTOCOL"        Value 0x000014
  [ 2/ 2] : Key "fuchsia.gpio.FUNCTION"        Value "fuchsia.gpio.FUNCTION.TOUCH_INTERRUPT"
...
```

## Appendices

### Register a component as a driver {:#register-a-component-as-a-driver}

To register a component as a driver to your Fuchsia device,
run the following command:

```posix-terminal
ffx driver register <URL>
```

Replace `URL` with a component URL from your Fuchsia package
server, for example:

```none {:.devsite-disable-click-to-copy}
$ ffx driver register fuchsia-pkg://fuchsia.com/my_example#meta/my_new_driver.cm
```

### Disable a driver {:#disable-a-driver}

To disable (that is, de-register) a driver from your Fuchsia device,
run the following command:

```posix-terminal
ffx driver disable <URL>
```

Replace `URL` with a component URL from your Fuchsia package
server, for example:

```none {:.devsite-disable-click-to-copy}
$ ffx driver disable fuchsia-pkg://fuchsia.com/my_example#meta/my_driver.cm
```

<!-- Reference links -->

[flash-a-device]: /docs/development/tools/ffx/workflows/flash-a-device.md
[fuchsia-emulator]: /docs/development/tools/ffx/workflows/start-the-fuchsia-emulator.md
[ssh-connection]: /docs/development/tools/ffx/workflows/create-ssh-keys-for-devices.md
[ffx-target-show]: /docs/development/tools/ffx/workflows/view-device-information.md#get_detailed_information_from_a_device
[dfv2]: /docs/concepts/drivers/driver_framework.md
[device-nodes]: /docs/concepts/drivers/drivers_and_nodes.md
[node-topology]: /docs/concepts/drivers/drivers_and_nodes.md#node_topology
[driver-host]: /docs/concepts/drivers/driver_framework.md#driver_host
[dot]: https://www.mankier.com/1/dot
[graphviz]: https://dreampuf.github.io/GraphvizOnline/#digraph
