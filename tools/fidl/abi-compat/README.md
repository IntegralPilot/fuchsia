# IPC ABI Compatibility

This tool evaluates the ABI (ie: runtime) compatibility between external
components targeting a single supported API level and platform components
targeting supported, in-development and `HEAD` levels.

This tool produces reports at `root_build_dir/compatibility-report-N.txt` for
each supported API level `N`.

## Limitations
The tool and the reports are currently a work in progress.

They do not yet take into account where types may be read or written using
standalone encoding functions.

For soft breakages the reports don't give useful guidance on how to correctly
implement platform components such that they will be compatible with all
supported versions.


