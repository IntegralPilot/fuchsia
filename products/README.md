# Fuchsia product definitions

This directory contains definitions for a variety of product configurations. The
products are organized into tracks that build on one another to produce a fully
featured system.

## Baseline track

### Bringup

A tiny system used to exercise kernel and core drivers and to bring up new
boards.

### Minimal

"The smallest set of features we'd still call Fuchsia." More feature-ful than
bringup (implying bringup is not a full Fuchsia system).

Per [RFC-0220], minimal can boot to userspace, run Component Manager and
components, and update itself over a network. It has no graphics, audio, wifi,
or other bread-and-butter items you'd think of in a consumer operating system.

Minimal is intended to be the basis for all production Fuchsia products, which
should not subtract from its contents, only add to them.

The main uses of minimal should be:
1. running [hermetic][test-hermeticity] tests
2. creating larger Fuchsia products

### Core (deprecated per [RFC-0220])

Self-updating system with core system services, connectivity, and metrics
reporting. Supports graphics.

Builds on [bringup](#bringup)

### Workbench

Based on minimal. Intended to be a figurative workbench for developers working
with Fuchsia. Supports graphics, media, audio, input, complex drivers like USB
and WiFi, etc. This contains the features you'd think of a production OS having,
but this product configuration is not meant for production. Instead, it's meant
to be a development and testing environment.

Workbench doesn't have a session (user interface) by default, though a developer
can add one at runtime using `ffx session launch`.

## Workstation track

### Terminal (deprecated per [RFC-0220])

A system with a simple graphical user interface with a command-line terminal.

Builds on [core](#core).

[test-hermeticity]: https://fuchsia.dev/fuchsia-src/development/testing/components/test_runner_framework#hermeticity
[RFC-0220]: /docs/contribute/governance/rfcs/0220_the_future_of_in_tree_products.md
