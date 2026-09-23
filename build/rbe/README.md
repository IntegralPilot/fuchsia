This directory contains support for building the Fuchsia tree
with build actions running on RBE (remote build execution).

# Remote execution wrappers

The top-level remote execution wrappers are used as command prefixes:

*   `reclient_cxx.sh`: prefix wrapper for remote compiling C++
    *   `cxx_remote_wrapper.py`: is a similar heavier wrapper with additional features
        and workarounds.
*   `rustc_remote_wrapper.py`: prefix wrapper for remote compiling and linking Rust
    *   Detects and gathers all inputs and tools needed for remote compiling.
    *   Detects extra outputs to download produced by remote compiling.
*   `cxx_link_remote_wrapper.py`: prefix wrapper for remote ilnking C++
*   `prebuilt_tool_remote_wrapper.py`: prefix wrapper for simple tools
*   `remote_action.py`: prefix wrapper for generic remote actions
    * Exists as standalone wrapper and library.
    * Includes detailed diagnostics for certain error conditions.
    * Includes limited fault-tolerance and retries.
    * The C++ and Rust wrappers inherit all of the features of this generic
      script.
*   `fuchsia-reproxy-wrap.sh`: automatically start/shutdown `reproxy` (needed by
    `rewrapper`) around any command.  Used by `fx build`.
*   `dlwrap.py`: downloads artifacts from RBE's CAS using download stub files

More details can be found by running with `--help`.

## Support scripts

*   `cl_utils.py`: generic command-line operation library
*   `cxx.py`: for understanding structure of C/C++ compile commands
*   `fuchsia.py`: Fuchsia-tree specific directory layouts and conventions.
    Parties interested in re-using wrapper scripts found here should expect
    to replace this file.
*   `output_leak_scanner.py`: Check that commands and outputs do not leak
    the name of the build output directory, for better caching outcomes.
*   `relativize_args.py`: Attempt to transform commands with absolute paths
    into equivalent commands with relative paths.  This can be useful for
    build systems that insist on absolute paths, like `cmake`.
*   `rustc.py`: for understanding structure of `rustc` compile commands
*   `tablefmt.py`: utilities for printing readable ASCII tables
*   `textpb.py`: generic protobuf text parsing library

## Configurations

*   `fuchsia-rewrapper.cfg`: rewrapper configuration
*   `fuchsia-reproxy.cfg`: reproxy configuration

# Troubleshooting tools

*   `action_diff.sh`: recursively queries through two reproxy logs to
    root-cause differences between output and intermediate artifacts.
    This is useful for examining unexpected digest differences and
    cache misses.
*   `bbtool.py`: buildbucket related tools
    *   `fetch_reproxy_log`: retrieve .rrpl log, possibly from subbuild.
*   `bb_fetch_rbe_cas.sh`: retrieve a remote-built artifact from the
    RBE CAS using a buildbucket id and the path under the build output
    directory.
*   `cas.py`: interface to CAS tool, including fetching objects.
*   `detail-diff.sh`: attempts to compare human-readable representations
    of files, including some binary files, by using tools like `objdump`.
*   `remotetool.sh`: can lookup actions and artifacts in the RBE CAS.
    From: https://github.com/bazelbuild/remote-apis-sdks
*   `reproxy_logs.sh`: subcommand utility
    *   `diff`: report meaningful difference between two reproxy logs
    *   `output_file_digest`: lookup the digest of a remote built artifact
    *   `bandwidth`: report total download and upload bytes from reproxy log
    *   `plot_download`: plot download demand over the course of a build
*   `rpl_tool.sh`: tools that involve reproxy logs and remotetool together
    *   `expand_to_rpl`: populate .rrpl with command and inputs -> .rpl

All tools have more detailed usage with `--help`.

# GN files

*   `build/toolchain/rbe.gni`: global `args.gn` variables for RBE

*   `build/toolchain/clang_toolchain.gni` and
    `build/toolchain/zircon/zircon_toolchain.gni`: use RBE wrappers depending on
    configuration
*   `build/rust/rustc_*.gni`: uses RBE wrappers depending on configuration

# Metrics and logs

*   `build_summary.py`: If your environment sets `FX_BUILD_RBE_STATS=1`,
    this script will be run after each build and display a summary of
    cache and download metrics.
*   `upload_reproxy_logs.py`: pushes RBE metrics and detailed logs to
     BigQuery.
     *   This can be enabled by `fx build-metrics`.
*   `pb_message_util.py`: library for translating protobufs to JSON
