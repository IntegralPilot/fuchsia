# Getting started with developing `ffx` subtools

FFX Subtools are the top-level commands that the [ffx cli](/docs/development/tools/ffx/architecture/cli.md)
can run. These can be either compiled directly into `ffx` and/or build as separate
commands that can be found in the build output directory or the SDK, and they
will then be invoked using the [FHO tool interface](/docs/development/tools/ffx/architecture/fho.md).

Note: The tool produced by this document will only be runnable as an external subtool.
The `ffx` maintainers will generally not be accepting new top level tools as being integrated into
the `ffx` binary unless there's a very strong reason for it, in which case please
ask the FFX team for guidance.

## Where to put it

First, create a directory somewhere in the fuchsia.git tree to hold
your subtool. Currently subtools exist in these locations:

* The [ffx plugins tree](/src/developer/ffx/plugins), where built-in only and
hybrid plugin/subtools go. New subtools should not generally be put here.
* The [ffx tools tree](/src/developer/ffx/tools), where external-run-only subtools
go. Putting it here makes it easier for the maintainers of `ffx` to assist with any issues or
update your subtool with any changes to the interface between `ffx` and the tool.
If you put it here and the FFX team isn't the primary maintainer of this tool,
you *must* put an `OWNERS` file in the directory you put it in that adds your
team's component and some individual owners so we know how to triage issues with
your tool.
* Somewhere in a project's own tree. This can make sense if the ffx tool is
simply a wrapper over an existing program, but if you do this you *must*
have your `OWNERS` files set up so that the FFX team can approve updates to the
parts that interact with `ffx`. You can do this by adding `file:/src/developer/ffx/OWNERS`
to your `OWNERS` file over the subdirectory the tool is in.

Other than not putting new tools in plugins, the decision of a specific location
may require discussion with the tools team to decide on the best place.

## What files

Once you've decided where your tool is going to go, create the source
files. Best practices is to have your
tool's code broken out into a library that implements things and a `main.rs` that simply calls into that library.

The following file set would be a normal starting point:

```
BUILD.gn
src/lib.rs
src/main.rs
OWNERS
```

But of course you can break things up into more libraries if you want. Note that
these examples are all based on the example [echo subtool](/src/developer/ffx/tools/echo),
but parts may be removed or simplified for brevity. Take a look at the files in
that directory if anything here doesn't work or seems unclear.

### `BUILD.gn`

Following is a simple example of a `BUILD.gn` file for a simple subtool. Note
that, if you're used to the legacy plugin interface, the `ffx_tool` action doesn't
impose a library structure on you or do anything really complicated. It's a fairly
simple wrapper around the `rustc_binary` action, but adds some extra targets
for generating metadata, producing a host tool, and producing sdk atoms.

```gn
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/developer/ffx/tools/echo/BUILD.gn" %}
```

### `main.rs`

The main rust file will usually be fairly simple, simply invoking FHO with
the right types to act as an entry point that `ffx` knows how to communicate
with:

```rust
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/developer/ffx/tools/echo/src/main.rs" %}
```

### `lib.rs`

This is where the main code of your tool will go. In here you will set up an
argh-based struct for command arguments and derive an `FfxTool` and `FfxMain`
implementation from a structure that will hold context your tool needs to run.

#### Arguments

```rust
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/developer/ffx/tools/echo/src/lib.rs" region_tag="command_struct" %}
```

This is the struct that defines any arguments your subtool needs after its
subcommand name.

#### The tool structure

```rust
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/developer/ffx/tools/echo/src/lib.rs" region_tag="tool_struct" %}
```

This is the structure that holds context your tool needs. This includes things
like the argument structure defined above, any proxies to the daemon or a
device you might need, or potentially other things that you can define yourself.

There must be an element in this struct that references the argument type
described above, and it should have the `#[command]` attribute on it so that
the correct associated type can be set for the `FfxTool` implementation.

Anything in this structure must implement the `TryFromEnv` or have a `#[with()]`
annotation that points to a function that returns something that implements
`TryFromEnvWith`. There are also several implementations of these built in to
the `fho` library or other `ffx` libraries.

Also, the `#[check()]` annotation above the tool uses an implementation of
`CheckEnv` to validate that the command should be run without producing an item
for the struct itself. The one here, `AvailabilityFlag`, checks for
experimental status and exits early if it's not enabled. When writing a new
subcommand, it should have this declaration on it to discourage people from relying
on it before it's ready for wider use.

##### Direct Connections
Target connections are direct by default. The `#[target(direct)]` declaration may be added for explicit subtool declaration:
```rust
#[derive(FfxTool)]
#[target(direct)]
pub struct MyTool {
    ...
}
```

##### No Target Connection
If a subtool does not require a target connection, it may declare this with
`#[target(None)]`:
```rust
#[derive(FfxTool)]
#[target(None)]
pub struct MyTool {
    ...
}
```

This is only _required_ when a command supports `ffx --strict`, which will
verify that the target is specified on the command line, unless the tool has
this declaration. However, it is good practice to add this attribute to any tool
that does not interact with a target device.

## FIDL protocols {#fidl-proxy}

FFX subtools can communicate with a target device using FIDL protocols through
[Overnet][overnet]. To access FIDL protocols from your subtool:

1. Add the FIDL Rust bindings as a dependency to the subtool's `BUILD.gn` file.
    The following example adds bindings for the `fuchsia.device` FIDL library:

    ```gn
      deps = [
        "//sdk/fidl/fuchsia.device:fuchsia.device_rust",
      ]
    ```

1. Import the necessary bindings into your subtool implementation. The following
    example imports `NameProviderProxy` from `fuchsia.device`:

    Note: The struct `NameProviderProxy` is generated as part of the rust
     bindings to the FIDL `fuchsia.device`,
     [`NameProviderProxy`](/sdk/fidl/fuchsia.device/name-provider.fidl)

    ```rust
    use fidl_fuchsia_device::NameProviderProxy;
    ```

1.  Declare a member field of the tool structure. Since FIDL proxies implement
  the `TryFromEnv` trait, the FHO framework will create and initialize the field
  for you.

    ```rust
    #[derive(FfxTool)]
    pub struct EchoTool {
      #[command]
      cmd: EchoCommand,
      name_proxy: NameProviderProxy,
    }
    ```

#### The `FfxMain` implementation

```rust
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/developer/ffx/tools/echo/src/lib.rs" region_tag="tool_impl" %}
```

Here you can implement the actual tool logic. You can specify a type for the
[`Writer`](writers.md) associated trait and that type will (through `TryFromEnv`) be
initialized for you based on the context `ffx` is run in. Most new subtools should
use the `MachineWriter<>` type, specifying a less generic type than the example
`String` above, but what makes sense will vary by tool. In the future, it may
be required that all new tools implement a machine interface.

Also, the result type of this function defaults to using the fho `Error` type,
which can be used to differentiate between errors that are due to user
interaction and errors that are unexpected. More information on that can be found
in the [errors](errors.md) document.

#### Unit tests {#unit-tests}

If you want to unit test your subtool, just follow the standard method for
testing [rust code][rust-testing] on a host. The `ffx_plugin()` GN template
generates a `<target_name>_lib_test` library target for unit tests when the
`with_unit_tests` parameter is set to `true`.

If your `lib.rs` contains tests, they can be invoked using `fx test`:

```posix-terminal
fx test ffx_example_lib_test
```

If fx test doesn't find your test, check that the product configuration includes
 your test. You can include all the ffx tests with this command:

```posix-terminal {:.devsite-disable-click-to-copy}
fx set ... --with=//src/developer/ffx:tests
```

#### Using fake FIDL proxy in tests

A common patten for testing subtools is to create a fake proxy for a FIDL
 protocol. This allows you to return the full variety of results from calling
 the proxy without actually having to deal with the complexity of an integration
 test.

```rust
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/developer/ffx/tools/echo/src/lib.rs" region_tag="fake_proxy" %}
```

Then use this fake proxy in a unit test

```rust
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/developer/ffx/tools/echo/src/lib.rs" region_tag="echo_test" %}
```

### `OWNERS`

If this subtool is in the `ffx` tree, you will need to add an `OWNERS` file that
tells us who is responsible for this code and how to route issues with it in
triage. It should look something like the following:

```OWNERS
file:/path/to/authoritative/OWNERS
```

It's better to add it as a reference (with `file:` or possible `include`) than
as a direct list of people, so that it doesn't get stale due to being out of the
way.

## Adding to the build

To add the tool to the GN build graph as a host tool, you'll need to reference it
in [the main list](/src/developer/ffx/tools/BUILD.gn) in the `ffx` tools gn file,
added to the `public_deps` of both the `tools` and `test` groups.

After this, if you `fx build ffx` you should be able to see your tool in the list
of `Workspace Commands` in the output of `ffx commands` and you should be able
to run it.

### Experimental subtools and subcommands

It's recommended that subtools initially do not include an `sdk_category` in
their `BUILD.gn`. These subtools without a specified category are considered
"internal", and they will not be part of SDKs. If users want to use
the binary, they will have to be given the binary directly.

Subcommands, however, are handled differently.

Subcommands need an `AvailabilityFlag` attribute added to the tool (see a commit
from the history of [`ffx target update`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/developer/ffx/plugins/target/update/src/lib.rs;l=19)
for an example). If users want to use a subcommand, they will need to set the
associated config option in order to invoke that subcommand.

However, there are problems with this approach, such as a lack of any verification
of the FIDL dependencies of the subcommand. Therefore, the mechanism for handling
subcommands is currently being changed as of December, 2023.

Similar to subtools, subcommands will be able to declare their SDK category
to determine whether the subcommands are available in SDKs.
The subtool will be built with only the subcommands at or above the subtool’s category
level. The FIDL dependency check will correctly verify the subcommand’s requirements.

## Adding to the SDK

Once your tool has stabilized and you're ready to include it in the SDK, you'll
want to add the binary to the SDK build. Note that before doing this, the tool
must be considered relatively stable and well tested (as much as possible without
having already included it in the SDK), and you need to make sure youhave considered
compatibility issues.

### Compatibility

There are three areas that you need to be aware of before adding your subtool to
the SDK and IDK:

1. FIDL libraries - You are required to add any FIDL libraries you are dependent
   on to the SDK when you add a subtool to the SDK. (For details, see
   [Promoting an API to `host_tool`][promoting-an-api-to-host_tool].)

2. Command line arguments - In order to test for breaking changes due to command line
   option changes, the [ArgsInfo] derive macro is used to generate a JSON representation
   of the command line.

   This is used in a [golden file test][golden-file-test] to detect
   differences. Golden files. Eventually, this test will be enhanced to detect and warn
   of changes that break backwards compatibility.

3. Machine friendly output - Tools and subcommands need to have machine output
   whenever possible, especially for tools that are used in test or build scripts.
   The [MachineWriter] object is used to facilitate encoding the output in JSON format
   and providing a schema that is used to detect changes to the output structure.

   Machine output must be stable along the current compatibility
   window. Eventually, there will be a golden check for the machine output format.
   The benefit of having machine writer output is that it frees you up to have
   unstable output in free text.

### Updating the subtool

To add your subtool to the SDK, set the `sdk_category` in its `BUILD.gn` to the
appropriate category (for instance, `partner`).  If the subtool includes subcommands
that are no longer experimental, remove their `AvailabilityFlag` attributes so
that they will no longer require a special config option to invoke.

### Inclusion in the SDK

You also need to add your subtool to the `host_tools` molecule of the
[SDK GN file][sdk-gn-file], for example:

```gn {:.devsite-disable-click-to-copy}
sdk_molecule("host_tools") {
  visibility = [ ":*" ]

  _host_tools = [
    ...
    "//path/to/your/tool:sdk", # <-- insert this
    ...
  ]
]
```

### UX and SDK review

Subtools are required to follow the
[CLI Guidelines](/docs/development/api/cli.md).
<!-- Reference links -->

[ArgsInfo]: https://github.com/google/argh/blob/e901d3a1cc285db9740e0e68a1e4225234377015/argh/src/lib.rs#L338
[ffx-target-update]: /src/developer/ffx/plugins/target/update/src/lib.rs
[golden-file-test]: /src/developer/ffx/tests/cli-goldens/README.md
[MachineWriter]: /docs/development/tools/ffx/development/subtools/writers.md
[overnet]: /src/connectivity/overnet/
[promoting-an-api-to-host_tool]: /docs/contribute/sdk/README.md#promoting-to-host-tool
[sdk-gn-file]: /sdk/BUILD.gn
[rust-testing]: /docs/development/languages/rust/testing.md
