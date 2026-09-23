# memfs: a simple in-memory filesystem

This library implements a simple in-memory filesystem for Fuchsia.

It currently has no settings. Because it uses the C allocator, it can expand to
fill all available memory and the max size values from POSIX `statvfs()` and FIDL
`fuchsia.io.Node.QueryFilesystem()` may not have much meaning.
