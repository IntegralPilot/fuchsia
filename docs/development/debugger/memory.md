# Inspect memory in zxdb

Zxdb can help you inspect memory for debugging purposes with the following
commands:

  * [`aspace`](#aspace)

    Shows mapped memory regions.

  * [`mem-analyze`](#mem-analyze)

    Dumps memory to help you to interpret pointers.

  * [`mem-read` / `x`](#mem-read)

    Dumps process memory.

  * [`stack-data`](#stack-data)

    Provides a low-level analysis of the stack.

  * [`sym-near`](#sym-near)

    Maps addresses to symbols.

## `aspace` {#aspace}

Note: This is the `aspace` command. You can also use `as` to express `aspace`.

The `aspace` command outputs address space information for the process. In
Fuchsia, virtual memory consists of a hierarchy of [Virtual Memory
Objects][vmo-object] (VMOs).

For example, the `aspace` command shows all VMOs in the process:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
aspace
           Start              End  Prot   Size      Koid       Offset  Cmt.Pgs  Name
        0x200000   0x7ffffffff000  ---    127T                                  proc:14629522
        0x200000   0x7ffffffff000  ---    127T                                    root
     0xe4ff29000      0xe5012a000  ---      2M                                      useralloc
     0xe4ff2a000      0xe5012a000  rw-      2M  14629569     0x201000        2        initial-thread
    0x6d0d1ad000     0x6d4d1b2000  ---      1G  14629573          0x0        0      scudo:reserved
    0x6d4d1b2000     0x6d4d1f2000  rw-    256K  14629575       0x5000        2      scudo:primary
...
  0x42afdf75e000   0x42afdf761000  rw-     12K  14629536          0x0        1        data:uncompressed-bootfs
  0x42afdf761000   0x42afdf771000  rw-     64K  14629537          0x0       16        bss:uncompressed-bootfs

              Page size: 4096
     Total mapped bytes: 48329449472
  Total committed pages: 125 = 512000 bytes
                         (See "help aspace" for what committed pages mean.)
```

If you specify an address, the `aspace` command shows the VMO hierarchy that
contains the specified address. This can be useful to determine where an address
is in memory, as the names of the VMOs typically indicate what type of region
that memory address is.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
aspace 0x6d0d1ad000
         Start              End  Prot  Size      Koid  Offset  Cmt.Pgs  Name
      0x200000   0x7ffffffff000  ---   127T                             proc:14629522
      0x200000   0x7ffffffff000  ---   127T                               root
  0x6d0d1ad000     0x6d4d1b2000  ---     1G  14629573     0x0        0      scudo:reserved

              Page size: 4096
```

In the example above, the `aspace` command details the following about the
`0x6d0d1ad000` address:

  * Hierarchy of VMOs that contain the address.
  * The address and size of each VMO.
  * The name of each VMO, which can give clues about their purpose.
      * From the name in this example, you can tell the address is in a stack
        allocated by `pthreads`.

The `Cmt.Pgs` column shows the number of committed pages (not bytes) in that
memory region in the mapped VMO.

If a VMO is a child (as in the case of mapped blobs), the original data is
present in the parent VMO but the child VMO that is actually mapped indirectly
references this data. The only pages in the child that count as committed
are those that are duplicated due to copy-on-write. This is why BLOBs and other
files that are unmodified will have a 0 committed page count.

### Additional details about the output

The following are relevant VMO names that could be included in output from the
`aspace` command:

  * `initial-thread`: The stack of the startup thread.
  * `pthread_t:0x...`: The stack of a pthread-created thread. The address
    indicates the memory location of the `pthread_t structure for that thread.
  * `*uncompressed-bootfs`: A memory-mapped library coming from bootfs (core
    system libraries). The `libs` command can tell you the library name for that
    address.
  * `stack: msg of ...`: The startup stack. This very small stack is only used
    by the dynamic linker and loader code.
  * `scudo:*`: Pages allocated with the scudo memory manager. If the process is
    using scudo, these regions are the application heap.
  * `vdso/next`: The built-in library that implements the next system calls.
  * `vdso/stable`: The built-in library that implements the stable system calls.
  * `blob-*`: Mapped library coming from blobfs. The `libs` command returns
    the library name for that address.

To see more information about a VMO, use the command `handle -k <koid>`

## `mem-analyze` {#mem-analyze}

Note: This is the `mem-analyze` command. You can also use `ma` to express
`mem-analyze`.

This command attempts to interpret memory as pointers and decode what they
point to. Addresses with corresponding symbols are symbolized, while other
addresses indicate the name of the memory-mapping region they fall into.

Note: For more information about dumping unknown memory, see the
[`aspace`](#aspace) command.

This example analyzes `0x42ff9c2fdd30`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
mem-analyze 0x42ff9c2fdd30
       Address               Data
0x42ff9c2fdd30 0x00000000000015f0
0x42ff9c2fdd38 0x0000000000000008
0x42ff9c2fdd40 0x000042f401a8a730 ▷ ldso
0x42ff9c2fdd48 0x000042f401a8a9f8 ▷ $(dls3.app)
0x42ff9c2fdd50 0x0000000000000053
0x42ff9c2fdd58 0x0000000010469c6b
0x42ff9c2fdd60 0x000042f401a8a9f8 ▷ $(dls3.app)
0x42ff9c2fdd68 0x0000000000000000
0x42ff9c2fdd70 0x000042ff9c2fde70 ▷ inside map "stack: msg of 0x1000"
0x42ff9c2fdd78 0x000042f4015e5548 ▷ dls3 + 0x42b
0x42ff9c2fdd80 0x10469c6b10769c7b
0x42ff9c2fdd88 0x10569c3310469c23
0x42ff9c2fdd90 0x10469c2710469c37
```

The `stack-data` command is a variant of `mem-analyze` and helps you analyze a stack.
For more information, see [`stack-data`](#stack-data).

## `mem-read` {#mem-read}

Note: This is the `mem-read` command. You can also use `x` to express `mem-read`.

The `mem-read` command provides hex dumps of the given address. Optionally, you
can override the default byte size to read with the `-size` (`-s`) option.

This example show the hex dumps for address `0x42ff9c2fdd30` while only reading
up to `100` bytes:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
mem-read -s 100 0x42ff9c2fdd30
0x42ff9c2fdd30:  f0 15 00 00 00 00 00 00-08 00 00 00 00 00 00 00  |
0x42ff9c2fdd40:  30 a7 a8 01 f4 42 00 00-f8 a9 a8 01 f4 42 00 00  |0    B       B
0x42ff9c2fdd50:  53 00 00 00 00 00 00 00-6b 9c 46 10 00 00 00 00  |S       k F
0x42ff9c2fdd60:  f8 a9 a8 01 f4 42 00 00-00 00 00 00 00 00 00 00  |     B
0x42ff9c2fdd70:  70 de 2f 9c ff 42 00 00-48 55 5e 01 f4 42 00 00  |p /  B  HU^  B
0x42ff9c2fdd80:  7b 9c 76 10 6b 9c 46 10-23 9c 46 10 33 9c 56 10  |{ v k F # F 3 V
0x42ff9c2fdd90:  37 9c 46 10
```

The `mem-read` command also supports an expression that evaluates to an address.
For example, if the type of the pointer has a known size, the dump automatically
shows that many bytes:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
mem-read &self->main_waker
0x1605a5d1ed0:  70 1a c8 36 47 04 00 00-68 fe 3d dd 25 01 00 00  |p  6G   h = %
```

## `stack-data`

The `stack-data` command provides a low-level analysis of the stack. This works
similarly to `mem-analyze`. `stack-data` defaults to the top of the current
thread's stack. The `stack-data` command attempts to decode addresses present in
the memory region, but it also adds annotations for the known register values
and stack base pointers of the thread.

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
stack-data
      Address               Data
0x1605a5d1428 0x000042a352fca11f ◁ rsp. ▷ _zx_port_wait + 0x1f
0x1605a5d1430 0x000001605a5d1460 ◁ frame 1 rsp. ▷ inside map "initial-thread"
0x1605a5d1438 0x000001605a5d1540 ▷ inside map "initial-thread"
0x1605a5d1440 0x7fffffffffffffff
0x1605a5d1448 0x0000044ab6c81800 ▷ inside map "scudo:primary"
0x1605a5d1450 0x000001605a5d14d0 ◁ rbp, frame 1 base. ▷ inside map "initial-thread"
0x1605a5d1458 0x00000125dd3566f5 ▷ zx_status::Status::ok
0x1605a5d1460 0x0000000000000000 ◁ frame 2 rsp
0x1605a5d1468 0x0000000000000000
0x1605a5d1470 0x0000000000000000
0x1605a5d1478 0x0000000000000000
0x1605a5d1480 0x0000000000000000 ◁ rdx, r14
```

In the notes column:

 * Left-pointing arrows indicate which registers point to that stack location.
 * Right-pointing arrows indicate where the value of the stack entry points to
   if it is interpreted as an address.

## `sym-near` {#sym-near}

Note: This is the `sym-near` command. You can also use `sn` to express `sym-near`.

The `sym-near` command attempts to map an address to a symbol name. Running the
command outputs the name and line information (if available) for the symbol at
or preceding the address and is most often used to tell what a pointer points to.

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
sym-near 0x125dd3a845e
0x125dd3a845e, power_manager::main() • main.rs:37
```

[vmo-object]: /docs/reference/kernel_objects/vm_object.md
