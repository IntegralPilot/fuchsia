# zxdb: The Fuchsia debugger

Zxdb is a console-mode debugger for native code running on Fuchsia.

It primarily supports C, C++ and Rust code, but any language that compiles
natively and exports DWARF symbols should work to some extent.

Interpreted code such as Dart and JavaScript is not supported.

## Run zxdb

There are several ways to attach `zxdb` to a component through
`ffx component debug`:

Note: If you have any issues with launching `zxdb`, see
[Troubleshooting][zxdb-troubleshooting].

```posix-terminal
ffx component debug {{ '<var>' }}component-identifier{{ '</var>' }}
```

For example, this command attaches to the component called `cobalt.cm`:

```posix-terminal
ffx component debug cobalt.cm
```

This shows an output like:

```none {:.devsite-disable-click-to-copy}
Waiting for process matching "job 20905".
Type "filter" to see the current filters.
👉 To get started, try "status" or "help".
Attached Process 1 state=Running koid=21057 name=cobalt.cm component=cobalt.cm
Loading 15 modules for cobalt.cm ...Done.
[zxdb]
```

Note: For more information about component identifiers, see
[component identifiers][component-identifiers].

You can also use other types of component identifiers such as a component
moniker.

For example, this example attaches to the `core/cobalt` component moniker:

Note: For more information about component monikers, see
[monikers][component-monikers].

```posix-terminal
ffx component debug core/cobalt
```

Additionally, you can also use shorter component identifiers, such
as `cobalt`:

```posix-terminal
ffx component debug cobalt
```

However, if `zxdb` can't figure out which component you are referring
to, you may see an output like:

```none {:.devsite-disable-click-to-copy}
The query "cobalt" matches more than one component instance:
core/cobalt
core/cobalt_system_metrics

To avoid ambiguity, use one of the above monikers instead.
```

In this case make sure to be specific as to which component you want `zxdb` to
attach to.


### Using a Fuchsia package URL

You can also use the full Fuchsia package URL:

Note: For more information about Fuchsia package URLs, see
[component URLs][component-urls].

```posix-terminal
ffx component debug fuchsia-pkg://fuchsia.com/cobalt#meta/cobalt.cm
```

This shows an output like:

```none {:.devsite-disable-click-to-copy}
Waiting for process matching "job 20905".
Type "filter" to see the current filters.
👉 To get started, try "status" or "help".
Attached Process 1 state=Running koid=21057 name=cobalt.cm component=cobalt.cm
Loading 15 modules for cobalt.cm ...Done.
[zxdb]
```

### See available components

You can see a full list of available components:

```posix-terminal
ffx component list
```

You should see an output like:

```none {:.devsite-disable-click-to-copy}
...
bootstrap
bootstrap/archivist
bootstrap/archivist/archivist-pipelines
...
core/brightness_manager
core/build-info
core/cobalt
...
```

## Working with zxdb

Once you have successfully connected to the `zxdb` debugger, you may want to:

* [Get started with zxdb][zxdb-commands]
* [Use breakpoints][zxdb-breakpoints]
* [Debug tests using zxdb][zxdb-tests]
* [Control thread execution][zxdb-execution] (pausing, stepping, and resuming)
* [Evaluate expressions][zxdb-expressions]
* [Inspect memory][zxdb-memory]
* [List handles and kernel objects][zxdb-kernel-objects]
* [Work with assembly language][zxdb-assembly]
* [Look at handles][zxdb-kernel-objects]
* [Understand how symbols are loaded][zxdb-symbols]
* [Work with exceptions][zxdb-exceptions]
* [See advanced zxdb topics][zxdb-advanced]

You can also follow the zxdb tutorials:

* [Tutorial: Debug tests using zxdb][zxdb-testing-tutorial]
* [Tutorial: Debug with minidumps using zxdb][zxdb-minidumps-tutorial]

Additionally, if you are interested in contributing to zxdb:

* [Developing and debugging zxdb][zxdb-developing]

[zxdb-troubleshooting]: /docs/development/debugger/troubleshooting.md
[zxdb-commands]: /docs/development/debugger/commands.md
[zxdb-advanced]: /docs/development/debugger/advanced.md
[zxdb-developing]: /docs/development/debugger/developing.md
[zxdb-execution]: /docs/development/debugger/execution.md
[zxdb-breakpoints]: /docs/development/debugger/breakpoints.md
[zxdb-expressions]: /docs/development/debugger/expressions.md
[zxdb-memory]: /docs/development/debugger/memory.md
[zxdb-assembly]: /docs/development/debugger/assembly.md
[zxdb-kernel-objects]: /docs/development/debugger/kernel_objects.md
[zxdb-symbols]: /docs/development/debugger/symbols.md
[zxdb-exceptions]: /docs/development/debugger/exceptions.md
[zxdb-tests]: /docs/development/debugger/tests.md
[zxdb-testing-tutorial]: /docs/development/debugger/tutorial-tests.md
[zxdb-minidumps-tutorial]: /docs/development/debugger/tutorial-minidumps.md
[component-urls]: /docs/reference/components/url.md
[component-identifiers]: /docs/concepts/components/v2/identifiers.md
[component-monikers]: /docs/concepts/components/v2/identifiers.md#monikers