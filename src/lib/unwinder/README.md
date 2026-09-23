# Fuchsia Unwinder Library

The `unwinder` library provides a robust, flexible, and standalone mechanism for
unwinding call stacks. It is used heavily by Fuchsia's debugger (`zxdb`),
profiling tools, and crash reporting systems.

The library is primarily designed to support asynchronous or remote unwinding.
An asynchronous unwinder is an unwinder that operates out-of-process from the
thread being unwound. The synchronous `Unwinder` interface supports unwinding
any process for which synchronous read access to an ELF process's memory can be
provided. The `AsyncUnwinder` interface supports unwinding any ELF process for
which _asynchronous_ read access to the process's memory can be provided, even
on remote systems.

"Offline" unwinding is supported, is unrelated to the location of the process,
but to the location of the _unwinding metadata_, for example CFI provided by the
`.eh_frame` segment or `.debug_frame` section of an ELF binary.

The interface allows for anything that can fulfill the contract of the `Memory`
object to able to be unwound, including things such as core dump files with no
live process memory at all.

## Hybrid Unwinding

A key feature of this library is **Hybrid Unwinding**, which allows the unwinder
to switch between different unwinding strategies on a per-frame basis.

Most unwinders typically commit to a single strategy (e.g., either CFI or Frame
Pointers) for the entire stack. In contrast, this library attempts multiple
strategies in a prioritized order for *each* frame. If one strategy fails to
recover the next frame, the next strategy in the list is tried.

Because strategies like CFI, Frame Pointers, and ARM EHABI all restore the Stack
Pointer (SP) and Program Counter (PC), they can be interleaved seamlessly. For
example, if a stack contains a mix of code compiled with and without frame
pointers, or code with and without CFI metadata, the unwinder can still recover
the full call stack by switching strategies as needed. The unwinder allows for
both `.eh_frame` and `.debug_frame` style CFI. When present, `.debug_frame` will
be preferred from the provided Memory object, which should be referencing an
unstripped binary, typically with full debugging symbols. This can be provided
in either of the synchronous or asynchronous unwinding contexts as described in
more detail below.

## Unwinding Strategies

The unwinder employs multiple strategies to recover the call stack, falling back
to less reliable methods if the preferred ones fail. The `Frame::Trust` enum
indicates which strategy successfully recovered a given frame.

The general order of precedence for each frame is:
1. **SigReturn (`kSigReturn`)**: Specifically detects and unwinds through signal
   handler trampolines (e.g., recovering state from a `sigcontext` struct).
2. **Call Frame Information (`kCFI`)**: Parses DWARF CFI from `.eh_frame` or
   `.debug_frame` sections. This is the most reliable and accurate method for
   standard function calls.
3. **ARM Exception Handling ABI (`kArmEhAbi`)**: Parses `.ARM.exidx` and
   `.ARM.extab` sections. Used primarily for 32-bit ARM binaries.
4. **Procedure Linkage Table (`kPLT`)**: A specialized unwinder for the first
   frame when the instruction pointer is inside a PLT entry (where CFI is often
   inaccurate or missing).
5. **Frame Pointers (`kFP`)**: Walks the linked list of frame pointers (e.g.,
   `RBP` on x64, `X29` on ARM64).
6. **Shadow Call Stack (`kSCS`)**: Recovers the return address using the shadow
   call stack pointer (e.g., `X18` on ARM64). This only recovers the program
   counter (`PC`), as the stack pointer (`SP`) is lost. Subsequent frames will
   also be unwound via SCS.

## Core Concepts

### `Memory` An abstract interface for reading memory.

The library provides several implementations:

* `LocalMemory`: Reads from the current process's memory space.
* `FuchsiaMemory` / `LinuxMemory`: Reads from another process using OS-specific
  APIs.
* `FileMemory`: Reads directly from an ELF file on disk.
* `AsyncMemory`: Wraps a memory interface to support asynchronous memory
  fetching, which facilitates reading memory from a remote process.

### `ElfModuleCache` Manages the lifetime and lookup of `Module` objects.

It is responsible for mapping an address to the corresponding ELF module and
caching parsed unwind metadata (like CFI or ARM EHABI tables). Using a cache is
highly recommended when performing multiple unwinding operations to avoid the
overhead of re-parsing ELF files.

### `Module` & `LoadedElfModule`

A `Module` represents the location and identification of an ELF binary in
memory. `LoadedElfModule` contains the parsed and cached unwind information for
that specific module.

### `Registers`

An architecture-independent container for CPU registers.

### `Frame`

Represents a single unwound call frame. It contains:

* The `Registers` recovered for that frame (at minimum, the `PC` and `SP`).
* A `Trust` level indicating which strategy recovered it.
* A flag indicating if the `PC` represents a return address or a precise
  location (e.g., in the case of signal frames).

## Usage

Regardless of which interface you use (synchronous or asynchronous), it is
expected that the target thread remains suspended for the entire duration of all
unwinder interactions. This ensures that the memory and registers do not change
during the unwinding process.

### Synchronous Unwinding

For synchronous unwinding, use the free function `unwinder::Unwind`:

```cpp
#include "src/lib/unwinder/unwind.h"

// Set up memory and modules
unwinder::LocalMemory memory;
std::vector<uint64_t> modules = { /* base addresses of loaded modules */ };
unwinder::Registers registers = /* capture current registers */;

// Unwind!
std::vector<unwinder::Frame> stack = unwinder::Unwind(&memory, modules,
                                                      registers, /*max_depth=*/50);
```

If you need to unwind multiple times, it is more efficient to instantiate an
`unwinder::Unwinder` object, which will cache the parsed ELF module data
internally.

### Asynchronous Unwinding

When unwinding from a remote process, memory needs to be fetched incrementally.
`AsyncUnwinder` provides an asynchronous API to accommodate these scenarios.

The `AsyncUnwinder` implementation also employs "collaborative yielding," where
it invokes the completion callback after each successfully recovered frame. This
allows the caller to handle other asynchronous events or defer the remainder of
the unwinding if necessary, and prevents deep recursion on the host's stack.

The `AsyncUnwinder` object itself must be kept alive for the entire duration of
the asynchronous unwinding process.

```cpp
#include "src/lib/unwinder/unwind.h"

// Requires an implementation of unwinder::AsyncMemory::Delegate
unwinder::AsyncMemory::Delegate* delegate = ...;
std::vector<unwinder::Module> modules = ...;

auto unwinder = std::make_unique<unwinder::AsyncUnwinder>(delegate, modules);

unwinder->Unwind(registers, max_depth,
                 [unwinder = std::move(unwinder)](std::vector<unwinder::Frame> frames) {
  // Handle the unwound frames. The capture of `unwinder` keeps it alive.
});
```

### Direct Use of a Specific Unwinding Implementation

Individual unwinding strategies inherit from `UnwinderBase`. You can instantiate
a specific strategy (e.g., `CfiUnwinder`, `FramePointerUnwinder`) directly to
use only that implementation.

When doing so, you must instantiate an `ElfModuleCache` to pass to the unwinder.
**Crucially, you must ensure that both the `ElfModuleCache` and the specific
unwinder object remain alive for the entire duration of the unwinding
operation.**

#### Synchronous Example (Force CFI only)

```cpp
#include "src/lib/unwinder/unwind.h"
#include "src/lib/unwinder/cfi_unwinder.h"

unwinder::ElfModuleCache module_cache(modules);
unwinder::CfiUnwinder cfi_unwinder(module_cache);

// Use the strategy directly.
std::vector<unwinder::Frame> frames = cfi_unwinder.Unwind(&memory, registers, max_depth);
```

#### Asynchronous Example (Force Frame Pointers only)

```cpp
#include "src/lib/unwinder/unwind.h"
#include "src/lib/unwinder/fp_unwinder.h"

struct FPUnwindSession {
  unwinder::ElfModuleCache module_cache;
  unwinder::FramePointerUnwinder fp_unwinder;
  std::unique_ptr<unwinder::AsyncMemory> stack;

  explicit FPUnwindSession(const std::vector<unwinder::Module>& modules,
                           unwinder::AsyncMemory::Delegate* delegate)
      : module_cache(modules),
        fp_unwinder(module_cache),
        stack(std::make_unique<unwinder::AsyncMemory>(delegate)) {}
};

auto session = std::make_unique<FPUnwindSession>(modules, delegate);

fp_unwinder->AsyncUnwind(session->stack.get(), registers, max_depth,
                         [session = std::move(session)](std::vector<unwinder::Frame> frames) {
  // Handle the frames. The capture of `session` keeps the unwinder,
  // module cache, and async memory alive.
});
```
