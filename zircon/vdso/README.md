# FIDL library zx

FIDL library `zx` describes Fuchsia's syscall interface.

This library is used internally to generate syscall definitions used by other
code. For the public library imported by `using zx;`, see
[`//zircon/vdso/zx/`](/zircon/vdso/zx/).

If you're adding a new syscall or a new Zircon object, see
[Adding a New Zircon Syscall][adding-a-new-zircon-syscall].

The modeling of the syscall interface is currently undergoing a slow evolution
toward the design described in [RFC 0190][rfc-0190]. This will involve migrating
more of the APIs in this library to the public library.

The current version of this library - an expedient and temporary measure dubbed
"v2" - purposefully overfits to modeling the C vDSO interface.
[issue 42061642][42061642] describes the reasoning and methodology of the
current "v2" approach. [Issue 42061412][42061412] tracks the process of
realizing `zx` as _pure_ FIDL (without experimental language features - see
[issue 42061642][42061642]) as described in [RFC 0190][rfc-0190] and framing
things in a more general way with [_"bring your own runtime"_][byor] in mind.

## Conventions & Quirks

### Naming

Library `zx` adheres to standard FIDL [style guide][fidl-naming].

### Experimental types

Per the 'C overfit' v2 design, several C-like types are temporarily introduced
to library `zx` behind the experimental `zx_c_types` flag:

* `experimental_pointer<T>`: represents a pointer (to a type `T`) and has the
FIDL ABI of a `uint64`.

* `usize64`: represents the size of a region of memory and has the FIDL ABI of
`uint64`. Its binding in Fuchsia-targeting C and C++ code is meant to be
`size_t`.

* `uintptr64`: an integral type of sufficient size so as to be able to hold a
pointer, practically regarded as an address in a remote address space. It has
the FIDL ABI of `uint64` and its binding in Fuchsia-targeting C/C++ code is
intended to be `uintptr_t`.

* `uchar`: represents an opaque, unsigned, 8-bit 'character' (e.g., ASCII or a
UTF-8 code point) and has the FIDL ABI of `uint8`. Its binding in
Fuchsia-targeting C/C++ code is intended to be `char`.

### Representation of syscalls

Currently lacking a means of declaring syscalls in FIDL in a first-class
manner, we use the following convention for declaring a logical grouping of
syscalls:

```
@transport("Syscall")
protocol NounPhrase {
    VerbPhrase(struct{...}) -> (struct{...}) error status;

    ...
};
```

This method corresponds to `zx_${noun_phrase}_${verb_phrase}()`: its 'in'
parameters are given in order in the request struct and its 'out' parameters
given in order in the response struct. The `error status`  clause can be dropped
in the case of syscalls that do not return `zx_status_t`.

See [@no_protocol_prefix](#no-protocol-prefix) for a possible protocol
annotation.

See [@blocking](#blocking), [@const](#const), [@internal](#internal),
[@next](#next), [@noreturn](#noreturn), and [@vdsocall](#vdsocall),
[@testonly, @test_category1, and @test_category2](#testonly-test_category1-test_category2)
for possible method annotations.

### Representation of buffers

We represent the buffers of caller-owned memory passed into syscalls as
`vector`s, implicitly representing separate data and length parameters.

See [@embedded_alias](#embedded_aliasalias_name), [@size32](#size32), and
[@voidptr](#voidptr) for possible annotations.

Syscall buffer specification should be holistically designed in the context
of [issue 42061412][42061412].

### Documentation

TODO(https://fxbug.dev/42061412): Have `fidldoc` emit the syscall documentation
(//docs/reference/syscalls/) in a more first-class way. Currently, the content
of those markdown pages are expected to appear as the docstrings of the
associated FIDL syscall declarations.

TODO(https://fxbug.dev/42061412): Settle on a convention for how official syscall
documentation should refer to its FIDL source-of-truth. For now, we ignore
the FIDL representation and refer solely to the associated C bindings.

### Special attributes

#### @blocking

Annotates a syscall declaration to indicate that the calling thread is blocked
until the call returns.

This should be formalized as something known to and validated by `fidlc` - or
redesigned altogether - in the context of [issue 42061412][42061412].


#### @const

Annotates a @vdsocall-decorated syscall declaration to indicate that the
function is "const" in the sense of `__attribute__((__const__))`.

This information is not a part of public ABI - relevant only to implementation
details - and should be designed away in the context of
[issue 42061412][42061412].


#### @embedded_alias("<alias_name>">)

Annotates a `vector` or `experimental_pointer` whose element/pointee type is an
alias. This attribute serves to expediently inject the name of the alias into
the related IR. Only a full resolution of an alias survives into the IR today,
a bug which is tracked by [issue 42057022][42057022]. Once resolved, this
attribute should be straightforwardly removed.


#### @handle_unchecked

Annotates a handle as a syscall parameter to indicate that it is
released/consumed by that call. Similarly so for a vector of handles.

This should be formalized as something known to and validated by `fidlc` - or
redesigned altogether - in the context of [issue 42061412][42061412].


#### @inout

Annotates a syscall parameter to indicate - with the C bindings in mind - that
it should be treated as both an 'in' and an 'out' parameter. If applied to a
vector, the implicit data parameter is regarded as an out parameter, while the
implicit size parameter as regarded as an 'in'.

This notion should be redesigned more holistically in the context of
[issue 42061412][42061412].


#### @internal

Annotates a "syscall" declaration to indicate that the call is not a part of
public ABI and describes internal vDSO logic.

This information is not a part of public ABI - relevant only to implementation
details - and should be designed away in the context of
[issue 42061412][42061412].


#### @next

Annotates an element to indicate that the feature is not yet 'well-baked' and
whose should not be unconditionally distributed in the SDK.

This should be formalized as something known to and validated by `fidlc` - or
redesigned altogether - in the context of [issue 42061412][42061412].


#### @no_protocol_prefix

Annotates a protocol representing a family of syscalls to indicate that the
name of the protocol should not be factored in to the name of its constituent
syscalls. In this case, the protocol name is arbitrary and the syscalls members
include the would-be family namespacing directly into their names. This is
expedient in the cases where the would-be protocol name clashes with that of
another FIDL element in the library.

For example, `zx_clock_read()` would naturally be represented - in `zx` v2 - as
```
@transport("Syscall")
protocol Clock {
    ...

    Read(resource struct {
        handle handle;
    }) -> (struct {
        now time;
    }) error status;

    ...
};
```
However, `zx.Clock` already exists as an enum, preventing us from declaring a
protocol with that same name. So instead we spell this as
```
@no_protocol_prefix
@transport("Syscall")
protocol ClockFuncs {  // An arbitrary name.
    ...

    ClockRead(resource struct {
        handle handle;
    }) -> (struct {
        now time;
    }) error status;

    ...
};
```

This is a hack and the collisions in question should be whittled down over the
course of [issue 42061412][42061412]. At that point, this attribute should go
away.


#### @noreturn

Annotates a syscall declaration to indicate that the call will not return.

This should be formalized as something known to and validated by `fidlc` - or
redesigned altogether - in the context of [issue 42061412][42061412].


#### @out

Annotates a syscall parameter in the request struct to indicate - with the C
bindings in mind - that it actually should be treated as an 'out' parameter.

This notion should be redesigned more holistically in the context of
[issue 42061412][42061412].

#### @release

Annotates a handle as a syscall parameter to indicate that it is
released/consumed by that call. Similarly so for a vector of handles.

This should be formalized as something known to and validated by `fidlc` - or
redesigned altogether - in the context of [issue 42061412][42061412].


#### @size32

Annotates vector syscall parameters to indicate that the implicit size
parameter is 32-bit.

Syscall buffer specification should be holistically designed in the context
of [issue 42061412][42061412].


#### @testonly, @test_category1, @test_category2

These are test-specific and it should be rethought in the context of
[issue 42061412][42061412] whether these elements should be defined in `zx`
proper.


#### @vdsocall

Annotates a syscall declaration to indicate that the call does not actually
enter the kernel and is properly defined within the vDSO.

This information is not a part of public ABI - relevant only to implementation
details - and should be designed away in the context of
[issue 42061412][42061412].


#### @voidptr

Annotates `experimental_pointer<byte>` or `vector<byte>` to indicate to C
backends that the mapped types should be represented with `void*`.

This should be redesigned altogether in the context of
[issue 42061412][42061412].

#### @wrapped_return

Annotates a _singleton_, syscall response struct, indicating that the syscall's
return type is actually the type of the contained parameter.

As an example, consider `uint32_t zx_system_get_num_cpus()`:
```
@transport("Syscall")
protocol System {
    @const
    @vdsocall
    GetNumCpus() -> (@wrapped_return struct {
        count uint32;
    });

    ...
};
```

This gives a workaround the limitation of protocol methods only being able to
return a struct and should be sidestepped an ultimate design for syscall
specification that does not have to piggy back off of protocols
([issue 42061412][42061412]).

[adding-a-new-zircon-syscall]: /docs/development/kernel/adding_a_new_syscall.md
[42057022]: https://fxbug.dev/42057022
[42061412]: https://fxbug.dev/42061412
[42061642]: https://fxbug.dev/42061642
[byor]: https://fuchsia.dev/fuchsia-src/concepts/principles/simple
[fidl-naming]: https://fuchsia.dev/fuchsia-src/development/languages/fidl/guides/style#names
[rfc-0190]: https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0190_fidl_support_for_syscalls
