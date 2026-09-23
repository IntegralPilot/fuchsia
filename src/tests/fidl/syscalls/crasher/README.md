# Channel Overflow Crasher

This is a simple test / example of crashing processes by asking a peer to
over-fill their channel message queues. It works by repeatedly sending
`Directory.ReadDirents` calls to the server backing `/svc` (generally Component
Manager) without ever reading replies. The peer has no way of knowing that their
replies aren't being read and then are killed by the kernel to avoid consuming
too much kernel memory.

Run this by adding `//src/tests/fidl/syscalls/crasher` to the build and running:
```
ffx component run --recreate /core/ffx-laboratory:crasher 'fuchsia-pkg://fuchsia.com/crasher#meta/crasher.cm'
```
