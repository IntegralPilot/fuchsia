# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, Literal

SteppingGranularity = Literal["statement", "line", "instruction"]

from pydantic import BaseModel, ConfigDict
from pydantic.alias_generators import to_camel


class DapBaseModel(BaseModel):
    """Base model for all DAP types, configuring camelCase alias generation."""

    # The DAP schema specifies methods and key names for arguments are camel case, so when
    # serializing to and from JSON we can use pydantic's built in conversion to do this for us.
    # There are few exceptions to this, for example `adapterID` in InitializeArguments or
    # `request_seq` in the Response type (see models.py). For these cases specify the alias manually
    # using pydantic.Field(alias="...").
    #
    # See more: https://microsoft.github.io/debug-adapter-protocol/overview#base-protocol.
    model_config = ConfigDict(
        alias_generator=to_camel,
        populate_by_name=True,
    )

    def dump_dap(self) -> dict[str, Any]:
        """Dumps the model to a dictionary suitable for DAP serialization."""
        return self.model_dump(exclude_none=True, by_alias=True)


class Source(DapBaseModel):
    """A Source is a descriptor for source code.

    Attributes:
        name: The short name of the source.
        path: The path of the source to be shown in the UI.
        source_reference: If the value > 0 the contents of the source must be
            retrieved through the `source` request.
        origin: The origin of this source (e.g., 'internal module', 'inlined content
            from source map').
    """

    name: str | None = None
    path: str | None = None
    source_reference: int | None = None
    origin: str | None = None


class StackFrame(DapBaseModel):
    """A StackFrame.

    Attributes:
        id: An identifier for the stack frame. It must be unique across all threads.
            This id can be used to retrieve the scopes of the frame with the `scopes`
            request or to restart the execution of a stack frame.
        name: The name of the stack frame, typically a method name.
        line: The line within the source of the frame. If the source attribute is missing
              or doesn't exist, `line` is 0 and should be ignored by the client.
        column: Start position of the range covered by the stack frame. It is measured in
                UTF-16 code units and the client capability `columnsStartAt1` determines
                whether it is 0- or 1-based. If attribute `source` is missing or doesn't
                exist, `column` is 0 and should be ignored by the client.
        source: The source of the frame.
        presentation_hint: A hint for how to present this frame in the UI. A value of
            'label' indicates an artificial frame used as a visual separator; 'subtle'
            indicates a subtle UI appearance. Values: 'normal', 'label', 'subtle'.
    """

    id: int
    name: str
    line: int
    column: int
    source: Source | None = None
    presentation_hint: str | None = None


class Thread(DapBaseModel):
    """A Thread.

    Attributes:
        id: Unique identifier for the thread.
        name: The name of the thread.
    """

    id: int
    name: str


class Scope(DapBaseModel):
    """A Scope.

    Attributes:
        name: Name of the scope.
        variables_reference: The variables of this scope can be retrieved by passing this
            value to the `variables` request.
        expensive: If true, the number of variables in this scope is large or expensive to retrieve.
    """

    name: str
    variables_reference: int
    expensive: bool


class Variable(DapBaseModel):
    """A Variable.

    Attributes:
        name: The variable's name.
        value: The variable's value.
        variables_reference: If variables_reference is > 0, the variable is structured and its children
            can be retrieved by passing variables_reference to the `variables` request.
        type: The type of the variable's value.
    """

    name: str
    value: str
    variables_reference: int
    type: str | None = None


class SourceBreakpoint(DapBaseModel):
    """A SourceBreakpoint.

    Attributes:
        line: The source line of the breakpoint.
        column: An optional source column of the breakpoint.
        condition: An optional expression for conditional breakpoints.
        hit_condition: An optional expression that controls how many times the breakpoint must be hit.
        log_message: If this attribute exists, the breakpoint behaves as a logpoint.
    """

    line: int
    column: int | None = None
    condition: str | None = None
    hit_condition: str | None = None
    log_message: str | None = None


class Breakpoint(DapBaseModel):
    """Information about a Breakpoint.

    Attributes:
        id: An optional unique identifier for the breakpoint.
        verified: If true, the breakpoint could be set (its location can be resolved).
        message: An optional message about the state of the breakpoint.
        source: The source where the breakpoint is located.
        line: The start line of the actual range covered by the breakpoint.
        column: An optional start column of the actual range covered by the breakpoint.
        end_line: An optional end line of the actual range covered by the breakpoint.
        end_column: An optional end column of the actual range covered by the breakpoint.
        instruction_reference: An optional memory reference to where the breakpoint is set.
        offset: An optional offset from the instruction reference.
    """

    id: int | None = None
    verified: bool
    message: str | None = None
    source: Source | None = None
    line: int | None = None
    column: int | None = None
    end_line: int | None = None
    end_column: int | None = None
    instruction_reference: str | None = None
    offset: int | None = None
