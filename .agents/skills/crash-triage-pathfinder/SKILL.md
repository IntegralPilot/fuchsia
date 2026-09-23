---
name: crash-triage-pathfinder
description: >
  Guides the triage and analysis of Zircon kernel panics and field crash
  reports. Helps systematically prune alternative theories by mathematically
  comparing software bugs (like Use-After-Free/vtable hijacking) against
  hardware-induced memory corruption (like single-bit flips).
---

# Crash Triage Pathfinder: Hardware vs. Software Classification Playbook

When investigating unexplained kernel panics or memory corruption crashes,
investigators often default to complex software bug theories (e.g., intricate
Use-After-Free race conditions or heap overrides). This guide provides a rapid
pruning procedure to mathematically distinguish software bugs from hardware
memory faults early in the triage phase.

## 1. The 3-Step Pruning Procedure

Before deep-diving into complex software bug reproduction or component state
machine analysis, execute these three diagnostic checks:

### Step 1: Hamming Distance Analysis on Corrupted Registers

If any pointer or register contains a corrupted or unexpected value, check if it
is a single-bit flip rather than arbitrary garbage:

1.  Identify the **expected/original** register value (e.g., thread pointer,
    return address).
2.  Identify the **actual/corrupted** register value from the crash log.
3.  Calculate the bitwise XOR difference: $$\text{Diff} = \text{Expected} \oplus
    \text{Actual}$$
4.  Calculate the Hamming distance (number of set bits in $\text{Diff}$):
   * **Hamming Distance = 1:** Highly indicative of a **hardware Single Event
     Upset (SEU)** in memory or CPU registers. Focus on tracing where the value
     was stored (e.g., stack, heap, or shadow call stack) when the thread was
     idle/suspended.
   * **Hamming Distance > 1:** If the distance is large, check if the
     "corrupted" value is actually a valid pointer to another active kernel
     structure or stack frame.
   * *Example:* If a register expected to hold a thread pointer instead holds a
     stack address, it is likely a **stack alignment mismatch** (e.g., popping
     from the wrong frame offset due to an incorrect function return target),
     not a bit flip.

### Step 2: Physical Range Constraints (Relative Offsets)

If the theory assumes a redirected execution path (e.g., vtable hijacking or ROP
gadget execution) on AArch64 or x86:

1.  Verify if the virtual call uses **relative offsets** (e.g., 32-bit signed
    integers in Relative VTables ABI) rather than absolute 64-bit pointers.
2.  Calculate the distance between the source memory segment (e.g.,
    heap-allocated object) and the destination segment (e.g., kernel text
    segment).
3.  If the distance is larger than the offset bit-width capacity (e.g.,
    heap-to-text on 64-bit kernels is often >200GB, while a 32-bit signed offset
    is limited to $\pm 2\text{GB}$), **prune the hijack theory immediately**. It
    is physically impossible for the branch to reach the target.

### Step 3: ELF Static Binary Search

If a theory assumes a branch redirected execution to a specific instruction
(e.g., landing exactly on a panic handler or `__stack_chk_fail` instruction):

1.  Run a static search over the entire compiled ELF binary (specifically the
    `.rodata` or instruction streams) to find all compiler-generated offsets.
2.  Check if any offset naturally targets the crashed address.
3.  If **zero exact matches** are found, and **zero single-bit-flipped offsets**
    target the crashed address, the software hijacking theory is mathematically
    disproved.

## 2. Register Verification and Semantic Mapping

When analyzing register dumps in crash reports (especially AArch64/ARM64),
verify the registers against standard ABI calling conventions and compiler
behavior to map out execution state:

1.  **Parameter and Argument Mapping (AArch64 ABI):**
   * **`x0` (or `x0`-`x7`):** Maps to `this` pointer for member function calls
     and/or initial arguments.
   * **Struct Parameters:** Small structs may be passed directly in consecutive
     registers (e.g., `x1`/`x2` for a 16-byte struct).
2.  **Large Struct Return Pointer (`x8`):**
   * If a function returns a large struct (usually >16 bytes) by value, the
     caller allocates storage on its stack and passes a pointer to this storage
     in **`x8`**. Compare `x8` with the Stack Pointer (`sp`/`usp`) to ensure it
     maps to `sp + offset`.
3.  **Control Flow Registers:**
   * **`lr` (Link Register):** Holds the return address of the caller. It should
     point exactly to the instruction following the caller's branch-with-link
     (`bl`).
   * **`elr` (Exception Link Register):** Points to the exact instruction that
     triggered the exception.
   * **`far` (Fault Address Register):** For memory/translation faults, contains
     the accessed address that caused the fault.
4.  **Reserved and Platform Registers:**
   * **`x18` (Shadow Call Stack):** In secure runtime configurations (like
     Zircon/Fuchsia), `x18` is reserved as the Shadow Call Stack (SCS) pointer.
   * **`tpidr_el1` / Thread Pointer:** Often stores the pointer to the current
     thread control structure (`thread_t*`). Check if another callee-saved
     register holds the same value (common when accessing thread local storage
     or performing per-cpu operations).
5.  **Loop and Status Codes:**
   * Check registers for common status returns (e.g., `ZX_ERR_OUT_OF_RANGE` /
     `-14` represented as `0xfffffff2` in 32-bit registers) to verify loop
     state.

## 3. Tracing Shadow Call Stack (SCS) Faults

In modern secure operating systems (like Fuchsia/Zircon), return addresses are
protected by a compiler-inserted Shadow Call Stack (`x18` register on AArch64).

If a single-bit flip occurs in the DRAM backing the shadow call stack:

1.  The thread blocks / sleeps (e.g., in a semaphore wait state).
2.  A bit-flip occurs in the stack memory (`[x18]`).
3.  The thread wakes up and pops the corrupted return address, jumping to an
    arbitrary address in the middle of another function's epilogue.
4.  The hijacked epilogue executes its own stack operations (e.g., stack canary
    validation) using the *original* thread's stack pointer (`x29`/`sp`).
5.  This inevitably fails the canary check or reads incorrect registers, causing
    a stack validation crash (`__stack_chk_fail`) and creating a pristine
    snapshot of the original caller's registers.

*Triage Tip:* In SCS corruption panics, the `lr` in the crash dump will point to
the instruction immediately following the hijacked epilogue's panic branch, and
the frame pointer chain will completely bypass the function that actually threw
the exception, appearing as if the crash occurred in the parent caller.

## 4. Rigorous Register Verification Methodology & Tooling

To ensure zero assumptions are made when verifying register contents during
crash triage, use the following systematic methodology with host-side tools on
the debug kernel image:

### A. ELF Mapping & Base Virtual Address Verification

* **Action:** Confirm the base memory layout and section virtual addresses (VMA)
  of the binary.
* **Command:** `llvm-objdump -h <debug_binary>`
* **Application:** Identify where the `.text` section begins in the symbol space
  to ensure offsets in the crash dump backtrace align with file offsets.

### B. Global Variable & Function Symbol Identification
* **Action:** Find the exact virtual address and sizes of classes, functions, or
  global tables.
* **Command:** `llvm-objdump -t <debug_binary> | grep <symbol_name>`
* **Application:** Verify if callee-saved registers hold pointers to global
  structures (e.g., finding the base of `arm64_percpu_array` to verify CPU ID
  mapping in `x20`).

### C. Structure Offset & Member Verification

* **Action:** Resolve exact offsets of members in structures or classes to
  verify pointer register offsets.
* **Commands:**
  * **Option 1 (DWARF Dumps):** `llvm-dwarfdump --name=<member_name>
    <debug_binary>` (Locate the `DW_TAG_member` node to read
    `DW_AT_data_member_location`).
  * **Option 2 (GDB Structure Layout):** `gdb -batch -ex "ptype /o
    <class_or_struct_name>" <debug_binary>` (Prints complete offset layout tree
    of a struct, including sizes, paddings, and alignment holes).
* **Application:** Verify exact member offsets (e.g., confirming `canary_` is at
  `0x14` or that list head pointers reside at specific offsets like `0xb8`
  inside `VmMapping`). E.g., verifying that the thread pointer `tp` maps to
  `Thread*` via `tp - offsetof(Thread, arch_.thread_pointer_location)`.

### D. Disassembly & Argument Tracing

* **Action:** Trace exact register assignments before function calls.
* **Command:** `llvm-objdump -d --start-address=<addr> --stop-address=<addr>
  <debug_binary>`
* **Application:** Inspect caller instructions (e.g., `ldp`, `stp`, `mov`,
  `lsl`) immediately preceding a branch/link (`bl`) instruction. Under the
  AArch64 ABI:
    * `x0`-`x7` represent the first 8 function parameters.
    * Small structures passed by value (like `VmCowRange`) occupy consecutive
      registers.
    * `x8` holds a pointer to the return buffer for returned structs larger than
      16 bytes.
    * Callee-saved registers (e.g., `x19`-`x28`) are preserved, so their
      contents inside the callee match what was set in the caller prior to the
      call.

### E. Exception Syndrome & Flags Decoding

* **Action:** Convert raw hexadecimal values to semantic meaning.
* **ESR/FAR Abort Types:** Decode the ESR register's Exception Class (EC, bits
  31:26) and IFSC (bits 5:0). An Instruction Abort (EC = `0x21`) translation
  fault indicates the MMU failed to fetch instructions from a code page,
  confirming a kernel text mapping failure.
* **Kernel Flags/Error Codes:** Use regex search (`grep_search`) to locate
  source definitions of constants (e.g., searching `#define ZX_ERR_` or `const
  uint VMM_PF_FLAG_`) to match exact register values (e.g., decoding `pf_flags =
  0x33` to write, user, not-present, and hardware-requested fault flags).

