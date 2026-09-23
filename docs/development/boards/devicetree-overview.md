# Devicetree overview

[**Devicetree**][devicetree-spec]{:.external} is a data structure for describing
hardware. In Fuchsia, the [board driver][board-driver] uses devicetree to create
[nodes][nodes] in the driver framework and sets configurations needed to
initialize the board. Devicetree contains the following two parts that are used
by the board driver to construct a devicetree parser:

- **Devicetree manager library**: [This library][devicetree-manager] is
  responsible for parsing the [devicetree blob](#passing-the-devicetree-blob)
  (DTB) and creating a runtime node structure which can be accessed by the
  devicetree visitors.

- **Devicetree visitors**: [Visitors][visitors] are objects that the board
  driver provides to the devicetree manager during the `Walk` call. Each
  visitor's interface will get invoked on all nodes of the devicetree when
  walking the tree. They are responsible for converting the devicetree data
  into driver-specific metadata and bind rules.

## Board driver integration {:#board-driver-integration}

The board driver initializes the devicetree manager library with the incoming
namespace in order to provide connection to the
`fuchsia_hardware_platform_bus::Service::Firmware` protocol. The manager reads
the devicetree using the `fuchsia_hardware_platform_bus::Service::Firmware`
protocol. The board driver can invoke the `Walk` method of the devicetree
manager to parse the devicetree blob and collect node properties. During the
walk call, several visitors can be provided to parse and transform node
properties into Fuchsia driver metadata and bind rules. Once the walk is
completed, the board driver invokes the `PublishDevices` method to publish all
the device nodes to the driver framework.

For reference implementation, see the [`examples`][devicetree-examples]
directory.

## Devicetree bindings and visitors {:#devicetree-bindings-and-visitors}

Devicetree bindings describes the requirements on the content of a devicetree
node pertaining to a specific device or a class of device. They are documented
using devicetree schema files, which are YAML files that follow a certain meta
schema (for example, see [`smc.yaml`][smc-yaml]). For more information on
devicetree schemas, see [Devicetree Schema Tools][dt-schema]{:.external}.

[Devicetree visitors][visitors] can be used with the devicetree manager to parse
and extract information from all or specific devicetree nodes. The devicetree
specification lists a set of
[standard properties][standard-properties]{:.external}, some of which are parsed
by the [default visitors][default-visitors] in the `visitors/default` folder.
A new visitor will have to be developed for any new bindings that need to be
parsed (for more details, see [Writing a new visitor][writing-new-visitor]).

The default visitors are compiled into a static library that can be built with
the board driver. All other visitors (that is,
[driver visitors][driver-visitors]) are built as a shared library using the
`devicetree_visitor` GN target. The board driver can include the necessary
visitor collection in its package under `/pkg/lib/visitors/`. At runtime, the
board driver can load all these visitors using the
[`load-visitors`][load-visitors] helper library.

## Passing the devicetree blob {:#passing-the-devicetree-blob}

Typically, the devicetree blob (DTB) is passed down by the bootloader to the
kernel as a `ZBI_TYPE_DEVICETREE` item and made available to the board driver
via `fuchsia_hardware_platform_bus::Service::Firmware` protocol. In boards where
the bootloader is not yet capable of passing the DTB (typically during board
bringup), the DTB can be passed in through board configuration's devicetree
field, and later it will be appended to the Zircon Boot Image (ZBI) by assembly.

## Testing devicetree {:#testing-devicetree}

A board driver integration test can be added to test that the parsing and
creation of nodes by the devicetree libraries and visitors. The
`board-test-helper` library can be used to create a test realm with driver
framework, platform bus and the board driver components running in it. The DTB
is passed as a test data resource and is provided to the board driver through
the fake `fuchsia_boot::Items` protocol implemented in the `board-test-helper`
library. The test creates a platform bus device with specified VID, PID to
which the board driver will bind to. The board driver then invokes the
devicetree manager, parses the devicetree, and creates other child nodes.
A typical test would be to enumerate the nodes created.

<!-- Reference links -->

[devicetree-spec]: https://devicetree-specification.readthedocs.io/en/v0.3/index.html
[nodes]: /docs/concepts/drivers/drivers_and_nodes.md
[devicetree-examples]: /sdk/lib/driver/devicetree/examples/
[smc-yaml]: /sdk/lib/driver/devicetree/visitors/default/smc/smc.yaml
[dt-schema]: https://github.com/devicetree-org/dt-schema
[standard-properties]: https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#standard-properties
[visitors]: /docs/development/boards/devicetree-visitors.md
[writing-new-visitor]: /docs/development/boards/devicetree-visitors.md#write-a-new-visitor
[default-visitors]: /docs/development/boards/devicetree-visitors.md#default-visitors
[driver-visitors]: /docs/development/boards/devicetree-visitors.md#driver-visitors
[board-driver]: /docs/development/boards/bringup.md#board-driver
[load-visitors]: /docs/development/boards/devicetree-visitors.md#helper-libraries
[devicetree-manager]: /sdk/lib/driver/devicetree/manager/
