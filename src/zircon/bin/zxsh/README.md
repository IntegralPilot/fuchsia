# zxsh - Fuchsia Shell

`zxsh` is a modern, lightweight, POSIX-like command shell implemented in Rust.
It is designed specifically for the Fuchsia operating system to serve as a fast,
robust, and minimal shell environment, intended to replace the NetBSD-derived
`dash` shell (`zircon/third_party/uapp/dash`).

## Key Features & Architecture

### Flat, Relocatable AST

Unlike traditional ASTs which rely on recursive heap-allocated structures (such
as `Box<Node>`), `zxsh` compiles commands into a **flat, relocatable Abstract
Syntax Tree** (`src/parser/ast.rs`, `src/relative.rs`).

*   **Relocatable Offset Pointers**: The AST is built inside a contiguous byte
    buffer using 32-bit unsigned offset pointer types (`relative::Ptr<T>`,
    `relative::Slice<T>`, and `relative::BStr`). Pointer dereferencing is
    bounds-checked against the containing buffer (`relative::Buffer`), ensuring
    memory safety without pointer patching during reallocation.
*   **Zerocopy AST Nodes**: All AST structures (`Command`, `WordPart`,
    `Redirect`, `CaseItem`) implement `zerocopy` traits (`FromBytes`,
    `IntoBytes`, `Immutable`, `KnownLayout`), allowing direct in-place access
    and serialization.
*   **Zero-Copy Subshells & VMO Transport**: When spawning a subshell, the
    parent serializes the AST buffer and the shell environment directly into a
    Zircon VMO with a fixed 24-byte header (`SubshellPayloadHeader` in
    `src/subshell.rs`) and transfers it via a startup handle
    (`fuchsia_runtime::HandleType::User0`). The spawned `zxsh` subshell process
    takes the handle on startup and executes the AST in place without reparsing
    or tree allocations.

### Binary Size & Memory Optimizations

Because `zxsh` runs in early-boot and recovery environments, keeping the binary
size small is a primary design goal:

*   **Byte-Safe Strings**: Standard Rust `String` and `str` are largely
    replaced by `BString` and `BStr` types from the `bstr` crate
    (`src/string.rs`). These operate directly on arbitrary `[u8]` byte slices
    (complying with Bourne shell path/variable byte semantics) and are compiled
    without the `"unicode"` feature to eliminate large Unicode validation
    tables.
*   **Minimal Error Formatting**: Standard library OS error loading tables are
    bypassed in favor of static lookup mappings (`src/errors.rs`) for Zircon
    (`zx_status_str`) and I/O (`io_err_str`) error codes.
*   **Zero-Allocation Custom Sort**: Uses a custom in-place 3-way quicksort
    (`src/sort.rs`) to avoid compiling the standard library's larger generic
    sorting routines.
*   **Custom Flat Collections**: Implements vector-backed `FlatMap` and
    `FlatSet` structures (`src/collections.rs`) for variables, functions,
    aliases, and limits, avoiding the overhead and SipHash hashing dependencies
    of standard `HashMap` and `HashSet`.
*   **Compiler Optimization**: Built with `-Copt-level=z` (`optimize-for-size`
    in `BUILD.gn`) to minimize binary footprint on BootFS.

### Line Editing & Interactive REPL

In interactive mode (`src/repl/mod.rs`):

*   **Line Editor Integration**: Uses the pure-Rust `line-editor` library
    (`//src/lib/line-editor`) for history management and interactive line
    editing.
*   **Tab Completion**: Provides autocompletion (`src/repl/completion.rs`) for
    commands in `PATH` as well as filesystem paths.
*   **Dynamic Prompts**: Supports shell prompt expansion for `PS1` (default
    `$ `), `PS2` (default `> `), and `PS4` (xtrace prefix, default `+ `).

---

## Shell Language Capabilities

`zxsh` implements standard POSIX shell grammar and execution semantics:

*   **Commands & Pipelines**: Simple commands (`cmd arg1 arg2`), pipelines
    (`cmd1 | cmd2 | cmd3`), and command blocks (`{ cmd; }`).
*   **Control Flow**:
    *   Conditionals: `if ...; then ...; elif ...; then ...; else ...; fi`
    *   Loops: `while ...; do ...; done`, `until ...; do ...; done`,
        `for var in ...; do ...; done`
    *   Pattern Matching: `case word in pattern) ... ;; esac`
    *   Chaining: `&&` (logical AND), `||` (logical OR), `;` and newline
        (sequence), `&` (asynchronous background execution)
    *   Loop Control: `break [n]`, `continue [n]`
*   **Functions**: Shell function definitions (`fname() { ... }`) with local
    variable scoping (`local`) and positional argument isolation.
*   **Expansions**:
    *   **Parameter / Variable Expansion**: `$VAR`, `${VAR}`,
        `${VAR:-default}`, `${VAR:=default}`, `${VAR:?error}`, `${VAR:+alt}`,
        string length `${#VAR}`, suffix removal `${VAR%pattern}` /
        `${VAR%%pattern}`, and prefix removal `${VAR#pattern}` /
        `${VAR##pattern}`.
    *   **Special Parameters**: `$?` (exit status), `$#` (arg count), `$$`
        (PID), `$!` (last background PID), `$*` / `$@` (all args), `$-` (current
        options), `$0` (script/shell name), and positional `$1`..`$9`, `${10}`.
    *   **Command Substitution**: `$(command)` and legacy backticks
        `` `command` ``.
    *   **Arithmetic Expansion**: `$(( expression ))` supporting arithmetic
        (`+`, `-`, `*`, `/`, `%`), bitwise (`&`, `|`, `^`, `~`, `<<`, `>>`),
        comparisons (`<`, `<=`, `>`, `>=`, `==`, `!=`), logical (`&&`, `||`,
        `!`), ternary (`? :`), assignment (`=`, `+=`, `-=`, etc.), variables,
        and grouping.
    *   **Pathname Expansion (Globbing)**: `*`, `?`, and character classes
        `[...]` (including ranges `[a-z]` and negations `[!...]` / `[^...]`).
    *   **Word Splitting & Quote Removal**: Field splitting using `$IFS`, single
        quotes `'...'`, double quotes `"..."`, and backslash escapes `\`.
*   **Redirections & Here-Documents**:
    *   File input `< file` and output `> file` (truncation) / `>> file`
        (append)
    *   Clobber control `>| file` (overriding `noclobber` / `set -C`)
    *   File descriptor duplication `>&fd`, `<&fd` and closing `>&-`, `<&-`
    *   Here-documents `<< EOF` and `<<- EOF` (with or without parameter
        expansion; small heredocs are written inline to the pipe, while large
        heredocs use a streaming worker thread to prevent pipe deadlocks)
    *   `/dev/null` redirection emulation via `fdio::create_fd_null`
*   **Job Control & Signals**:
    *   Background process management (`&`, `jobs`, `fg`, `bg`, `wait`)
    *   Signal trap handling via `trap` (supports `EXIT`, `INT`, `TERM`, `HUP`,
        `QUIT`)
    *   Interactive interrupt handling (Ctrl+C cancels line buffer and sets exit
        status to 130)

---

## Built-in Commands

`zxsh` supports a rich set of builtins implemented across dedicated modules:

### 1. Essential POSIX Built-ins

Implemented in `src/builtins/essential.rs` and `src/builtins/mod.rs`:

*   `cd`, `chdir`, `pwd` (supports `-L` logical and `-P` physical modes)
*   `export`, `readonly`, `local`, `unset`
*   `alias`, `unalias`
*   `set`, `shift`, `getopts`
*   `eval`, `exec`, `exit`, `return`
*   `break`, `continue`
*   `jobs`, `fg`, `bg`, `wait`
*   `trap` (signal trap registration and inspection)
*   `read` (reads line from standard input with `-r` raw mode and `$IFS` field
    splitting)
*   `type` (identifies command as builtin, function, alias, or external binary)
*   `command` (executes command bypassing shell functions and aliases)
*   `hash` (displays or manages cached executable path lookups)
*   `ulimit` (manages resource limits such as file size, stack, open files, CPU
    time)
*   `umask` (displays or updates file mode creation mask in octal or symbolic
    format)
*   `.` (source), `:`, `true`, `false`

### 2. File Utilities

Implemented in `src/builtins/file_utils.rs` to allow filesystem operations
without the overhead of spawning external processes:

*   `ls` (directory and file listing; supports `-l` long format)
*   `cp` (copy files and directories; supports `-r`/`-R`, `-f`, `-p`)
*   `mv` (move/rename files; supports `-f`)
*   `rm` (remove files and directories; supports `-r`/`-R`, `-f`)
*   `mkdir` (create directories; supports `-p`)

### 3. Text & Formatted Output

*   `echo` (`src/builtins/echo.rs`): Prints arguments with escape sequence
    processing (`\n`, `\t`, `\c`, octal escapes) and `-n` flag support.
*   `printf` (`src/builtins/printf.rs`): POSIX standard formatted output
    supporting format specifiers (`%s`, `%c`, `%d`, `%i`, `%o`, `%u`, `%x`,
    `%X`, `%b`, `%q`, `%%`), width and precision modifiers (including `*`), and
    formatting flags (`-`, `+`, space, `#`, `0`).

### 4. Condition Evaluation

*   `test` / `[` (`src/builtins/test.rs`): Full POSIX expression evaluation:
    *   File tests: `-e`, `-r`, `-w`, `-x`, `-f`, `-d`, `-s`, `-c`, `-b`, `-p`,
        `-h`, `-L`, `-S`, `-u`, `-g`, `-k`, `-t`, `-O`, `-G`, `-nt`, `-ot`,
        `-ef`
    *   String tests: `-z`, `-n`, `=`, `==`, `!=`, `<`, `>`
    *   Integer comparisons: `-eq`, `-ne`, `-lt`, `-le`, `-gt`, `-ge`
    *   Logical operators: `!`, `-a`, `-o`, `( ... )`

### 5. Diagnostics & Timing Utilities

*   `list` (`src/builtins/list.rs`): Displays numbered lines of a file
    (`list <filename>`).
*   `msleep` (`src/builtins/msleep.rs`): Sleeps for a specified number of
    milliseconds (`msleep <msecs>`).
*   `dump` (`src/builtins/dump.rs`): Formats and prints a hexadecimal/ASCII dump
    of a file (`dump <filename>`).
*   `times` (`src/builtins/times.rs`): Prints accumulated user and system CPU
    times for the shell and child processes.

### 6. Fuchsia/Zircon System Control Built-ins

Implemented in `src/builtins/fuchsia.rs`:

*   `dm` / `power`: Connects to `fuchsia.hardware.power.statecontrol.Admin` to
    control device power states:
    *   `dm poweroff` / `dm shutdown` / `power off` / `power shutdown`
    *   `dm reboot` / `power reboot`
    *   `dm reboot-bootloader` (or `rb`) / `power reboot-bootloader` (or `rb`)
    *   `dm reboot-recovery` (or `rr`) / `power reboot-recovery` (or `rr`)
    *   `dm help` / `power help`
*   `k`: Connects to `fuchsia.kernel.DebugBroker` to execute kernel debug
    commands directly from the shell (e.g. `k thread`). Forwards `poweroff`,
    `reboot`, and `reboot-bootloader` commands to `dm`.

---

## Running zxsh

`zxsh` supports multiple execution modes (`src/main.rs`, `src/args.rs`):

1.  **Interactive REPL**:
    ```bash
    zxsh
    # or force interactive mode:
    zxsh -i
    ```
2.  **Command Execution**:
    ```bash
    zxsh -c "echo 'Hello from zxsh!'"
    # with positional arguments:
    zxsh -c 'echo "$1 $2"' script_name "foo" "bar"
    ```
3.  **Script Execution**:
    ```bash
    zxsh /path/to/script.sh [args...]
    ```
4.  **Standard Input Execution**:
    ```bash
    zxsh -s [args...]
    # or forced stdin reading:
    zxsh - [args...]
    ```
5.  **Subshell Process (Internal)**:
    Spawned internally via startup handle (`HandleType::User0`) containing a
    serialized AST and shell environment in a VMO.

### Command-Line Options

Supported single-letter flags (set with `-`, clear with `+`) and `-o` / `+o`
options:

| Flag | Option Name | Description |
| :--- | :--- | :--- |
| `-a` | `allexport` | Automatically export all defined or modified variables |
| `-b` | `notify` | Report background job termination immediately |
| `-C` | `noclobber` | Prevent redirection from overwriting existing files |
| `-e` | `errexit` | Exit immediately if a command exits non-zero |
| `-f` | `noglob` | Disable pathname expansion (globbing) |
| `-i` | `interactive`| Force shell to run interactively |
| `-I` | `ignoreeof` | Ignore EOF (Ctrl-D) when reading from stdin |
| `-l` | `login` | Run as a login shell |
| `-m` | `monitor` | Enable job control |
| `-n` | `noexec` | Read commands but do not execute them |
| `-s` | `stdin` | Read commands from standard input |
| `-u` | `nounset` | Treat unset variables as an error during expansion |
| `-v` | `verbose` | Print shell input lines as they are read |
| `-x` | `xtrace` | Print commands and arguments as they are executed |
| `-E` | `emacs` | Enable Emacs line editing mode |
| `-V` | `vi` | Enable Vi line editing mode |

---

## Limitations

### File Descriptor Inheritance

Unlike POSIX shells on Linux/macOS, `zxsh` on Fuchsia does not support implicit
inheritance of non-standard file descriptors (FDs other than `0`, `1`, and `2`)
for external commands.

For example, if you run:
```bash
exec 3>file.txt
my_external_command >&3
```
Or:
```bash
my_external_command 3>file.txt
```
The child process `my_external_command` will NOT inherit FD 3. It will start
with FD 3 closed.

This is a design limitation of the Fuchsia process creation model (`fdio_spawn`
/ `fuchsia.process.Launcher`), which requires explicit transfer of handles and
does not support cloning the entire file descriptor table to child processes by
default. This behavior matches the Fuchsia port of `dash`
(`zircon/third_party/uapp/dash`).

Redirections of standard streams (`0`, `1`, `2`) are fully supported for all
commands.

---

## Testing

`zxsh` includes a comprehensive test suite covering parser grammar, AST
serialization, evaluations, expansions, arithmetic, builtins, and parity testing
against `dash`.

To run the test suite:
```bash
fx test zxsh_tests
```

Unit tests are organized in `src/tests/` and cover:
*   Language constructs (AST, tokenization, parsing, pipelines, control flow,
    redirections, expansions)
*   Evaluation runtime (signals, arithmetic, globbing, state frames, subshells)
*   Parity tests verifying behavioral compatibility with `dash` for all shell
    builtins
