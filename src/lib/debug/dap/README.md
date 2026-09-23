# Debug Adapter Protocol (DAP) Library


This library provides a complete and extensible implementation of the [Debug
Adapter Protocol
(DAP)](https://microsoft.github.io/debug-adapter-protocol/specification) in
Python.

## Features

- **Protocol Models**: Full set of Python types representing DAP requests,
  responses, and events.
- **Client Implementation**: A `DapClient` that handles the message framing
  (Content-Length headers) and asynchronous request/response matching.
- **Extensible Architecture**: Easy to add vendor-specific extensions without
  modifying the core library.


## Client Usage Example

Here is a basic example of how to use the `DapClient` to connect to a debug
adapter and send an initialize request:

```python
import asyncio
from pydap.client import DapClient
from pydap.models import InitializeArguments


async def run():
    client = DapClient()

    # Connect to the debug adapter (e.g., via TCP)
    reader, writer = await asyncio.open_connection("127.0.0.1", 12345)

    # Send initialize request
    args = InitializeArguments(adapterID="test")

    # We create a task to send the request
    # In a full implementation, you would also have a task reading from 'reader'
    response = await client.initialize(writer, args)
    print(f"Response: {response}")


asyncio.run(run())
```


## Naming Convention

To ensure idiomatic Python development while maintaining exact compliance with the DAP specification, this library defines models using standard Python `snake_case` (e.g., `adapter_id`) and uses Pydantic's aliasing feature to automatically serialize them to the required `camelCase` or acronym casing (e.g., `adapterID`) as defined in the official protocol.

Thanks to Pydantic's `populate_by_name` configuration, you can instantiate models using either snake_case or camelCase keyword arguments, though snake_case is preferred for idiomatic code.

## Extensibility

One of the key design goals of this library is extensibility. The base protocol is often extended by specific debuggers to support unique features.

To extend the library:
1. Define your custom argument types by subclassing `DapBaseModel`.
2. Use the protected `DapClient::_send_request` method directly with your custom command string and model instance.

### Example: Custom Profiling Extension

Suppose you are building a debug adapter that supports a custom profiling feature not covered by the standard protocol. You can extend `DapClient` to support this server feature by subclassing or using compositional mixin classes.

#### Step 1: Define Custom Arguments
```python
from pydap.dap_types import DapBaseModel


class StartProfilingArguments(DapBaseModel):
    # Duration in milliseconds.
    duration: int
```

#### Step 2a: Direct Subclassing
```python
import asyncio
from pydap.client import DapClient


class CustomDapClient(DapClient):

    async def start_profiling(
        self, writer: asyncio.StreamWriter, duration: int
    ):
        args = StartProfilingArguments(duration=duration)
        return await self._send_request(writer, "startProfiling", args)
```

#### Step 2b: Compositional Mixins (Recommended for Modular Extensions)
For complex clients with multiple independent extensions, mixin classes provide a cleaner architectural pattern:

```python
import asyncio
from typing import Any, Protocol
from pydap.client import DapClient
from pydap.dap_types import DapBaseModel


# Define a protocol for type checking within the mixin
class DapClientProtocol(Protocol):

    async def _send_request(
        self,
        writer: asyncio.StreamWriter,
        command: str,
        arguments: DapBaseModel | None = None,
        timeout: float = 5.0,
    ) -> dict[str, Any]: ...


class ProfilingMixin:
    """Mixin class adding profiling support to a DapClient."""

    async def start_profiling(
        self: DapClientProtocol, writer: asyncio.StreamWriter, duration: int
    ) -> dict[str, Any]:
        args = StartProfilingArguments(duration=duration)
        return await self._send_request(writer, "startProfiling", args)


# Compose your final client from base DapClient and mixins
class ExtendedDapClient(DapClient, ProfilingMixin):
    pass
```
