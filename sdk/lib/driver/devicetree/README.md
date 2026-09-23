# Fuchsia Devicetree Overview

Devicetree is a data structure for describing hardware. See
<https://devicetree-specification.readthedocs.io/en/v0.3/index.html> for more information.

In Fuchsia, devicetree can be used by the board driver to create driver framework nodes and set
configurations needed to initialize the board. This directory contains two parts that can be used by
the board driver to construct a devicetree parser.

1. Devicetree manager library - This library is responsible for parsing the devicetree blob and
   creating a runtime node structure which can be accessed by the visitors.
2. Devicetree visitors - Visitors are objects that can be provided to the devicetree manager by the
   board driver during the |Walk| call. Each visitor's interface which will get invoked on all nodes
   of the devicetree when walking the tree. They are responsible to convert the devicetree data into
   driver specific metadata and bind rules.

## Board driver integration

The board driver should initialize the devicetree manager library with the incoming namespace in
order to provide connection to |fuchsia_boot::Items| protocol. The manager reads the devicetree
using the |fuchsia_boot::Items| protocol. The board driver can invoke the |Walk| method of the
manager to parse the devicetree blob and collect node properties. During the walk call, several
visitors can be provided to parse and transform node properties into Fuchsia driver metadata and
bind rules. Once the walk is completed, the board driver invokes |PublishDevices| to publish all the
driver framework devices/nodes. Check out the `examples` folder for reference implementation.

## Devicetree Bindings and Visitors

Devicetree bindings describes the requirements on the content of a devicetree node pertaining to a
specific device or a class of device. They are documented using devicetree schema files, which are
YAML files that follow a certain meta schema. See <https://github.com/devicetree-org/dt-schema> for
more information.

[TODO](https://fxbug.dev/42083284) Add devicetree schema validation support.

Devicetree Visitors, as mentioned before can be used with the devicetree manager to parse and
extract information from all/specific devicetree nodes. The devicetree specification lists a set of
[standard
properties](https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#standard-properties),
some of which are parsed by the default visitors in the `visitors/default` folder. A new visitor
will have to be developed for any new bindings that need to be parsed. Currently these visitors need
to manually coded ([bug](https://fxbug.dev/42083285)), but might be auto generated from the schema
files in the future. See `visitors/README.md` for more details on how to write a visitor.

The default visitors are compiled into a static library that can be built with the board driver. All
other visitors are built as a shared library using the `devicetree_visitor` GN target. Creating a
shared library of the visitor helps to keep the list of visitors dynamic i.e. visitors can be added
and removed from the board driver without having to recompile it. This helps to update and
contribute visitors independent of the board driver. The board driver can include the necessary
visitor collection in it's package under `/pkg/lib/visitors/`. At runtime the board driver can load
all these visitors using the `load-visitors` helper library.

## Passing the DTB

Typically the devicetree blob (DTB) is passed down by the bootloader to the kernel as a
|ZBI_TYPE_DEVICETREE| item and made available to the board driver via |fuchsia_boot::Items| protocol.
In boards where the bootloader is not yet capable of passing the DTB (typically during board
bringup), the DTB can be passed in through board configuration's devicetree field, and later it will
be appended to ZBI by assembly.
## Testing

A board driver integration test can be added to test that the parsing and creation of nodes by the
devicetree libraries and visitors. The board-test-helper library can be used to create a test realm
with driver framework, platform bus and the board driver components running in it. The DTB is passed
as a test data resource and is provided to the board driver through the fake |fuchsia_boot::Items|
protocol implemented in the `board-test-helper` library. The test creates a platform bus device with
specified VID, PID to which the board driver will bind to. The board driver would then invoke
devicetree manager parses the devicetree and creates other child nodes. A typical test would be to
enumerate the nodes created.
