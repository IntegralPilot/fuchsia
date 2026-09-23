# Understand how symbols are loaded {:#understand-load-symbols}

Zxdb should automatically set symbol settings by your environment.

This section explains how zxdb loads symbols. If you are looking for ways
to troubleshoot common issues with zxdb's symbolizer, see
[Diagnose problems with symbols][troubleshoot-symbols].

## build IDs

Zxdb locates the symbols for a binary on a target device by using the binary's
**build ID**.

Note: If you are using macOS, you have to install `readelf`.

For example, to see the build ID for a binary on Linux, dump the `notes` for
the ELF binary:

```none {:.devsite-disable-click-to-copy}
$ readelf -n my_binary

  ... (some other notes omitted) ...

Displaying notes found in: .note.gnu.build-id
  Owner                Data size 	Description
  GNU                  0x00000014	NT_GNU_BUILD_ID (unique build ID bitstring)
    Build ID: 18cec080fc47cdc07ec554f946f2e73d38541869
```

The `sym-stat` zxdb command shows the build IDs for each binary and the library
that is currently loaded in the attached process. It also displays the
corresponding symbol file if found.

## Symbol servers {#symbol-servers}

Zxdb can load symbols for prebuilt libraries from Google servers or upstream
`debuginfod` servers. This is the mechanism how symbols are delivered for SDK
users for anything that isn't built locally. For more information, see
[Downloading symbols][advanced-download-symbols].

When working with large binaries, symbols can be several gigabytes so the
download process may take some time (usually in the range of several minutes).
The `sym-stat` command displays `Downloading...` while downloading symbols.

When zxdb downloads symbols, it stores them in the symbol cache. The
`symbol-cache` setting contains the name of this directory.

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
get symbol-cache
symbol-cache = /home/me/.fuchsia/debug/symbol-cache
```

When zxdb downloads symbols from prebuilt binaries fetched from upstream
packages, the debuginfo may not have a corresponding ELF binary (depending on
the server and the package). When this happens, zxdb may display errors that
look like:

```none {:.devsite-disable-click-to-copy}
...
binary for build_id 26820458adaf5d95718fb502d170fe374ae3ee70 not found on 5 servers
binary for build_id 53eaa845e9ca621f159b0622daae7387cdea1e97 not found on 5 servers
binary for build_id f3fd699712aae08bbaae3191eedba514c766f9d2 not found on 5 servers
binary for build_id 4286bd11475e673b194ee969f5f9e9759695e644 not found on 5 servers
binary for build_id 2d28b51427b49abcd41dcf611f8f3aa6a2811734 not found on 5 servers
binary for build_id 0401bd8da6edab3e45399d62571357ab12545133 not found on 5 servers
...
```

This indicates that the ELF binary file was not found on the debuginfod servers.
When this happens, the ELF symbols are not available for this particular binary,
which is typically fine for most debugging scenarios. The most commonly used ELF
specific symbols that may be unavailable are PLT symbols.

You can use the `sym-stat` command to verify that DWARF information has been
loaded:

Note: DWARF information is downloaded separately from the ELF symbols.

```none {:.devsite-disable-click-to-copy}
  libc.so.6
    Base: 0x1c85fdc2000
    Build ID: 0401bd8da6edab3e45399d62571357ab12545133
    Symbols loaded: Yes
    Symbol file: /home/alice/.fuchsia/debug/symbol-cache/04/01bd8da6edab3e45399d62571357ab12545133.debug
    Source files indexed: 1745
    Symbols indexed: 10130
```

### .build-id directory symbol databases
Caution: If you are working from a Fuchsia checkout or with a Fuchsia SDK, you
don't need to consider the information from this section.

Many build environments, including the main `fuchsia.git` repository, add
symbolized binaries in a standard directory structure called `.build-id`. This
directory contains subdirectories that are named according to the first two
characters of the binary's build ID. These subdirectories contain the symbol
files named according to the remaining characters of the build ID.

You can set one or more build ID directories on the command line or
interactively by using the `build-id-dirs` setting which is a list of directory
paths.

For example, to add `/home/alice/project/out/x64/.build-id` as a `build-id-dirs`:

Note: These directories do not need to be named `.build-id`

* {zxdb instance}

  Note: These `build-id-dirs` are only added for the duration of the zxdb
  instance.

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  set build-id-dirs += /home/alice/project/out/x64/.build-id
  ```

* {ffx}

  Note: These paths are added to the configuration of zxdb and persist
  through zxdb instances. Make sure to use a path that is relative to your
  Fuchsia checkout


  ```posix-terminal
  ffx debug symbol-index add out/default/.build-id
  ```

These directories are then annotated with `(folder)` in the output of the
`sym-stat` instead of the number of binaries that are contained in the directory.
These binaries are not displayed in the `sym-stat --dump-index` output because
zxdb searches these directories on demand when searching for symbols rather than
enumerating them in advance.

### Individual files and directories

Caution: If you are working from a Fuchsia checkout or with a Fuchsia SDK, you
don't need to consider the information from this section.

If you have a single binary file without one of the other symbol database
formats, you can configure zxdb for that specific file. You can use configure
the `symbol-paths` for a particular file by adding it to the list of paths.

For example, to add `/home/alice/project/a.out` to the `symbol-paths` setting:

Note: This setting also accepts directory names. In the case of directories
case, zxdb non-recursively enumerates all files in that directory and attempts
to find binaries with build IDs.

* {File}

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  set symbol-paths += /home/alice/project/a.out
  ```

* {Directory}

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  set symbol-paths += /home/me/project/build/
  ```

You can see the status of the locations you configured with the `sym-stat`
command. For example:

Note: You can also see the build IDs and file names of the binaries added with
the `sym-stat --dump-index` command.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
sym-stat
Symbol index status

  This command just refreshed the index.
  Use "sym-stat --dump-index" to see the individual mappings.

   Indexed  Source path
         1  /home/alice/a.out
         2  /home/alice/project/build/
```

### ids.txt symbol index

Some older internal Google projects generate a file called `ids.txt`. This file
provides a mapping from a binary's build ID to the symbol path on the local
system. If your build produces such a file and it is not automatically loaded,
you can specify it for Zxdb with the `ids-txts` setting (a list of file names):

Note: `ids-txts` is a list of file names.

For example, to add `/home/alice/project/build/ids.txt` to the `ids-txts` setting:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
set ids-txts += /home/alice/project/build/ids.txt
```

The symbol files from `ids.txt` files is also displayed when you run the
`sym-stat` or the `sym-stat --dump-index` commands.

## Symbol settings {#symbol-settings}

The settings described in
[Understand how Zxdb loads symbols](#understand-load-symbols) section get
automatically applied by your environment. This section describes how these
settings are set.

The `symbol-index-files` setting contains one or more JSON-format files that
are set by the development environment:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
get symbol-index-files
symbol-index-files =
  • /home/alice/.fuchsia/debug/symbol-index.json
```

This file can contain some global settings and reference other `symbol-index`
files. Typically each build environment that you are actively using has a
similar file that is referenced from this global file.

If you are switching between build environments and notice that symbols aren't
loading, make sure that your environment is registered with the
`ffx debug symbol-index list` command.

[advanced-download-symbols]: /docs/development/debugger/advanced.md#download-symbols
[troubleshoot-symbols]: /docs/development/debugger/troubleshooting.md#diagnose-problems-symbols
[ffx-symbol-index]: /reference/tools/sdk/ffx.md#symbol-index
