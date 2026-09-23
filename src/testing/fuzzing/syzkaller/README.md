This directory contains definitions needed to build and run
[syzkaller](https://github.com/google/syzkaller) against Fuchsia's system call
interfaces.

- The top-level directory is relevant for Zircon fuzzing (currently on pause:
  https://fxbug.dev/296220035). Its BUILD.gn contains rules to build syzkaller's
  `syz-executor` component for Fuchsia.

- The `starnix` subdirectory is relevant for Starnix fuzzing (currently active).
