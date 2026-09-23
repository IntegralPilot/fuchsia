---
name: fidl-to-honeydew-affordance
description: >
  Guide for translating FIDL protocols and data types in sdk/fidl into Honeydew
  affordances following the asynchronous Fuchsia Controller and WLAN affordance
  architectural patterns.
---

# Translating FIDLs into Honeydew Affordances

This skill provides a standardized methodology for translating Fuchsia FIDL definitions (under `sdk/fidl/`) into robust, ergonomic, and asynchronous Python affordances in Honeydew (`src/testing/end_to_end/honeydew/affordances/`).

It follows the modern Honeydew architectural conventions demonstrated in `affordances/connectivity/wlan/` (such as `wlan_policy` and `wlan_core`) and `affordances/drivers/battery/`.

---

## 1. Honeydew Affordance Architectural Pattern

A complete Honeydew affordance translating a FIDL protocol typically consists of the following modular structure:

```
src/testing/end_to_end/honeydew/honeydew/affordances/<domain>/<affordance_name>/
├── __init__.py               # Main affordance class (implements AsyncLazyReady)
├── utils/
│   ├── __init__.py
│   ├── errors.py             # Custom exceptions mapping FIDL error enums/failures
│   └── types.py              # Frozen dataclasses with from_fidl() and to_fidl()
└── tests/
    ├── __init__.py
    ├── BUILD.gn              # GN build targets for unit tests
    ├── unit_tests.py         # IsolatedAsyncioTestCase with mocked FuchsiaController
    └── functional_tests.py   # Mobly E2E tests against real/emulated hardware
```

In addition, the affordance is integrated into:
- `src/testing/end_to_end/honeydew/honeydew/fuchsia_device/fuchsia_device.py`: Expose the affordance via an `@properties.Affordance` property on `FuchsiaDevice`.
- `src/testing/end_to_end/honeydew/BUILD.gn`: Add all source files to `sources`, the FIDL Python IR dependencies (`<fidl_target>_python`) to `honeydew_fidl_ir_and_deps`, and the unit tests to `group("unit_tests")`.

---

## 2. Step-by-Step Translation Workflow

### Step 1: Analyze the FIDL Specification
Inspect `sdk/fidl/<library>/<file>.fidl` to identify:
1. **Types (`struct`, `table`, `enum`, `union`)**: Note optional vs required fields, nested types, Zircon primitives (e.g., `zx.Duration`, `zx.Time`), and byte vectors.
2. **Errors (`error <ErrorEnum>`)**: Note all error enum variants returned by methods.
3. **Protocols & Methods**: Identify request-response calls, hanging-get watchers, event streams, and channel closures.
4. **Discoverability & Transport**: Moniker/capability routing (e.g. `@discoverable`, devfs node path, or component expose/use).

---

### Step 2: Define Data Types (`utils/types.py`)

Honeydew decouples test code from raw FIDL bindings by wrapping FIDL tables and structs in immutable Python dataclasses:
- Use `@dataclass(frozen=True)` for data structures.
- Provide `@classmethod def from_fidl(cls, fidl: ...) -> Self` to parse FIDL structures into Python types.
- Provide `def to_fidl(self) -> ...` if the affordance sends data back to the device.
- Convert raw FIDL types into idiomatic Python types:
  - Byte vectors (`vector<uint8>` / `string`) $\to$ `str` or `bytes`.
  - `zx.Duration` (nanoseconds) $\to$ `datetime.timedelta`.
  - FIDL flexible/strict enums $\to$ Python `enum.IntEnum` or validated attributes.
  - Nested tables $\to$ nested dataclasses with `None` defaults for unset fields.

#### Generic Pattern:
```python
from __future__ import annotations
import enum
from dataclasses import dataclass
from datetime import timedelta
import fidl_fuchsia_example_foo as f_foo

class FooState(enum.IntEnum):
    UNKNOWN = 0
    ACTIVE = 1
    SUSPENDED = 2

@dataclass(frozen=True)
class FooStatus:
    state: FooState | None = None
    duration: timedelta | None = None
    name: str | None = None

    @classmethod
    def from_fidl(cls, fidl: f_foo.Status) -> FooStatus:
        duration = (
            timedelta(microseconds=fidl.duration // 1000)
            if fidl.duration is not None
            else None
        )
        state = FooState(fidl.state) if fidl.state is not None else None
        return cls(state=state, duration=duration, name=fidl.name)

    def to_fidl(self) -> f_foo.Status:
        duration_ns = (
            int(self.duration.total_seconds() * 1_000_000_000)
            if self.duration is not None
            else None
        )
        state = f_foo.FooState(self.state.value) if self.state is not None else None
        return f_foo.Status(state=state, duration=duration_ns, name=self.name)
```

---

### Step 3: Define Custom Exceptions (`utils/errors.py`)

Create a domain-specific exception hierarchy inheriting from `honeydew.errors.HoneydewError`:
- Define a base exception for the affordance (e.g. `Honeydew<Affordance>Error`).
- Define specific exceptions for protocol-level errors, state mismatches, device not found, or transport timeouts.

#### Generic Pattern:
```python
from honeydew import errors
import fidl_fuchsia_example_foo as f_foo

class HoneydewFooError(errors.HoneydewError):
    """Base exception for Foo affordance operations."""

class FooRequestError(HoneydewFooError):
    """Raised when the Foo service returns an error code."""
    def __init__(self, method: str, error: f_foo.Error) -> None:
        super().__init__(f"{method} failed with error {error.name} ({error.value})")
        self.error = error

class FooDeviceNotFoundError(HoneydewFooError):
    """Raised when the target Foo hardware or capability cannot be found."""
```

---

### Step 4: Implement Affordance Class (`__init__.py`)

The main class should:
1. Inherit from `AsyncLazyReady` to enable asynchronous lazy connection management.
2. Accept standard dependencies in `__init__`: `device_name`, `ffx`, `fuchsia_controller`, `reboot_affordance`, `fuchsia_device_close`.
3. Call `self.verify_supported()` in `__init__` (checking capabilities via `ffx component capability` or devfs).
4. Register reboot and close hooks:
   - `reboot_affordance.register_for_on_device_boot(self.make_ready)`
   - `fuchsia_device_close.register_for_on_device_close(self._close)`
5. Implement `make_ready(self)` to connect to the FIDL proxy using `self._fc_transport.connect_device_proxy(...)` or devfs channel.
6. Decorate public methods with `@ensure_ready`.
7. Handle FIDL transport errors (`FcTransportStatus`, `ZxStatus`) and unwrap result unions.

#### Generic Pattern:
```python
from __future__ import annotations
import logging
from fuchsia_controller_py import FcTransportStatus, ZxStatus
from honeydew import affordances_capable, errors
from honeydew.affordances.affordance import AsyncLazyReady, ensure_ready
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.ffx import types as ffx_types
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing.custom_types import FidlEndpoint
import fidl_fuchsia_example_foo as f_foo

from .utils.errors import FooDeviceNotFoundError, FooRequestError, HoneydewFooError
from .utils.types import FooStatus

_LOGGER = logging.getLogger(__name__)
_FOO_MONIKER = "bootstrap/foo-service"
_FOO_CAPABILITY = "fuchsia.example.foo.Device"
_REQUIRED_CAPABILITIES = [_FOO_CAPABILITY]

class Foo(AsyncLazyReady):
    """Foo affordance implementation using Fuchsia Controller."""

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
        fuchsia_device_close: affordances_capable.FuchsiaDeviceClose,
    ) -> None:
        super().__init__()
        self._device_name = device_name
        self._ffx = ffx
        self._fc_transport = fuchsia_controller
        self._reboot_affordance = reboot_affordance
        self._fuchsia_device_close = fuchsia_device_close
        self._proxy: f_foo.DeviceClient | None = None

        self.verify_supported()
        self._reboot_affordance.register_for_on_device_boot(self.make_ready)
        self._fuchsia_device_close.register_for_on_device_close(self._close)

    def verify_supported(self) -> None:
        """Verifies device support."""
        for capability in _REQUIRED_CAPABILITIES:
            output = self._ffx.run(
                ["component", "capability", capability],
                machine=ffx_types.MachineFormat.RAW,
            )
            if capability not in output:
                raise errors.NotSupportedError(
                    f"Capability '{capability}' not supported on {self._device_name}"
                )

    async def make_ready(self) -> None:
        """Establishes connection to the FIDL service."""
        await super().make_ready()
        try:
            endpoint = FidlEndpoint(_FOO_MONIKER, _FOO_CAPABILITY)
            channel = self._fc_transport.connect_device_proxy(endpoint)
            self._proxy = f_foo.DeviceClient(channel)
        except Exception as err:
            raise FooDeviceNotFoundError(f"Failed to connect to {_FOO_MONIKER}") from err

    async def _close(self) -> None:
        """Clean up proxy connection on device close."""
        self._proxy = None

    @ensure_ready
    async def get_status(self) -> FooStatus:
        """Retrieves dynamic status."""
        assert self._proxy is not None
        try:
            res = await self._proxy.get_status()
            if res.err is not None:
                raise FooRequestError("get_status", res.err)
            assert res.response is not None
            return FooStatus.from_fidl(res.response.status)
        except (FcTransportStatus, ZxStatus) as err:
            raise HoneydewFooError("Transport error during get_status") from err
```

---

### Step 5: Implement Unit Tests (`tests/unit_tests.py`) & GN (`tests/BUILD.gn`)

Unit tests verify conversion logic, state management, and error handling in isolation:
- Inherit from `unittest.IsolatedAsyncioTestCase`.
- Reset `GlobalHandleWaker()._reset_for_testing()` in `asyncSetUp()`.
- Mock `FuchsiaController`, `FFX`, and the FIDL proxy.

#### Example `tests/BUILD.gn`:
```gn
import("//build/python/python_host_test.gni")

assert(is_host, "Python only supported in host toolchains.")

python_host_test("unit_tests") {
  main_source = "unit_tests.py"
  libraries = [ "//src/testing/end_to_end/honeydew" ]
  main_callable = "unittest.main"
  extra_args = [ "-v" ]
}
```

---

### Step 6: Update Honeydew Wiring

1. **`src/testing/end_to_end/honeydew/BUILD.gn`**:
   - Add new source files to `sources` under `python_library("honeydew_no_testonly")`.
   - Add FIDL Python bindings under `group("honeydew_fidl_ir_and_deps")`:
     ```gn
     "//sdk/fidl/<library>:<library>_python",
     ```
     > [!NOTE]
     > Do not add FIDL dependencies directly to `group("honeydew")`. Adding them to `group("honeydew_fidl_ir_and_deps")` ensures non-test targets that depend on `honeydew_no_testonly` receive the bindings at runtime.
     > If the FIDL library is unstable (`stable = false` or not in partner SDK), add it to `host_test_allowlist.gni` and `src/developer/ffx/build/ffx_subtool_allowlist.gni` (`ffx_subtool_fidl_partner_unstable_allowlist`) to satisfy IDK compatibility checks.
   - Add unit test targets to `group("unit_tests")`:
     ```gn
     "honeydew/affordances/<domain>/<name>/tests:unit_tests($host_toolchain)",
     ```
2. **`src/testing/end_to_end/honeydew/honeydew/fuchsia_device/fuchsia_device.py`**:
   - Import the affordance module.
   - Expose via `@properties.Affordance`.

---

## 3. In-Tree Reference Implementations

Refer to the following in-tree implementations for complete, production-grade examples:
* **WLAN Affordances**: [`src/testing/end_to_end/honeydew/honeydew/affordances/connectivity/wlan/`](//src/testing/end_to_end/honeydew/honeydew/affordances/connectivity/wlan/)
* **Driver Battery Affordance**: [`src/testing/end_to_end/honeydew/honeydew/affordances/drivers/battery/`](//src/testing/end_to_end/honeydew/honeydew/affordances/drivers/battery/)
  * `utils/types.py`: Mapping `fuchsia.hardware.power.battery` & `fuchsia.hardware.power.source` tables.
  * `utils/errors.py`: Mapping FIDL `fuchsia.hardware.power.source.Error` enums.
  * `__init__.py`: Async battery affordance with `get_spec()`, `get_status()`, and hanging `watch()`.
  * `tests/unit_tests.py`: Mocked unit test suite.
