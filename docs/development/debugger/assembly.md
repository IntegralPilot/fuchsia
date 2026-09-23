# Work with assembly language

## `disassemble`

Note: This is the `disassemble` command. You can also use `di` to express
`disassemble`.

The `disassemble` command disassembles from the current location. If available,
the instructions and call destinations are annotated with source line information:

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
disassemble
miscsvc.cc:118
 ▶ 0x20bc1c7aa60a  mov     dword ptr [rbx + 0x10c], eax
miscsvc.cc:122
   0x20bc1c7aa610  movabs  rax, -0x5555555555555556
   0x20bc1c7aa61a  mov     qword ptr [rbx + 0xe8], rax
   0x20bc1c7aa621  mov     qword ptr [rbx + 0xe8], 0x0
   0x20bc1c7aa62c  mov     rdi, qword ptr [rbx + 0xb0]
   0x20bc1c7aa633  mov     rax, qword ptr [rbx + 0xe8]
   0x20bc1c7aa63a  mov     qword ptr [rbx + 0x20], rax
   0x20bc1c7aa63e  call    0x20d    ➔ std::__2::size<>()
```

### Functions

If you specify an address or symbol, the `disassemble` command disassembles on
the respective address or symbol. If you provide a function name, it
disassembles the entire function:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
disassemble main
miscsvc.cc:88
   0x20bc1c7aa000  push    rbp
   0x20bc1c7aa001  mov     rbp, rsp
   0x20bc1c7aa004  push    rbx
   0x20bc1c7aa005  and     rsp, -0x20
   0x20bc1c7aa009  sub     rsp, 0x140
   0x20bc1c7aa010  mov     rbx, rsp
   0x20bc1c7aa013  mov     rax, qword ptr fs:[0x10]
   ...
```

### PC relative offsets

In some cases, you may want to disassemble based on the PC (program counter)
relative offset.

For example, to `disassemble` at the address `$rip - 0x7`:

Note: `$rip` is a register that stores a number (an address) of the next
instruction to execute.

```
[zxdb] di -- -0x7 # Disassemble at the address $rip - 0x7
     350     FX_LOGS(FATAL) << "Failed to construct the cobalt app: " << app.status();
     351   }
     352   inspector.Health().Ok();
     353   loop.Run();
   0x591e76352b  xor   edx, edx
   0x591e76352d  call  0x260fae   ➔ async::Loop::Run(async::Loop*, zx::time, bool)
     354   FX_LOGS(INFO) << "Cobalt will now shut down.";
 ▶ 0x591e763532  mov   edi, 0x30
   0x591e763537  call  0x81ab4    ➔ fuchsia_logging::ShouldCreateLogMessage(fuchsia_logging::LogSeverity)
   0x591e76353c  mov   byte ptr [rbp - 0x1b1], 0x0
   0x591e763543  test  al, 0x1
   0x591e763545  jne   0x2
   0x591e763547  jmp   0x74
```

### Arguments

The `disassemble` command accepts the following arguments:

  * `--num=<lines>` or `-n <lines>`: The number of lines or instructions to emit.
    If the location is a function name, it defaults to the instructions in the
    given function. If the location is an address or symbol, it defaults to 16.

  * `--raw` or `-r`: Output raw bytes in addition to the decoded instructions.

## Stepping in machine instructions

To step through machine instructions you can use following zxdb commands:

  * `nexti` / `ni`: Advances to the next instruction, but steps over call
    instructions for the target architecture..

  * `stepi` / `si`: Advances exactly one machine instruction..

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
nexti
🛑 main(int, const char**) • main.cc:102
main.cc:99
 ▶ 0x23f711346233  mov   edx, 0x20
   0x23f711346238  call  0x35a3a3  ➔ __asan_memcpy
   0x23f71134623d  mov   rdi, qword ptr [rbx + 0x258]
   0x23f711346244  call  0x1677    ➔ $anon::DecodeCommandLine

[zxdb] nexti
🛑 main(int, const char**) • main.cc:102
main.cc:99
 ▶ 0x23f711346238  call  0x35a3a3 ➔ __asan_memcpy
   0x23f71134623d  mov   rdi, qword ptr [rbx + 0x258]
   0x23f711346244  call  0x1677   ➔ $anon::DecodeCommandLine
   0x23f711346249  mov   rdi, qword ptr [rbx + 0x260]
```

Zxdb maintains information about whether the last command was an assembly
command or source-code and shows that information on stepping or breakpoint
hits.

To switch to assembly-language mode, use `disassemble`.

To switch to source-code mode, use `list`.

## `regs`

The `regs` command shows the most common CPU registers.

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
regs
General Purpose Registers
      rax  0xfffffffffffffffa = -6
      rbx          0x50b7085b
      rcx                 0x0 = 0
      rdx      0x2023de8c87a0
      rsi  0x7fffffffffffffff
      rdi          0x50b7085b
      rbp      0x224bb1e0b950
      rsp      0x224bb1e0b928
      ...
```

The `regs` command accepts the following arguments:

  * `--all` or `-a`: Enable all register categories (does not imply `-e`).

  * `--debug` or `-d`: Prints the debug registers.

  * `--extended` or `-e`: Enables more verbose flag decoding. This enables more
    information that is not normally useful for everyday debugging. This
    includes information such as the system level flags within the `rflags`
    register for x64.

  * `--float` or `-f`: Prints the dedicated floating-point registers. In most
    cases you should use `--vector` instead because all 64-bit ARM code and most
    x64 code uses vector registers for floating point.

  * `--vector` or `-v`: Prints the vector registers.

### Registers in expressions

Registers can be used in [expressions](expressions.md) like variables. The
canonical name of a register is `$reg(register name)`.

For more information and examples, see [CPU registers][cpu-registers-doc].

### Vector registers

The `regs --vector` command displays vector registers in a table according
to the current `vector-format` setting.

Use `get vector-format` to see the current values.

Use `set vector-format <new-value>` to set a new vector format.

The following values are supports:

  * `i8` (signed) or `u8` (unsigned): Array of 8-bit integers.
  * `i16` (signed) or `u16` (unsigned): Array of 16-bit integers.
  * `i32` (signed) or `u32` (unsigned): Array of 32-bit integers.
  * `i64` (signed) or `u64` (unsigned): Array of 64-bit integers.
  * `i128` (signed) or `u128` (unsigned): Array of 128-bit integers.
  * `float`: Array of single-precision floating point.
  * `double`: Array of double-precision floating point. This is the default.

This example sets the `vector-format` to `double`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
set vector-format double
```

This example returns the vector registers:

Note: The vector register table in `regs` are displayed with the low values
on the right side.

```
[zxdb] regs -v
Vector Registers
  mxcsr 0x1fa0 = 8096

   Name [3] [2] [1]       [0]
   ymm0   0   0   0         0
   ymm1   0   0   0   3.14159
   ymm2   0   0   0         0
   ymm3   0   0   0         0
   ...
```

Vector registers can also be used like arrays in expressions. The
`vector-format` setting controls how each register is converted into an array
value.

For example, to show the low 32 bits interpreted as a floating-point value of
the x86 vector register `ymm1`:


1. Set the `vector-format` to float:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  set vector-format float
  ```

1. Print the vector register `ymm1`:

  ```
  [zxdb] print ymm1[0]
  3.14159
  ```

When converting to an array, the low bits are assigned to be index 0, increasing
from there.

[cpu-registers-doc]: /docs/development/debugger/expressions.md#cpu-registers
