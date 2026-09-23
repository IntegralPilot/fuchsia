# Fuchsia C library startup

## Phases of program startup

The following sections describe the phases of program startup from the program
loader through to the program's `main` function, as implemented in this C
library and (when using the _shared_ C library) a compatible dynamic linker.

### Program loader

The program loader provides a stack and calls the entry point.

   It does not initialize the thread pointer or the shadow-call-stack pointer,
   both of which are necessary for the Fuchsia Compiler ABI.  (The thread
   pointer is used to access the SafeStack unsafe stack pointer, among other
   things.)  That is, the entry point must use only the basic machine ABI.

 * The protocol is the C(++) calling convention for a signature of:
   `[[noreturn]] void(zx_handle_t bootstrap, const void* vdso_base)`.

### Startup dynamic linker

When the program loader detects a `PT_INTERP` in the executable ELF file, it
doesn't process that file any further.  (The "interpreter" in the ELF headers
usually means the startup dynamic linker--it's a program that "interprets" the
executable ELF file somehow.)  Instead it loads the ELF file that the service
for the [`fuchsia.ldsvc`](/sdk/fidl/fuchsia.ldsvc) FIDL protocol returns given
the `PT_INTERP` string (e.g. `ld.so.1`).

This also makes it use a variant of the process bootstrap protocol that sends a
different message intended for the dynamic linker (including the original
executable file's VMO and a session with that same FIDL service it can use to
get shared library ELF files).  (This precedes the message meant for the
program itself, which is handled the the C library code here.)
**TODO(https://fxbug.dev/326312148):** _Future work will replace the legacy
process bootstrap protocol and refine the `fuchsia.ldsvc` protocol it relays.
This will simplify some matters in the startup flow but not change the overall
structure described here for the most part._

For a dynamically-linked executable (one with a `PT_INTERP`), the program
loader's call above uses the startup the dynamic linker's entry point.  The
dynamic linker does its work so the executable and libraries (usually including
libc.so) are loaded and their relocations resolved; unwinds back to the
starting stack pointer and argument registers; and then jumps to the
executable's entry point.

Its work is all done using only the basic machine ABI, so thread and
shadow-call-stack registers are untouched throughout (still zero from process
start).  The dynamic linker uses a different, but compatible, signature for the
entry point than the program loader:

```
[[noreturn]] void(zx_handle_t bootstrap, const void* vdso,
                  zx_handle_t svc_server_end);
```

This is fully compatible because all the arguments fit into the standard
registers, and the program loader (really `zx_process_start`) always starts
with zero in all registers other than the first two.

### Static PIE

For a static PIE, the program loader starts at the executable's own entry point
right away.  If the PIE needs any relocation, that must be done by code reached
from that entry point via only directly-linked pure PIC paths (no use of PLT,
GOT, or other RELRO).  (Later sections describe how the statically-linked C
library's startup code achieves this.)

The third argument register always starts as zero, so the `svc_server_end`
argument is seen as ZX_HANDLE_INVALID.  From the perspective of the entry point
per se, there's no immediate distinction between being a static PIE and being
started via a dynamic linker.

The process bootstrap protocol sent by the program loader reflects the static
PIE case only very slightly.  There is no dynamic-linker message (no VMO for
the executable, and no `fuchsia.ldsvc` session) as for the `PT_INTERP` case,
only the primary protocol meant for the program itself.  But that primary
protocol also includes a VMAR handle not in the post-dynamic-linker version.
This lets the self-relocating PIE apply RELRO protections to itself (as the
startup dynamic linker has already done in the dynamic linking cases).
**TODO(https://fxbug.dev/326312148):** _The future bootstrap protocol will
likely handle this differently, to more clearly distinguish "hand off from
basic ELF program loader" (in either static PIE or PT_INTERP variant) from
"main program startup" (after any ELF or dynamic-linking related setup)._

### Remote dynamic linking

A compatible [remote dynamic linker](/sdk/lib/ld/#remoting-support) is meant to
appear indistinguishable to the program's code at runtime from a compatible
[startup dynamic linker](/sdk/lib/ld/#startup-dynamic-linker).  In this case,
the executable's own entry point will be used in `zx_process_start` directly as
for a static PIE.  But both the dynamic linking semantics; as well as the
process bootstrap protocol meant for the program itself, and handled by the
shared C library; should be essentially identical to what the executable's
entry point would have experienced using the startup dynamic linker instead.
**TODO(https://fxbug.dev/326312148):** _In the future bootstrap protocol
mentioned in the previous section, the remote dynamic linking case will likely
simply omit the initial ELF-related message from the program loader (as the
ELF-reification work it's meant to enable is done from outside the process
before it starts); it only ever sees the "main program startup" message(s)._

The arguments after the first two will always be zero from `zx_process_start`,
since there is never anything like instrumentation data from the startup
dynamic linker to be published via `svc_server_end` (if the remote dynamic
linker service has instrumentation data to publish, that's elsewhere in a
different process).

The "root" VMAR handle used for memory allocation and the like by the C
library, or other capabilities from the process bootstrap protocol, might be
more constrained sandbox capabilities than a process using the traditional
program loader and startup dynamic linker where e.g. new executable mappings
need to be allowed after process start.  Such things might be disallowed for a
process under some remote dynamic linking regime by default.  (In fact, a
static PIE likewise might never need a VMAR handle with rights to make further
executable mappings in its own process.)

### Executable entry: `_start`

Either right at process start, or (roughly transparently) after handoff from a
dynamic linker, the main executable's entry point runs in the same way.  This
is usually the `_start` function (unless linker options say otherwise).  That
function usually comes implicitly from an object injected by the compiler
driver (unless switches say otherwise).  That object is the [`Scrt1.o`](crt1.S)
file that always comes paired with the C library.

* The main executable's entry point runs.  It receives the full argument
  signature described above (arguments after the second sometimes being
  implicit zero).

  Whether dynamically linked or a static PIE, it has its own entry point code.
  An executable linked in the standard way will have its entry point at
  `_start` as defined in [`Scrt1.o`](crt1.S).  That same object file is linked
  into any executable, whether it links statically against `libc.a` or
  dynamically against `libc.so`.  See detailed comments in the [source](crt1.S)
  on the nuances of one object file supporting both dynamic linking and static
  PIE (which really means limited dynamic linking that _hasn't happened yet_).

* `extern "C" _start` is a normal `[[noreturn]]` function but it uses only the
   basic machine ABI (no thread pointer and no shadow call stack register).
   The argument list represents what the dynamic linker and/or program loader
   handoff provides in argument registers.  Its implementation is always
   trivial.  It just calls `__libc_start_main`, passing along those arguments
   plus an additional argument that's the pointer to the standard C `main`
   function ([or equivalent jump target](crt1.S)).  This defers all the setup
   work to actually call `main` to libc code, whether statically linked or in
   the shared library.  But it keeps the `main` symbol local to the executable
   so that symbol name is not visible to dynamic linking (unless some other
   link-time factors cause it to be exported, as with any function; or `main`
   itself is found only in a shared library).

### C library entry: `__libc_start_main`

`extern "C" __libc_start_main` transitions from the basic machine ABI to the
Fuchsia Compiler ABI by allocating space for the various stacks and the thread
pointer area.  [`startup-trampoline.h`](startup-trampoline.h) explains more
thoroughly.  In summary, assembly code calls into C++ code that hermetically
uses only the basic machine ABI to do all that setup and unwind fully back to
the assembly code with its entering stack pointer restored.  After this, the
assembly code is ready to start over with the Fuchsia Compiler ABI fully
available (thread pointer initialized enough for the full ABI, though not yet
for all libc internals; and shadow call stack available).

#### Machine stack switching

Due to the legacy process bootstrap protocol's stack size rules for the startup
dynamic linker case, the shared C library version of this first function must
switch to a new machine stack.  In that case, the C++ code has allocated a new
stack and the assembly code switches to it before "starting over".

_Note: This is done unconditionally by the shared C library code even though in
[remote dynamic linking](#remote-dynamic-linking) cases, the service taking the
place of the program loader does respect the executable's stack size setting._

**TODO(https://fxbug.dev/326312148):** _One of the goals of the future
bootstrap protocol's design will be to make the system program loader handle
the initial machine stack details (sizing, guard area, etc.) more uniformly
across static PIE and PT_INTERP cases such that the entire stack-switching case
can be "unwound" from the libc code. However, the current design that unwinds
back to the entry point as if switching machine stacks even if not doing so is
still probably desirable just for getting a final state of fully-aligned
machine stack and shadow call stack state for backtraces, as detailed in the
next sections._

### I'll come in again: _another_ `__libc_start_main`

This is the second phase of C library startup--and of `__libc_start_main`.

* It's [all C++](start-main.cc) and it uses the full Fuchsia Compiler ABI.

  It expects at least whatever stack size the executable's `PT_GNU_STACK`
  specified, which the startup dynamic linker and first phase code cannot
  presume (due to the legacy process bootstrap protocol with the program
  loader, hence the stack switching there).

  It has access to libc-internal page-wise memory allocation via global state
  already initialized (and just used for stacks et al in phase one)--though not
  yet "normal" libc state with initialization or anything leading to `malloc`.

  The state of CFI (unwind info), frame pointers, and shadow call stack for
  this call will all show it as the direct callee of the executable's entry
  point code (`_start`).  The exported `__libc_start_main` (known as the
  [trampoline](#trampoline)) has done a proper tail call to the later (hidden,
  not exported) `__libc_start_main`.  The real work of program startup begins.

  If `svc_server_end` was passed by the dynamic linker, it should now be sent
  on to the `/svc` name table entry as a pipelined open/reopen/clone.  The
  [`fuchsia.debugdata`](/sdk/fidl/fuchsia.debugdata) protocol is used by the
  startup dynamic linker for its own instrumentation data, though it didn't
  have the name table.

* The function then does the rest of libc initialization; calls static
  constructors; and finally calls `main` (via the function pointer originally
  passed by `_start`).  If `main` returns, its return value goes to `exit`,
  which never returns.

### Unwinding considerations

The `_start` code and the first phase of `__libc_start_main` in the C library
both use only the basic machine ABI, so there is never a shadow call stack.
However, all that code meticulously maintains both precise frame pointers and
precise DWARF / `.eh_frame` CFI state throughout.

In particular, it's maintained as an invariant that the outermost frames of the
full backtrace will be `_start` -> `__libc_start_main` throughout and will
resolve properly and identically via either frame pointers or CFI all along if
single-stepped from the entry point at `_start` (or if interrupted or faulting
anywhere in there).

Once `__libc_start_main` has established the Fuchsia Compiler ABI invariants,
another invariant is added that shadow call stack backtraces match the two
basic machine ABI methods (modulo usual shadow call stack leaf fuzziness).
This guarantee of matching three-method backtraces is in place before any C++
static constructors or [`<zircon/sanitizer.h>`](../zircon/sanitizer.h) hook
calls, and before the [fdio](/sdk/lib/fdio) setup that precedes constructors.

Immediately at the point of reestablishing invariants in the full-ABI context,
there is a tail call from `__libc_start_main` to another `__libc_start_main`.
This does not directly affect unwinding, as with any tail call: a backtrace is
either inside the original function or inside the tail-called function, with
the same caller frame in either case (here `_start`).

Once constructors are done, there is the actual call to `main`.  At this point,
the code again maintains a strict invariant that the _raw_ backtrace (by any
method) will always show `_start` -> `__libc_start_main` -> `main` with no
intervening (real) stack frames.  (See below about _symbolized_ backtraces.)

#### Symbolization considerations

The first `__libc_start_main` is the [assembly](startup-trampoline.S)
"trampoline" function: the _actual_ callee of `_start` (well, not counting the
likely PLT entry in the executable that was a different trampoline first!).
Being in assembly, it has only plain a ELF symbol with `extern "C"` linkage and
no DWARF entry.  This symbolizes just as its plain global name, no signature.
(Source locations in the assembly code will be symbolized normally, however.
That aspect of DWARF is actually wholly separate from identifying functions.)
But this function is only observable at all when stepping through the earliest
startup phases (or debugging some fault or profile trace inside there).  It
would never appear in a backtrace from application code.

The second function runs with the full ABI (and on its new versions of all the
stacks).  It's the C++ function `__fuchsia_libc::__libc_start_main`, which we
call phase two of `__libc_start_main`.  With debug information access, this has
its full details described in DWARF and a symbolizer may display it with or
without namespace qualification and with or without argument signature.  If
neither, then it would look just like plain `__libc_start_main` in a symbolized
backtrace (as in phase one).

Both shared and static C libraries are always built with both frame pointers
(except for leaf functions) and precise CFI, and everything except the phase
one startup code is built using the Fuchsia Compiler ABI and thus uses the
shadow call stack as well (except for leaf functions).  Backtraces by all
methods should match; so symbolization from any is the same at any point.

Backtraces out of static constructors or sanitizer hooks can show frames
between the callee and out to (phase two) `__libc_start_main` for internal libc
implementation functions.  The application or sanitizer runtime code will be at
least indirect callees of `__libc_start_main` of both the raw backtrace frames
and any inlined function virtual frames, but no further guarantees except for
correct unwinding (always) and correct symbolization (given libc DWARF data).

Once `main` is reached, the _raw_ backtrace frame after the `_start` frame will
symbolize as `__libc_start_main` (but the phase two C++ one, perhaps scoped).
However, there are in fact _guaranteed inlined_ functions `__libc_start_main`
itself calls to actually call `main`, which can become visible via DWARF data.

If `main` returns, the implicit `exit` call will similarly be a "direct" callee
of `__libc_start_main` in the _raw_ backtrace.

##### Virtual frames for inlined calls

The callers of `main` are very careful (using `[[gnu::always_inline]]` et al)
to ensure that `__libc_start_main` is the "direct" caller of `main` in the raw
backtrace (and then of `exit`).  Furthermore, all these inline functions are
annotated with `[[gnu::artificial]]`.  This is meant to set `DW_AT_artifical`
on the inline function's DWARF entry.  This is traditionally set on "hidable"
uninteresting functions such as certain kinds of checking wrappers for standard
functions (it also makes any compile-time error messages elide the inlined
function as if the message applied to the inline's call site instead).

Ideally the symbolizer will notice `DW_AT_artifical` flags and suppress showing
all such virtual frames "in between" true call frames when symbolizing a
backtrace.  Users see a lot of backtraces that all go back to `main` and
beyond.  The libc implementation details are not interesting to most users, and
can change in ways that become startling or are in danger of getting baked into
some test's expectations about backtraces.  So the best default backtrace
experience is to see the predictable `_start` -> `__libc_start_main` -> `main`
backtrace frames symbolized with simple names and without exposing further libc
implementation details.

But it's not entirely clear yet what the various symbolizers all do.

## Implementation structure

### `LIBC_ASM_LINKAGE`

The `LIBC_ASM_LINKAGE` family of macros are used internally in the libc
implementation and defined in the [`asm-linkage.h`](../asm-linkage.h) header.
These provide something akin to C++ namespace scoping that can reasonably be
used in assembly code and build rules.  Their trivial "name-mangling" scheme
just prepends the libc namespace and an underscore to identifiers.

This is used a fair bit in the startup code, because of the small amount of
assembly code and the larger amount of code that must go into hermetic partial
links because of ABI issues, as described in the next section.

### `basic_abi` in `libc_source_set()`

It's essential that code using only the basic machine ABI remain intentionally
isolated from code using the full Fuchsia Compiler ABI as most libc code does.
The first phase of startup code must use only the basic machine ABI.

The [`hermetic_source_set()`](/build/toolchain/hermetic_source_set.gni) build
mechanism uses hermetic partial links to isolate code at link time.  In the
libc build rules, this is used by setting the `basic_abi` flag when defining a
`libc_source_set()`.  That requires listing all the undefined symbols that code
will refer to outside its own `deps` graph, and all the global symbols it will
define to be visible to the outside link.  Since ELF symbol names must be
listed manually in `BUILD.gn` files, any libc-internal symbols that must be
used across these boundaries use `LIBC_ASM_LINKAGE(name)` in C++ and assembly
code, and use `"${libc_namespace}_name"` in GN code.

### `_start`

The `_start` function is implemented entirely in assembly in `crt1.S`(crt1.S).
This is basic machine ABI code that's linked statically into each executable.

### Trampoline

The true `__libc_start_main` entrypoint is just an assembly "trampoline",
implemented in [`startup-trampoline.S`](startup-trampoline.S).  It first calls
`LIBC_ASM_LINKAGE(StartCompilerAbi)` using the basic machine ABI.  That sets up
the thread pointer and shadow call stack pointer before it returns.  Part of
its return value is the new machine stack pointer to run on.  So the trampoline
installs that stack and then tail-calls `LIBC_ASM_LINKAGE(start_main)` (i.e.,
phase two), which can use the full Fuchsia Compiler ABI.

### Static PIE self-relocation

The first thing that `StartCompilerAbi` has to do is ensure that system calls
can be made.  In the static PIE case, this is where it must perform its own
self-relocation.  That's handled by the [`StartupRelocate`](startup-relocate.h)
object.  In the shared library, its has [no-op stubs methods](stub-relocate.cc)
because the (startup or remote) dynamic linker already dealt with it.

In the static library, the [real methods](static-pie-relocate.cc) perform
simplified dynamic linking.  This code must take pains to be pure PIC: not
relying on the runtime dynamic linking work it must do itself.  It applies
simple fixup relocations to the executable's data and RELRO segments, and
resolves any symbolic dynamic linking references therein.  Only the vDSO system
call functions can be referenced (no other dependencies are possible).

That same code, used only in the static library, defines and initializes the
[passive ABI](/sdk/lib/ld/#passive-abi) symbols.  This stands in for what a
full dynamic linker would provide for dynamic linking against the (startup or
remote stub) dynamic linker.  That's not present--only the executable itself
and the vDSO.  Instead, they are provided as normal `STV_HIDDEN` symbols in the
executable's own RELRO segment.  From the (static) link-time perspective of
libc (or other) code using the passive ABI, it's just the same either way:
after `StartupRelocate` is all done, references to those `extern const`
variables find read-only memory with the right contents.  In the static PIE
case, the passive ABI reports the only two ELF modules: the executable itself,
and the vDSO (whereas the dynamic linker cases will also report `libc.so` and
`ld.so.1` and probably many more in between those two).

### Custom bootstrap protocol hooks

See [`<zircon/startup.h>`](../include/zircon/startup.h).  The API functions
there are not public or interposable for the shared library.  But the common
implementation is maintained as the single place across static and shared
library cases to encapsulate all knowledge of a process bootstrap protocol, and
things in that protocol's terms such as [fdio](/sdk/lib/fdio) startup.  In a
static PIE, these can be replaced while tying into the rest of libc startup as
described here without regard to all these internal details.  In the shared
library's `__libc_start_main` path, they are baked in and unavoidable.

The [legacy bootstrap protocol](/zircon/system/public/zircon/processargs.h) is
handled by the current implementation of those API functions, split between
phases [one](processargs-get-handles.cc) and [two](processargs-preinit.cc).

**TODO(https://fxbug.dev/326312148):** _Support for a future replacement
protocol can be confined to replacing these default/baked-in implementations of
the existing `<zircon/startup.h>` API in those two files with new counterparts
for each phase._
