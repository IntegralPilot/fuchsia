# Evaluate expressions

Zxdb can evaluate and print values from simple C, C++, and Rust
expressions. The most common use case to evaluate an expression in zxdb is with
the `print` verb. Expressions can also be used for commands that take a memory
location as an argument such as `stack` or `mem-read`.

When you evaluate an expression you need a stack frame, which in turn requires
a process with a paused thread. If the process is currently running, you can use
the [pause](execution.md) verb to pause the thread.

You can use the print command to show the current value of a variable in the
current stack frame:

For example, to see the value of a variable `i`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
print i
34
```

You can also evaluate expressions in the context of another stack frame
without switching to that frame.

For example, to see the value of `argv[0]` in stack frame `2`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
frame 2 print argv[0]
"/bin/cowsay"
```

## Print with expressions {#print-expressions}

The `print` command accepts the following arguments:

  * *`--max-array=<number>`*: Specifies the maximum array size to print. By
    default this is 256. Specifying large values slows down expression
    evaluation.

  * *`--raw` or `-r`*: Bypass pretty-printers and show the raw type information.

  * *`--types` or `-t`*: Force type printing on. The type of every value printed
    is explicitly shown. Implies `-v`.

  * *`--verbose` or `-v`*: Don't omit type names. Show reference addresses and
    pointer types.

To write an expression beginning with a hyphen, use `--` to mark the end of
arguments. Hyphens after `--` are treated as part of the expression:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
print -- -i
```

## Number formatting options {#number-formatting-options}

The `print` command accepts the following options to force numeric values
 to be of specific types:

  * `-b`: Binary
  * `-c`: Character
  * `-d`: Signed decimal
  * `-u`: Unsigned decimal
  * `-x`: Unsigned hexadecimal

## Special variables

When you work with variables in zxdb you may have an identifier name that is
not parseable in the current language. This is often the case for
compiler-generated symbols. Make sure to enclose such strings in `$(<symbols>)`.
Parentheses inside the escaped contents can be literal as long as they are
balanced, otherwise, escape them by preceding with a backslash. Include any
literal backslash with two backslashes.

These are all valid examples:

  * `$(something with spaces)`
  * `{% verbatim %}$({{impl}}){% endverbatim %}`  {# The verbatim block is to avoid issues with the fuchsia.dev template engine #}
  * `$(some_closure(data))`
  * `$(line\)noise\\)`

Additionally, zxdb also supports:

* [CPU registers](#cpu-registers)
* [Vector registers](#vector-registers)

### CPU registers {#cpu-registers}

You can refer to CPU registers with the `$reg(register name)` syntax. For
example, to display the ARM register `v3`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
print $reg(v3)
0x573a420f128
```

CPU registers can also be used unescaped as long as no variable in the current
scope has the same name. Registers can also be used like any other variable in
more complex expressions:

Note: You can use the `-x` option to display the unsigned hexadecimal value. For
additional options, see [Number formatting options](#number-formatting-options).

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
print -x rax + rbx
0x2108aa0032a
```

### Vector registers {#vector-registers}

Vector registers can be treated as arrays based on the setting of
`vector-format`.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
print ymm1
{3.141593, 1.0, 0, 0}

[zxdb] print ymm[0] * 2
6.28319
```

#### List vector registers

You can list the vector registers with `regs`.

For example, to list all of the vector registers:

Note: You can also use `-v` instead of `--vector`.


```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
regs --vector
    (Use "print $registername" to show a single one, or
     "print $registername = newvalue" to set.)

Vector Registers
  mxcsr 0x1fa0 = 8096

   Name [3] [2]           [1]           [0]
   ymm0   0   0 -3.72066e-103 -3.72066e-103
   ymm1   0   0  3.79837e-312  2.63127e-312
   ymm2   0   0             0 -3.72066e-103
   ymm3   0   0  1.26218e-311  1.26218e-311
   ymm4   0   0  1.26218e-311  1.26218e-311
   ymm5   0   0  1.26218e-311  1.26218e-311
   ymm6   0   0  5.96337e-321  5.87938e-321
   ymm7   0   0  2.56125e-311   2.4891e-311
   ymm8   0   0             0             0
   ymm9   0   0             0             0
  ymm10   0   0             0             0
  ymm11   0   0             0             0
  ymm12   0   0             0             0
  ymm13   0   0             0             0
  ymm14   0   0             0             0
  ymm15   0   0             0             0
    (Use "get/set vector-format" to control vector register interpretation.
     Currently showing vectors of "double".)
```

## Use `display`

When stepping through a function, it can be useful to automatically print one
or more expressions each time the program stops. The `display` command adds a
given expression to this list:

Note: This example uses a variable named `status` and its value is returned
when the program stops.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
display status
Added to display for every stop: status

[zxdb] next
🛑 main(…) • main.cc:48

    [code dump]

status = 5;
```

## Use `locals`

The `locals` command shows all local variables in the current stack frame. It
accepts the same arguments as `print` (see [Print with expressions](#print-expressions)):

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
locals
argc = 1
argv = (const char* const*) 0x59999ec02dc0
```
