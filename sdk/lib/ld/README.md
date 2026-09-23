# Fuchsia Dynamic Linker

**TODO(https://fxbug.dev/42072830):** This is a work in progress. When finished, it
will be included in the SDK as a prebuilt ld.so binary but not as source.

This is the implementation of the core of the Fuchsia Dynamic Linker, but that
means three different things:
 1. The startup dynamic linker.
 2. The quasi-stable layout of the "passive ABI".
 3. A library of reusable code for working with the passive ABI.
    This includes pieces for client code (such as in a C library)
    consuming the passive ABI, as well as pieces for implementing
    dynamic linking itself compatibly with the startup dynamic linker's
    semantics (such as for out-of-process dynamic linking).  There is also a
    separate library that assists in writing related [gtest]-based tests.

Only the startup dynamic linker binary is published in the SDK.
The passive ABI is shared between this binary and other binaries
included in the SDK (libc, libdl), but is not generally public
in the SDK.

[gtest]: https://github.com/google/googletest

## Passive ABI

The central pillar of the design is the "passive ABI", whose details are
defined here.  This enables both generalized runtime traditional (in-process)
dynamic linking (`<dlfcn.h>` API), and out-of-process dynamic linking, neither
of which is implemented here.  Instead, the core implementation provides only a
"startup" dynamic linker for the traditional in-process dynamic linker
requested via the `PT_INTERP` program header in an ELF executable.  This
populates a "passive ABI" of data structures that are read-only after startup.
Once the startup dynamic linker passes control to the main executable's entry
point, its code is not used again.  Its only exported symbols provide the
"passive ABI" of data structures that describe the ELF modules (executable and
its transitive set of `DT_NEEDED` requirements) as they were loaded.

This ABI is only "quasi-stable".  It's not intended to be a long-term stable
ABI that will be directly used by application code.  It's an ABI shared between
the startup dynamic linker, libc, and libdl (the runtime dynamic linker).  In
the Fuchsia package model, these all travel together and have lockstep versions
copied from an SDK into binary packages.  However, out-of-process use cases
could make this a longer-term ABI between the loading environment and the libc
binaries in packages it loads.

The passive ABI consists of C++ data structures that have well-defined ABI
layouts independent of any changeable whims of the C++ compiler.  The core of
these are just the ELF format types, whose ABI is well-known and stable.  The
passive ABI defines some types that point to ELF format types in the memory
image of a loaded ELF module.  These point to the details necessary for
enumerating the modules, finding their images in memory, calling their
initializers and finalizers, and looking up exported symbols.

['<lib/ld/abi.h>`](include/lib/abi.h) and the headers it refers to declare
these types and symbols.  All these types are defined using a combination of
the ELF format types (integers and structs of integers) and pointers defined
via the `elfldltl:AbiPtr` abstraction.  This just has the ABI of a normal
pointer in the passive ABI, but it's defined in a way that can be used with the
[remoting support](#Remoting_support) to ensure that an out-of-process dynamic
linker can safely and correctly populate the data structures in a different
address space.

Note that these incorporate some of the [`elfldltl`](/src/lib/elfldltl) API
types.  Many of those types are just representations of the standard ELF ABI
types.  A few others are just collections of details gleaned during loading and
dynamic linking that are useful in the passive ABI, such as
`elfldltl::SymbolInfo` and `elfldltl::InitFiniInfo` (and others).  Changes to
those classes in the toolkit can change the passive ABI, so care must be taken
to maintain the toolkit code with the `//sdk/lib/ld` code in mind.

### Passive ABI Support Library

**TODO(https://fxbug.dev/42072830):** doesn't exist yet

### C++ namespace

The `ld::abi` namespace is used for the types and symbols that form the passive
ABI itself.  Though they're declared in the `ld::abi` C++ namespace scope, the
only ELF symbol names (external linkage symbols) generated are `extern "C"`
symbols with unmangled names that all begin with the `_ld_` prefix.  (The
`_r_debug` linkage symbol is also defined here, but this is not formally
considered part of the passive ABI per se.  It's not meant to be used directly,
but only by debuggers.)

The wider `ld` namespace is used for both the implementation pieces of the
startup dynamic linker and for reusable library code for working with the
passive ABI.  None of these types or symbols should ever be exported into any
shared library ABI.

### Remoting Support

**TODO(https://fxbug.dev/42072830):** This will be implemented as part of the
out-of-process dynamic linking support, but doesn't exist it.

## Startup Dynamic Linker

The `PT_INTERP` dynamic linker or "startup dynamic linker" implementation is
what bootstraps a traditional dynamically-linked program.  The system program
loader loads this dynamic linker and starts the process at its entry point
rather than that of the main executable.  In Fuchsia, the system program loader
loads _only_ the dynamic linker, which is then responsible for loading the main
executable.  In other systems like Linux, the system program loader loads both
the main executable and the dynamic linker, which is given some some necessary
details like the executable's address and entry point.

In this implementation, the `PT_INTERP` dynamic is better called the startup
dynamic linker because it operates only at startup.  Once initial dynamic
linking is done, the dynamic linker exists in memory only to provide the
passive ABI.  Additional runtime loading and dynamic linking can be done by a
separate runtime dynamic linker (libdl) that uses the startup dynamic linker's
passive ABI to prime its initial state.

## Stub Dynamic Linker

The "stub" dynamic linker is not really a dynamic linker, but it has the same
ABI (`DT_SONAME` and symbols) as the startup dynamic linker.  An out-of-process
dynamic linker uses the stub dynamic linker as the prototype for populating the
passive ABI when it sets up a dynamic linking domain in another address space.

## Testing Support Library

Header files [`<lib/ld/testing/*.h>`](include/lib/ld/testing) provide
interfaces in the `ld::testing` C++ namespace.  These are things used in the
tests for the startup dynamic linker and that can be reused for tests of other
kinds of dynamic linking implementations, such as out-of-process setups
intended to be compatible with the passive ABI of the startup dynamic linker.
