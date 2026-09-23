# Starnix virtual file system (VFS)

The page examines the key concepts and underlying structure of [Starnix][starnix]'s
virtual file system (VFS):

- [Motivation for Starnix's own virtual file system](#motivation-for-starnix-vfs)
- [Building blocks of the Starnix VFS](#building-blocks-of-starnix-vfs)
- [Mounted file systems in the Starnix VFS](#mounted-file-systems-in-starnix-vfs)

## Motivation for Starnix's own virtual file system {:#motivation-for-starnix-vfs}

Starnix has its own virtual file system (which is separate from
[Fuchsia's VFS][fuchsia-vfs]) for the reasons below:

- **Performance** – The Starnix VFS allows many file system operations to avoid
  making FIDL calls. The Starnix VFS does this by caching the results from
  previous FIDL calls and implementing the entire file system in a process
  (similar to [`tmpfs`][tmpfs]{:.external} and [`procfs`][procfs]{:.external}).

- **Compatibility** – A file system for Starnix must support several Linux
  file system operations that are not supported by Fuchsia's VFS libraries,
  which many Linux-based operating systems rely on. In particular, the following
  two operations are hard to retrofit into Fuchsia's existing VFS design:

  - Mounting directories – A Linux file system allows a directory of one
    file system to be mounted to a directory of another file system.

  - Tracking a file's path after `open()` – A Linux file system can use
    a file descriptor to determine the path of the file that is opened by an
    `open()` call. If the file or one of its parent directories gets moved or
    renamed, the file's path must account for this change.

## Building blocks of the Starnix VFS {:#building-blocks-of-starnix-vfs}

This section constructs a simple file system using the fundamental building
blocks of the Starnix VFS:

- [Files and directories](#files-and-directories)
- [A file system](#a-file-system)

### Files and directories {:#files-and-directories}

At the core of the Starnix VFS is a collection of `FsNode` instances. Each `FsNode`
contains information about a file or a directory (see Figure 1). File system
operations, such as reading, writing, and retrieving file information, are
performed on `FsNode` instances.

![A simple file system](images/starnix-vfs-01.png "Diagram showing a simepl file
system in the Starnix VFS")

**Figure 1**. A simple file system with a directory and a file.

A `DirEntry` assigns a name to an `FsNode`. This mapping of `DirEntry` to `FsNode`
is referred to as a hard link. Multiple `DirEntry` instances can be mapped to a
single file-typed `FsNode` (see Figure 2). This allows multiple paths in a file system
to be resolved to the same `FsNode`. As a result, multiple paths can be resolved to the
same file content and attributes in a file system. However, while a file can have
multiple hard links, a directory (that is, a directory-typed `FsNode`) can only have
one hard link.

![A file system with two DirEntry instances](images/starnix-vfs-02.png "Diagram
showing the mapping of two DirEntry instances to one FsNode in the Starnix VFS")

**Figure 2**. Multiple `DirEntry` instances can be mapped to the same `FsNode`.

### A file system {:#a-file-system}

A file system tracks information on a group of related files and directories. In
the Starnix VFS, a file system (that is, a single instance of `FileSystem`)
represents a collection of `DirEntry` instances, and each `FileSystem` instance
contains the following items:

- A `DirEntry` pointing to the root directory
- A cache of `FsNode` instances (representing files and directories under the
  root directory)

![A FileSystem](images/starnix-vfs-03.png "Diagram showing a simple FileSystem
instance in the Starnix VFS")

**Figure 3**. A `FileSystem` tracks `DirEntry` and `FsNode` instances.

## Mounted file systems in the Starnix VFS {:#mounted-file-systems-in-starnix-vfs}

Mounting allows users to seamlessly move from one file system to another file
system. A file system and its contents become available to users (or tasks) once
the file system is mounted to a certain directory. This "mount point" directory
is often a directory of another file system that the users already have access to.
For example, in Figure 4, users can reach the `file1.txt` file in the Example FS
(file system) by using the path `/example/file1.txt` from the Parent FS.

![Mounted file systems](images/starnix-vfs-04.png "Diagram showing a file system
mounted to another file system in the Starnix VFS")

**Figure 4**. The root directory of the Example FS is mounted to the `example`
directory of the Parent FS.

Mounting a file system and tracking mount points in the Starnix VFS involve
the following instances:

- [Mount](#mount)
- [NamespaceNode](#namespacenode)

### Mount {:#mount}

A `Mount` makes a `FileSystem` accessible at the mount point directory of
another `FileSystem`. To put it differently, a `Mount` is used to link the
following two directories:

- A mount point directory in the parent `FileSystem`
- The root directory of the child `FileSystem`

In Figure 4, the Parent FS has the `example` directory as a subdirectory and the
Example FS is mounted on this `example` directory (which is the mount point
directory). In this setup, when you change your working directory to the
`example` directory in the Parent FS (for instance, `cd example` is run), you
"enter the mount," that is, you are now in the root directory of the Example FS.

![A mounted file system](images/starnix-vfs-05.png "Diagram showing a Mount
pointing to a mounted file system in the Starnix VFS")

**Figure 5**. A `Mount` tracks the root directory of a mounted `FileSystem`.

To enable the feature of seamlessly moving from the parent `FileSystem` into the
child `FileSystem` and vice versa, a `Mount` tracks a `DirEntry` that points to the
root directory of the mounted `FileSystem` (see Figure 5). And a `Mount` also tracks
its parent `Mount` using the `mountpoint` pointer (see
[`NamespaceNode`](#namespacenode)) and its child `Mount` instances using the
`submounts` pointer. (see Figure 6).

![Mount instances](images/starnix-vfs-06.png "Diagram showing how Mount instances
are used to track mounted file systems in the Starnix VFS")

**Figure 6**. `Mount`'s `mountpoint` and `submounts` pointers are used to move
between mounted file systems.

It's important to note that a `FileSystem` can be mounted to a number of different
`FileSystem` instances at once. This allows the same `FileSystem` to be reached from
multiple parent `FileSystem` instances, enabling various paths (or
[symlinks](#symbolic-links)) to be used to reach the same file in the mounted
`FileSystem`.

### NamespaceNode {:#namespacenode}

Every task in Starnix contains an instance of `FsContext`. An `FsContext` holds
information about the task's association with the Starnix VFS, except for the
file descriptor table.

Importantly, an `FsContext` contains a `Namespace`, which
enables the task to identify all `FileSystem` instances that are mounted under
this namespace. This is possible because each `Namespace` contains a `Mount` that
tracks the "root mount" in the namespace (see Figure 7). Using the root mount's
`submounts` pointer (which points to a map of all `DirEntry` instances and their
respective `Mount` instances in the namespace), the task can reach all of the
other mounted `FileSystem` instances under this namespace.

![FsContext and Namespace](images/starnix-vfs-07.png "Diagram showing FsContext and
Namespace in the Starnix VFS")

**Figure 7**. An `FsContext` has a `Namespace`, which has a `Mount` for tracking the
root mount.

A `NamespaceNode` is useful for traversing paths in a `Namespace`. Each
`NamespaceNode` tracks a `Mount` and its respective `DirEntry`.

However, because the same `FileSystem` can be mounted multiple times in a
`Namespace` (for instance, at different mount point directories in the same parent
`FileSystem`), multiple `NamespaceNode` instances may be necessary to track these
different `Mount` instances that lead to the same underlying `FileSystem`.

![NamespaceNodes instances](images/starnix-vfs-08.png "Diagram showing NamespaceNodes
in the Starnix VFS")

**Figure 8**. A `FsContext` has two `NamespaceNode` instances for tracking specific
directories: root and cwd.

For instance, each `FsContext` contains, at a minimum, two `NamespaceNode`
instances for tracking the following directories (see Figure 8):

- The root directory
- The current working directory (cwd)

Both `NamespaceNode` instances are used for looking up paths in this `Namespace`. A
path starting with `/` (for example, `/example/file1.txt`) is looked up respective
to the root `NamespaceNode`, and a path without a starting `/` (for example,
`file1.txt`) is looked up using the cwd `NamespaceNode`.

![A simple namespace](images/starnix-vfs-09.png "Diagram showing a simple namespace
in the Starnix VFS")

**Figure 9**. A simple namespace with a single (root) `Mount`.

The diagram in Figure 9 illustrates a namespace with a single `Mount` where the
current working directory (cwd) also happens to be the root directory. The diagram
in Figure 10 shows that the current working directory is changed to the `example`
directory (for instance, `cd example` is run), which causes the `entry` pointer of
the cwd `NamespaceNode` to be updated to the `example` directory.

![cwd NamespaceNode](images/starnix-vfs-10.png "Diagram showing that cwd NamespaceNode
points a subdirectory in the Starnix VFS")

**Figure 10**. The current working directory is changed to the `example` directory
in the namespace.

The diagram in Figure 11 shows a namespace where the Example FS is mounted to the
Parent FS. Under this scenario, when `cd example` is run, the `mount` pointer of
the cwd `NamespaceNode` is updated to a new `Mount` (labeled "Example Mount") that
tracks the root directory of the Example FS. Using this new `Mount`, the task in
the `FsContext` can seamlessly move from the Parent FS to the Example FS when
traversing paths.

![A namespace with mounted file systems](images/starnix-vfs-11.png "Diagram showing
a namespace with mounted file systems in the Starnix VFS")

**Figure 11**. A namespace where the Example FS is mounted to the `example` directory of
the Parent FS.

#### Symbolic links {:#symbolic-links}

Symbolic links (or symlinks) are handled while walking `NamespaceNode` instances.
If the task hits a symlink and asks for its path, the path of this symlink is
resolved recursively from the `NamespaceNode`.

<!-- Reference links -->

[starnix]: /docs/concepts/starnix/syscalls.md
[fuchsia-vfs]: /docs/concepts/filesystems/filesystems.md
[tmpfs]: https://en.wikipedia.org/wiki/Tmpfs
[procfs]: https://en.wikipedia.org/wiki/Procfs
