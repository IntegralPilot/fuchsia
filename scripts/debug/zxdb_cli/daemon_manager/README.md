# DaemonManager

`DaemonManager` is a lightweight, client-side library responsible for
spawning, connecting to, and managing the lifecycle of the background
`zxdb-daemon` process.

## Core Features

- **Subprocess Lifecycle Management**: Spawns `zxdb-daemon` using a
  platform-specific command-line wrapper (`FxCmd`).
- **Startup Synchronization**: Block-waits for daemon readiness via an
  inherited pipe file descriptor (`--ready-fd`), guarding against premature
  crashes and startup timeouts.
- **Unix Domain Socket (UDS) Connectivity**: Connects to the daemon over a
  local socket (default: `/tmp/fx-debug-daemon.sock`) and executes DAP session
  handshake verifications (`HelloRequest`/`StartRequest`).
- **Fault Tolerance & Cleanup**: Automatically unlinks stale sockets on
  startup, prevents double-running daemon processes, and guarantees graceful
  (`SIGTERM`) or forceful (`SIGKILL`) process group cleanup on failures.

## Directory Layout

- [manager.py](manager.py): Core implementation of the `DaemonManager` class,
  lifecycle exceptions, and helper processes.
- [tests/test_manager.py](tests/test_manager.py): Thorough asynchronous unit
  and process-group integration tests.
- [BUILD.gn](BUILD.gn): Declares the python target library and its
  test target.

## Usage

Downstream tools (such as `fx test`) import `DaemonManager` directly to
coordinate background debugger attachments:

```python
from daemon_manager import DaemonManager

manager = DaemonManager(port=15678, connect_to_existing=True)
process = await manager.start()
```
