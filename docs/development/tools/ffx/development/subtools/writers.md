# Writers

The `ffx` subtool interface has the concept of a `Writer` that manages output
from the tool and the user. The different types of Writer are used to
distinguish between output intended to be read by interactive users and
structured output intended for use by other programs and scripts.

For the most part, you can use the writer as you would any implementation of
`std::io::Write`, and it's fine to just use `writeln!()` or similar built-in
macros against it for any unstructured string output.

All writers also implement some convenience functions for basic writing needs
like `print` and `line`. However, those convenience functions will only produce
output if the command is not run in machine mode.

For example:

```rust
writer.line("This is a human friendly message")?;
writer.machine(&theMessageObject)?;
```

Only one of those lines will be written, depending whether the
command is run in machine mode. If the command is not in machine mode, the
line "This is a human friendly message" will be printed. If the command is
in machine mode, the message referenced by `&theMessageObject` will be printed.

All writers also implement `item`, which will either print the object
given in machine mode, or use its `Display` implementation to output it in
non-machine mode as text.

The different implementations implement the [`ToolIO`][toolio] trait.
Depending on the how the subtool will be used, subtool authors should
use the appropriate implementation.

## SimpleWriter

The [`SimpleWriter`][simplewriter] struct is the most basic of the Writers.
It should be used when there is no need to support structured output and has
no restrictions or guidelines on how to use it. The advantage of a SimpleWriter
versus directly using stdio is that the SimpleWriter can be instantiated using
buffers which allows easier testing of output. To create the buffer backed
SimpleWriter see [`new_with_buffers()`][simple-new-with-buffers].

## MachineWriter

The [`MachineWriter`][machinewriter] struct builds on the SimpleWriter by
adding the implementations of methods to support structured output:

* `machine`: Only print the given object in machine mode.
* `machine_many`: Print the given objects in machine mode.
* `machine_or`: If in machine mode, print the object. Otherwise print the
   textual information in the other argument (implementing `Display`).
* `machine_or_else`: If in machine mode, print the object. Otherwise print the
textual information resulting from the function in the other argument.

The format of the output of `MachineWriter` is controlled by the top level
command line argument to ffx, `--machine`.

MachineWriter should be used when there is need to support programmatic
processing of the output. Since the `machine()` method is used to produce the
output, the subtool should use machine() for all possible output, including
errors when possible. This way the caller of the subtool can handle and present
errors that are encountered in a structured way, rather than falling back to
unstructured data on stderr.

To use MachineWriter, the declaration of the Writer attribute in FfxMain
requires an object type that implements the [`serde::Serialize`][serialize] trait.

## VerifiedMachineWriter

The [`VerifiedMachineWriter`][verifiedwriter] struct builds on the MachineWriter
behavior and adds JSON schema support. This schema is intended to describe the
structure of the output so it can be deserialized. The schema is also used to
detect changes over time to promote backwards compatibility with integrations
with other tools and scripts so eventually, there are no breakages due to
updating the ffx tools, or at least they are known and can be accommodated.

To use VerifiedMachineWriter, the declaration of the Writer attribute
in FfxMain requiresan object type that implements the
[`serde::Serialize`][serialize] trait and [`schemars::JsonSchema`][jsonschema].


## How to specify your `Writer` type

In this subtool interface, you specify this as an associated type on
the `FfxMain` trait for your tool, and the machine type is a generic
argument to the `MachineWriter` type. If your tool doesn't implement machine
output, it should use `SimpleWriter` instead of something like
`MachineWriter<String>` to avoid having people depend on your unstructured
output.

<!-- refs -->
[simplewriter]: /src/developer/ffx/lib/writer/src/simple_writer.rs
[simple-new-with-buffers]: /src/developer/ffx/lib/writer/src/simple_writer.rs
[machinewriter]: /src/developer/ffx/lib/writer/src/machine_writer.rs
[verifiedwriter]: /src/developer/ffx/lib/writer/src/verified_machine_writer.rs
[toolio]: /src/developer/ffx/lib/writer/src/lib.rs
[serialize]: https://docs.rs/serde/latest/serde/trait.Serialize.html
[jsonschema]: https://graham.cool/schemars/