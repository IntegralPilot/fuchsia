# Components

Components are the basic unit of executable software on Fuchsia.

## Architectural concepts

-   [Introduction](introduction.md): Understanding components and the component
    framework.
-   [Component manager](component_manager.md): The runtime.
-   [Lifecycle](lifecycle.md): Component instance progression from creation to
    destruction.
-   [Topology](topology.md): The relationships among component instances.
-   [Realms](realms.md): Sub-trees of the component instance topology.
-   [Identifiers](identifiers.md): Identifiers for components and
    component instances.

## Developing components

-   [Capabilities](capabilities/README.md): Different types of capabilities and
    how to route them between components.
-   [Component manifests](component_manifests.md): How to define a component for
    the framework.
-   [ELF runner](elf_runner.md): How to launch a component from an ELF file.
    Typically useful for developing system components in C++, Rust, or Go.

## Extending the component framework

-   [Runners](capabilities/runner.md): Instantiate components; add support for
    more runtimes.
-   [Resolvers](capabilities/resolver.md): Find components from URLs; add
    support for methods of software packaging and distribution.

## Internals

-   [Component manifest design principles][rfc0093]
-   [Components vs. processes](components_vs_processes.md): how the concepts
    differ.

[rfc0093]: /docs/contribute/governance/rfcs/0093_component_manifest_design_principles.md
