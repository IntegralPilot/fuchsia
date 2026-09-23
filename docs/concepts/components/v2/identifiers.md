# Component identifiers

The Component Framework uses different identifiers to describe components.
This section describes the relationship between the following component
identifiers, and their usage:

-   [Component URLs](#component-urls): Identifies a component as a resource to
    be fetched by a component resolver.
-   [Monikers](#monikers): Identifies specific component instances in the
    component instance tree.

## Component URLs {#component-urls}

A [component URL][glossary.component-url] is a [URL][wiki-url] that locates a
component, including its declaration, program, and assets. Component Framework
uses [component resolvers][doc-resolvers] to resolve a component URL into a
[component declaration][doc-manifests-declaration].

### Usage

The primary use of component URLs is to identify a component in the definition
of a component instance, as part of a [child declaration][doc-manifests-children]:

```json5 {:.devsite-disable-click-to-copy}
{
    children: [
        {
            name: "logger",
            url: "fuchsia-pkg://fuchsia.com/logger#logger.cm",
        },
    ],
}
```

The above example declares the `logger` component as an absolute resource
in a [Fuchsia package][doc-package] hosted in a package repository.

Component Framework also supports relative URLs.

To identify a component built into the same package as the parent component,
specify only the URL fragment:

```json5 {:.devsite-disable-click-to-copy}
{
    children: [
        {
            name: "child",
            url: "#meta/child.cm",
        }
    ],
}
```

To identify a component in a [subpackage][doc-subpackaging] of the parent
component's package, include the subpackage name followed by the component
manifest path (via URL fragment):

```json5 {:.devsite-disable-click-to-copy}
{
    children: [
        {
            name: "child",
            url: "child#meta/default.cm",
        }
    ],
}
```

Relative component URLs are often used in tests, where the best practice is to
re-package production components in a test-specific package to promote
[hermeticity][test-hermeticity].

For more details on component URL syntax, see the
[component URL reference][url-reference].

## Monikers {#monikers}

A [component moniker][glossary.moniker] is a string that identifies a specific
component instance in the component instance tree using a topological path. It
follows similar semantics to `fuchsia.io` paths.

Each path element is the name assigned by a parent component to its child,
ultimately identifying the "leaf component" corresponding to the last path
element.

Monikers are always relative to something: a parent component, or the root of
the entire component topology.

### Usage

Some examples of component monikers:

- `.`: Self-referential moniker. For example, the root component (the
  first component launched by `component_manager`) can be referred to using
  this moniker. Other uses are context-dependent.
- `alice/carol/sandy`: Uniquely identifies the component instance
  "sandy" as the descendent of "alice" and "carol".
- `alice/support:dan`: Uniquely identifies the component instance "dan"
  as an element in the "support" collection descended from "alice".

Monikers are passed to developer tools, such as
[`ffx component explore`][component-explore], to identify specific component
instances on a target device. They also make up the first part of the
[diagnostic selectors][diagnostic-selectors] syntax.

Monikers are used by [developer tool implementations][component-select] to
interact with specific component instances on a target device.

For more details on component moniker syntax, see the
[component moniker reference][moniker-reference].

### Design principles

#### Stability

Monikers are stable identifiers so long as the component topology leading
to that component does not change.

#### Privacy

Monikers may contain privacy-sensitive information about other components that
the user is running.

To preserve the encapsulation of components, components are unable to
determine the moniker of other components running outside of their own
realm. Components cannot learn their own moniker, that of their parent, or
of siblings.

Monikers may appear in system logs and the output of developer tools.

[glossary.component-url]: /docs/glossary/README.md#component-url
[glossary.moniker]: /docs/glossary/README.md#moniker
[component-explore]: /docs/development/tools/ffx/workflows/explore-components.md
[component-select]: /docs/development/tools/ffx/commands/component-select.md
[diagnostic-selectors]: /docs/reference/diagnostics/selectors.md
[doc-manifests-children]: https://fuchsia.dev/reference/cml#children
[doc-manifests-declaration]: /docs/concepts/components/v2/component_manifests.md#component-declaration
[doc-package]: /docs/concepts/packages/package.md
[doc-subpackaging]: /docs/concepts/components/v2/subpackaging.md
[doc-resolvers]: /docs/concepts/components/v2/capabilities/resolver.md
[moniker-reference]: /docs/reference/components/moniker.md
[url-reference]: /docs/reference/components/url.md
[test-hermeticity]: /docs/development/testing/components/test_runner_framework.md#hermeticity
[wiki-url]: https://en.wikipedia.org/wiki/URL
