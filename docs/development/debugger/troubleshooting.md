# Troubleshooting zxdb

This page lists common troubleshooting tips for `zxdb`:

* [Ensure that zxdb and `debug_agent` are built](#zxdb-built)
* [Ensure that ffx can communicate with the device](#ffx-device-communication)
* [Ensure that the Fuchsia package server is running](#package-server-running)
* [Diagnose problems with symbols](#diagnose-problems-symbols)
* [Verify that package serving is working correctly for debug_agent](#verify-that-package-serving-is-working-correctly-for-debug-agent)
* [Check if the debugger component exists](#check-if-the-debugger-component-exists)

## Ensure that ffx can communicate with the device {:#ffx-device-communication}

Make sure that ffx can discover the device, either a emulator or a hardware device, and
RCS is started on the device.

```
$ ffx target list
NAME       SERIAL       TYPE             STATE      ADDRS/IP                    RCS
demo-emu   <unknown>    core.x64         Product    [10.0.2.15,                 Y
                                                    fec0::90e:486e:b6b5:9780,
                                                    fec0::487b:fabd:20fa:43ee,
                                                    127.0.0.1]
```

## Ensure that the Fuchsia package server is running {:#package-server-running}

For most in-tree build configurations, the `debug_agent` which is used by zxdb
is in the universe dependency set, but not in the base dependency set so won't
be on the Fuchsia target device before boot. This may lead you to see errors
similar to the following:

```none {:.devsite-disable-click-to-copy}
BUG: An internal command error occurred.
Error: Attempted to find protocol marker fuchsia.debugger.Launcher at '/toolbox' or '/core/debugger', but it wasn't available at either of those monikers.

Make sure the target is connected and otherwise functioning, and that it is configured to provide capabilities over the network to host tools.
    1.  This service dependency exists but connecting to it failed with error CapabilityConnectFailed. Moniker: /core/debugger. Capability name: fuchsia.debugger.Launcher
More information may be available in ffx host logs (ffx.log) in directory:
    <directory>
To inspect, run: tail -f "<directory>/ffx.log"
```

If you see this type of error, make sure that `fx serve` is running in a
separate terminal. For example:

```posix-terminal
fx serve
```

## Diagnose problems with symbols {#diagnose-problems-symbols}

### Debug symbols are registered {#set-symbol-location}

By default, zxdb obtains the locations of the debug symbols from the
[symbol index](/docs/development/tools/ffx/workflows/register-debug-symbols.md).
The registrations of debug symbols from in-tree and most out-of-tree
environments are automated.

In case symbol registration fails, zxdb has these command-line options to provide
additional symbol lookup locations:

* [`--build-id-dir`](#build-id-dir)
* [`--ids-txt`](#ids-txt)
* [`--symbol-path`](#symbol-path)

These options have  settings that can be manipulated using `set` or `get`.

For example, to add a `.build-id` directory, you can do either of the following:

Note: For in-tree development, `ffx debug connect` automatically sets up all
necessary options.

* {set `build-id-dirs`}

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  set build-id-dirs += some/other_location/.build-id
  ```

* {`--build-id-dir` flag}

  ```posix-terminal
  ffx debug connect -- --build-id-dir some/other_location/.build-id
  ```

### `build-id-dir` {:#build-id-dir}

Some builds produce a `.build-id` directory. Symbol files in this directory are
indexed according to their build IDs. For example, the Fuchsia build makes a
`.build-id` directory inside it's build directory, e.g., `out/x64/.build-id`.

These directories can be added to zxdb through the `build-id-dirs` setting or
the `--build-id-dir`.

### `ids-txt` {#ids-txt}

Instead of a `.build-id` directory, some builds produce a file called `ids.txt`
that lists build IDs and local paths to the corresponding binaries. These files
can be added to zxdb through the `ids-txts` setting or the `--ids-txt`
command-line flag.

### `symbol-path` {#symbol-path}

The `--symbol-path` flag can be used to add arbitrary files or directories to
the symbol index. If the path is pointing to a file, zxdb treats it as an ELF
file and adds it to the symbol index. If it's a directory, all binaries under
the given path are indexed.

### Check symbol status {#check-symbol-status}

The `sym-stat` command returns the status for symbols. If there is no running
process, it returns information on the different symbol locations that you have
specified. If your symbols aren't found, make sure this matches your
expectations.

To see the status of symbols, run `sym-stat`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
sym-stat
Symbol index status

  Indexed  Source path
 (folder)  /home/alice/.build-id
 (folder)  /home/alice/build/out/x64
        0  my_dir/my_file
```

If you see a `0` in the `Indexed` column of `Symbol index status` this indicates
that zxdb could not find the source of the symbols. If you cannot debug the
issue, file a [zxdb bug][zxdb-bug-link].

### Variable values are unavailable

Zxdb can return an issue around variable values, which in most cases is related
to the optimization level of the program. For example:

* _Optimized out_: This indicates that the program symbols declare a variable
with the given name, but that it has no value or location. This indicates that
the compiler has entirely optimized out the variable and the debugger can't
display it. If you need to see the variable, use a less-optimized build setting.

* _Unavailable_: This indicates that the variable is invalid at the current
address, but that its value is known at other addresses. In optimized code, the
compiler often re-uses registers, which can overwrite previous values, which
then become unavailable.

For example, you can see the valid ranges for the `my_variable` variable with
the `sym-info` command:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
sym-info my_variable
Variable: my_variable
  Type: int
  DWARF tag: 0x05
  DWARF location (address range + DWARF expression bytes):
    [0x3e0d0a3e05b, 0x3e0d0a3e0b2): 0x70 0x88 0x78
    [0x3e0d0a3e0b2, 0x3e0d0a3eb11): 0x76 0x48 0x10 0xf8 0x07 0x1c 0x06

```

Note: DWARF is a standardized debugging data format.

`DWARF location` gives you a list of address ranges where the value of the
variable is known. The address range is inclusive at the beginning of the range
and non-inclusive at the end of the range.

`DWARF expression bytes` indicate the internal instructions for finding the
variable.

You can also use the `di` command to see the current address.

### Source code location is correctly set up

The Fuchsia build generates symbols relative to the build directory. The
relative paths look like `../../src/my_component/file.cc`. The build directory
is usually provided by the symbol index, so that source files can be located.

If your source files are not being found, you need to manually set the source
map setting.

For example, if the debugger can't find `./../../src/my_component/file.cc`, an
the file is located at `/path/to/fuchsia/src/my_component/file.cc`, you need
to set the `source_map`:

```none
[zxdb] set source-map += ./../..=/path/to/fuchsia
```

Once you have set `source-map`, zxdb looks for
`/path/to/fuchsia/src/my_component/file.cc`.

### Mismatched source lines {:#mismatches-source-lines}

Sometimes the source file listings may not match the code. The most common
reason is that the build is out-of-date and no longer matches the source. The
debugger checks that the symbol file modification time is newer than the source
file, but it only prints the warning the first time the file is displayed.

Some users have multiple checkouts. Use the `-f` option with the `list` command
to check the file name of the file that zxdb found. For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
list -f
/home/alice/fuchsia/out/x64/../../src/foo/bar.cc
 ... <source code> ...
```

If zxdb is finding a file in the incorrect checkout, override the `build-dirs`
option as described in [Debug symbols are registered][debug-symbols-location].

You can set the `show-file-paths` option to increase the information for file
paths. When this setting is set to `true`:

  * It shows the full resolved path in source listings as in `list -f`.
  * It shows the full path instead of just the file name.

To set `show-file-paths` to `true`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
set show-file-paths true
```

#### When setting breakpoints

You may notice a mismatch source line when setting a breakpoint on a specific
line where the displayed breakpoint location doesn't match the line number you
typed. In most cases, this is because this symbols did not identify any code on
the specified line so zxdb used the next line. This can happen even in
unoptimized builds, and is most common for variable declarations.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
b file.cc:138
Breakpoint 1 (Software) @ file.cc:138
   138   int my_value = 0;          <- Breakpoint was requested here.
 ◉ 139   DoSomething(&my_value);    <- But ended up here.
   140   if (my_value > 0) {
```

## Verify that package serving is working correctly for debug_agent {:#verify-that-package-serving-is-working-correctly-for-debug-agent}

If you suspect that serving the `debug_agent` package is failing, check to make
sure that package serving is working correctly in your Fuchsia checkout setup.

Use `pkgctl` directly on the device shell to resolve the package:

1. Run the `resolve` command for the `debug_agent` package, for example:

   ```posix-terminal
   fx shell pkgctl resolve fuchsia-pkg://fuchsia.com/debug_agent
   ```

2. To confirm that the package was successfully resolved, inspect the package
   resolver stats:

   ```posix-terminal
   ffx inspect show core/pkg-resolver:root/resolver_service/successful_resolves
   ```

   Look for entries where `source = TUF` and the hash of the `debug_agent` package
   matches the value from the `out/default/amber-files/repository/targets.json` file
   in your Fuchsia source checkout setup.

## Check if the debugger component exists {:#check-if-the-debugger-component-exists}

If the `ffx debug connect` command fails while setting up `zxdb`, check if the
`debugger` component actually exists on the target device using the
`ffx component list` command, for example:

```posix-terminal
ffx component list | grep debugger
```

This command needs to return `core/debugger` for `ffx debug connect` to work.

<!-- Reference links -->

[fuchsia-dependency-sets]: /docs/get-started/learn/build/product-packages.md#dependency_sets
[debug-symbols-location]: /docs/development/debugger/troubleshooting.md#set-symbol-location
[zxdb-bug-link]: https://issues.fuchsia.dev/issues/new?component=1389559&template=1849567
