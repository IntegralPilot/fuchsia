# Build configuration

A build configuration uses a combination of a [**product**][products-source] and a [**board**][boards-source] (which are GN includes) to provide a baseline configuration for a Fuchsia build.

It is expected that a GN build configuration include exactly one board GNI
file, and one product GNI file. In [fx][fx] this pair is the primary argument
to the `fx set` command.

In a Fuchsia GN build configuration the board is always included first. The
board starts the definition of three dependency lists that are then augmented
by the imported product (and later, optional GN inclusions). Those list are
[Base](#base), [Cache](#cache) and [Universe](#universe) respectively, and
are defined below.

## Boards

A [board](/docs/glossary/README.md#board) defines the architecture that the
build produces for, as well as key features of the device upon which the
build is intended to run. This configuration affects what drivers are
included, and may also influence device-specific kernel parameters.

The available boards can be listed using `fx list-boards`.

Note: The generic `arm64` board configuration is intended for physical target
hardware and does not support running in the emulator. To run Fuchsia in the
emulator for ARM64, you must use the `qemu-arm64` board configuration instead.

## Products

A product defines the software configuration that a build will produce. Most
critically, a product typically defines the kinds of user experiences that
are provided for, such as what kind of graphical shell the user might
observe, whether or not multimedia support is included, and so on.

The available products can be listed using `fx list-products`.

## Dependency Sets

Boards define, and products augment three lists of dependencies, Base, Cache
and Universe. These dependencies are GN labels that ultimately contribute
packages to various system artifacts, such as disk images and signed package
metadata, as well as various development artifacts such as host tools and
tests.

### Base

The `base` dependency list contributes packages to disk images and system
updates as well as the package repository. A package included by the `base`
dependency set takes precedence over a duplicate membership in the `cache`
dependency set. Base packages in a system configuration are considered system
and security critical. They are updated as an atomic unit and are never
evicted at runtime regardless of resource pressure.

### Cache

The `cache` dependency list contributes packages that are pre-cached in the
disk image artifacts of the build, and will also be made available in the
package repository. These packages are not added to the system updates, but
would instead be updated ephemerally. Cached packages can also be evicted
from running systems in order to free resources based on runtime resource
demands.

### Universe

The `universe` dependency list contributes packages to the package repository
only. These packages will be available for runtime caching and updating, but
are not found in system update images nor are they pre-cached in any disk
images. All members of `base` and `cache` are inherently also members of
`universe`.

## Key Product Configurations

There are many more product definitions than those listed below, but the
following four products are particularly important configurations to be familiar
with:

### Bringup {#bringup-product}

The `bringup` product is the most minimal viable target for development.
Because it lacks most network capabilities, the `bringup` product
cannot use the `fx` commands, such as
<code>[fx serve](/docs/development/build/fx.md#serve-a-build)</code> and
<code>[fx shell](/docs/development/build/fx.md#connect-to-a-target-shell)</code>,
that require network connectivity.

For more see [Bringup product definition](/docs/development/build/software_assembly/bringup.md)

### Core {#core-product}

`core` is a minimal feature set that can install additional software (such as
items added to the "universe" dependency set). It is the starting point for
all higher-level product configurations. It has common network capabilities
and can update a system over-the-air.

### Minimal

As described in
[RFC-0220](/docs/contribute/governance/rfcs/0220_the_future_of_in_tree_products.md),
Minimal is intended to be "the smallest thing which can be called Fuchsia."
Definitionally, this is a system which can:

* Boot into userspace.
* Run Component Manager and components.
* Update itself using Fuchsia's over-the-air update system. (This implies
  that storage and networking are working, with drivers provided by the
  board.)

### Workbench
Workbench is a product configuration for local development, running larger
tests which cannot or should not be packaged hermetically, and exercising larger
parts of the Fuchsia platform than minimal supports. It is intended to be like a
literal workbench in that it supports development tools and allows a developer
to poke at the system and make changes. It is not intended to be a product that
ships to users, or be a basis for those products.






[products-source]: /products/
[boards-source]: /boards/
[fx]: /docs/development/build/fx.md
[fx-netboot]: /docs/development/build/fx.md#what-is-netbooting
[fx-paving]: /docs/development/build/fx.md#what-is-paving
[fx-serve]: /docs/development/build/fx.md#serve-a-build
[fx-shell]: /docs/development/build/fx.md#connect-to-a-target-shell
