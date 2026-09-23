# Adding a New Zircon Syscall (...or a New Zircon Object Type)

This doc is a short guide to adding a new Zircon syscall.  The intended audience
is kernel developers or anyone who is adding a new syscall.

Keep in mind that this guide only covers the *mechanics* of adding
syscall/objects.  It's not a style guide, rubric, and does not provide any
advice on how to know when an API is stable enough or mature enough to become
part of the system interface.

Because our system continues to change, you should expect that this guide is out
of date.  Sorry.  If you are following this guide and you notice errors or
omissions, please make an attempt to update it.  Thank you!


## Overview

There are a number of ways you can structure the changes to add a new Zircon
syscall (or object).  This guide will describe an approach consisting of four
CLs: stub, implementation, extras, and annotations.

We're going to follow the "rewrite history" method.  Zircon syscalls do not
([currently][345252431]) follow Platform Versioning so instead we're going to
make it look as if the new syscall had been there all along.  To do this, we'll
need to update some SDK history files for frozen API levels.

### Stub CL: Start with a stub at HEAD

In this CL, you'll define the syscall and provide a stub implementation that
simply returns `ZX_ERR_NOT_SUPPORTED`.  Be sure to include a core-test in this
stub CL that verifies the caller doesn't crash and that the new call returns the
error as expected.

The build system and FIDL compiler will automatically generate some of the
boilerplate for you.  To take advantage, you'll want to get in the habit of
performing a default build, `fx build`, and running `fx check-goldens` after you
make changes to FIDL files.

To illustrate, let's look at a [stub CL][stub-cl] that adds both a new Zircon
object (`counter`) and a new syscall (`zx_counter_create`).

The stub CL has four parts.

#### 1. Define the syscall using FIDL

Next, [define the new syscall][counter-fidl] in an existing `.fidl` file or add
a new file if appropriate.  Be sure to add a commented out
`@available(added=HEAD)` annotation that indicates the new syscall (or object if
adding a new Zircon object) is added at HEAD.

In the rarer event of adding a new Zircon object, you'll need to take a few
extra steps.

a. Teach the FIDL compiler about the new object type.  The FIDL toolchain bakes
in knowledge of object types. This isn't elegant, but it avoids some circular
dependencies. You'll have to update parts of the FIDL toolchain to understand
your new object type. In particular, update:

- `HandleSubtype` in [`//tools/fidl/fidlc/src/properties.h`][fidlc-properties]
- `NameHandleSubtype` in [`//tools/fidl/fidlc/src/names.cc`][fidlc-names]
- `HandleType` in
[`//tools/fidl/abi-compat/src/compare/handle.rs`][fidl-compare]
- `handle-subtype` in [`//tools/fidl/fidlc/schema.json`][fidlc-schema]
- `GoodHandleSubtype` in
[`//tools/fidl/fidlc/tests/types_tests.cc`][fidlc-types-tests]
- `HandleSubtype` and `ObjectType` consts in
[`//tools/fidl/lib/fidlgen/types.go`][fidlgen-types]
- `handleSubtypes`, `handleSubtypesFdomain` and `objectTypeConsts` in
[`//tools/fidl/fidlgen_rust/codegen/ir.go`][fidlgen-ir]

b. Update the `fidlc` goldens by following the instructions provided in the build
error you get when you build without updating them.

c. Add the new object type to [zircon/types.h][zircon-types-header].

d. Update the `//sdk/history/*/zx.api_summary.json` [files][api-summary]. These
are for the public `zx` FIDL library, which shares `zx_common.fidl` with the
syscall library.

e. [Extend docsgen][gen-syscalls-toc] as necessary.

#### 2. Implement the syscall in the kernel

Implement the syscall [somewhere in
zircon/kernel/lib/syscalls/][counter-syscall-impl], with a prefix of `sys_`
instead of `zx_`.  For now, the implementation should simply return
`ZX_ERR_NOT_SUPPORTED`.

Note: The `sys_` function's signature isn't necessarily the same as the `zx_`
function's signature.  If you're unsure of the right `sys_` signature you run a
build and check the resulting
`$root_build_dir/fidling/gen/zircon/vdso/zx/zither/kernel/lib/syscalls/kernel.inc`
file.  Or check the build output for a linker error that tells you the missing
`sys_` function's expected signature.

#### 3. Add a lib/zx C++ wrapper

[Add a new or extend an existing lib/zx wrapper][counter-wrapper] in order to
provide a C++ API for the new syscall.  Be sure to apply a
`ZX_AVAILABLE_SINCE(HEAD)` annotation to your new C++ method (or class if it's a
new Zircon object).  While the syscall itself will appear to have been present
from the very beginning, the C++ wrapper is added at HEAD.

#### 4. Add a core-test

[Add a core-test][counter-core-test] that verifies the stub returns the right
error value (and does not crash the system).  By using the new `lib/zx` wrapper
in this test, you can be sure it also works as expected.

Once you've gotten the stub to build and pass CQ, land it.  Next, move on to the
implementation CL.

#### 5. Add FIDL language bindings for new Zircon objects

To enable the use of a new Zircon object (i.e. `zx.Handle:COUNTER`) in the C++
and Rust language bindings, there are a few additional areas that must be
updated. These updates allow the language binding code generators to know how to
handle a new Zircon object.

For FIDL C++ language bindings, update:

- `ShortObjTypeName` in [`//src/lib/fidl_codec/display_handle.cc`][fidlcodec-display-handle]
- `DisplayObjType` in [`//src/lib/fidl_codec/printer.cc`][fidlcodec-printer]
- Add object header to [`//sdk/lib/fidl/hlcpp/include/lib/fidl/cpp/internal/header.h`][fidlcpp-header]
- `handleSubtypeConsts` in [`//tools/fidl/lib/fidlgen_cpp/handles.go`][fidlgencpp-handles]
- `zxNames` in [`//tools/fidl/lib/fidlgen_cpp/name_transforms.go`][fidlgencpp-name-transforms]

For FIDL Rust language bindings, update:

- `ObjectType` in [`//sdk/rust/zx/src/handle.rs`][rust-handle]
- `invoke_for_handle_types` in [`//src/lib/fuchsia-async/src/handle/mod.rs`][fasync-handle]

### Implementation CL: Filling in the stub

Now it's time to replace the stub implementation with a real one, add real tests
and real documentation.  Built it, pass CQ, land it, and move on.

### Extras CL: Add the rest

Once you've got a working implementation with C/C++ bindings, you're now ready
to add [Rust bindings][rust], and update `(fidl_codec` (used by `fidlcat`):
- `ShortObjTypeName` in [`display_handle.cc`][codec-handle]
- `PrettyPrinter::DisplayObjType` in [`printer.cc`][codec-printer]

The above list of "extras" may be incomplete so this is a good time look around
ad other syscalls to see if there are more things you need to update (tools?
bindings for other languages?).  Land the extras CLs and move on to the final
step.

### Annotations CL: Change HEAD to NEXT

Now that the implementation and extras have landed, go back and change the value
you added in the `@available` and `ZX_AVAILABLE_SINCE` annotation(s) from `HEAD`
to `NEXT` to make the syscall available in the next stable API level.


<!-- TODO(https://fxbug.dev/383761360): Update the links below to use code
search rather than CL in gerrit. -->

[345252431]: https://fxbug.dev/345252431
[stub-cl]: https://fxrev.dev/1181176
[fidlc-properties]: /tools/fidl/fidlc/src/properties.h
[fidlc-names]: /tools/fidl/fidlc/src/names.cc
[fidl-compare]: /tools/fidl/abi-compat/src/compare/handle.rs
[fidlc-schema]: /tools/fidl/fidlc/schema.json
[fidlc-types-tests]: /tools/fidl/fidlc/tests/types_tests.cc
[fidlgen-types]: /tools/fidl/lib/fidlgen/types.go
[fidlgen-ir]:/tools/fidl/fidlgen_rust/codegen/ir.go
[zircon-types-header]: https://fuchsia-review.googlesource.com/c/fuchsia/+/1181176/9/zircon/system/public/zircon/types.h
[api-summary]: https://fuchsia-review.googlesource.com/c/fuchsia/+/1181176/9/sdk/history/16/zx.api_summary.json
[gen-syscalls-toc]: https://fuchsia-review.googlesource.com/c/fuchsia/+/1181176/9/tools/docsgen/gen_syscalls_toc.py
[counter-fidl]: https://fuchsia-review.googlesource.com/c/fuchsia/+/1181176/9/zircon/vdso/counter.fidl
[counter-syscall-impl]: https://fuchsia-review.googlesource.com/c/fuchsia/+/1181176/9/zircon/kernel/lib/syscalls/counter.cc#13
[counter-wrapper]: https://fuchsia-review.googlesource.com/c/fuchsia/+/1181176/9/zircon/system/ulib/zx/include/lib/zx/counter.h
[counter-core-test]: https://fuchsia-review.googlesource.com/c/fuchsia/+/1181176/9/zircon/system/utest/core/counter/counter.cc
[rust]: /sdk/rust/zx
[codec-handle]: /src/lib/fidl_codec/display_handle.cc
[codec-printer]: /src/lib/fidl_codec/printer.cc
[fidlcodec-display-handle]: /src/lib/fidl_codec/display_handle.cc
[fidlcodec-printer]: /src/lib/fidl_codec/printer.cc
[fidlcpp-header]: /sdk/lib/fidl/hlcpp/include/lib/fidl/cpp/internal/header.h
[fidlgencpp-handles]: /tools/fidl/lib/fidlgen_cpp/handles.go
[fidlgencpp-name-transforms]: /tools/fidl/lib/fidlgen_cpp/name_transforms.go
[rust-handle]: /sdk/rust/zx/src/handle.rs
[fasync-handle]: /src/lib/fuchsia-async/src/handle/mod.rs
