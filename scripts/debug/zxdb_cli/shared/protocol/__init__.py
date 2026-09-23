# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Annotated, Any

from pydantic import Field, TypeAdapter, create_model
from shared.protocol.base import (
    PROTOCOL_VERSION,
    BaseRequest,
    ProtocolRegistry,
    Response,
    T_Resp,
    deserialize_request,
    deserialize_response,
    get_schema,
    make_request,
    serialize,
)

# isort: split
from shared.protocol.async_backtrace import (
    AsyncBacktraceRequest,
    AsyncBacktraceResponse,
)
from shared.protocol.attach import AttachRequest
from shared.protocol.break_request import BreakRequest
from shared.protocol.continue_request import ContinueRequest
from shared.protocol.detach import DetachRequest
from shared.protocol.evaluate import EvaluateRequest, EvaluateResponse
from shared.protocol.finish import FinishRequest
from shared.protocol.get_state import (
    GetStateRequest,
    GetStateResponse,
    ThreadInfo,
)
from shared.protocol.hello import HelloRequest
from shared.protocol.next_request import NextRequest
from shared.protocol.pause import PauseRequest
from shared.protocol.stack_trace import (
    ProcessStackTraceResponse,
    StackFrame,
    StackTraceRequest,
    ThreadStackTraceResponse,
)
from shared.protocol.start import StartRequest
from shared.protocol.step_in import StepInRequest
from shared.protocol.stop import StopRequest
from shared.protocol.threads import ThreadsRequest
from shared.protocol.variables import VariablesRequest
from shared.protocol.wait_for_event import WaitForEventRequest

RequestType = Annotated[
    AsyncBacktraceRequest
    | AttachRequest
    | BreakRequest
    | ContinueRequest
    | DetachRequest
    | EvaluateRequest
    | FinishRequest
    | GetStateRequest
    | HelloRequest
    | NextRequest
    | PauseRequest
    | StackTraceRequest
    | StartRequest
    | StepInRequest
    | StopRequest
    | ThreadsRequest
    | VariablesRequest
    | WaitForEventRequest,
    Field(discriminator="command"),
]

ResponseType = (
    GetStateResponse
    | EvaluateResponse
    | ThreadStackTraceResponse
    | ProcessStackTraceResponse
    | AsyncBacktraceResponse
    | dict[str, Any]
    | None
)

__all__ = [
    "BaseRequest",
    "Response",
    "T_Resp",
    "ThreadInfo",
    "GetStateResponse",
    "ProcessStackTraceResponse",
    "ThreadStackTraceResponse",
    "StackFrame",
    "AsyncBacktraceResponse",
    "PROTOCOL_VERSION",
    "serialize",
    "make_request",
    "deserialize_request",
    "deserialize_response",
    "get_schema",
    "RequestType",
    "ResponseType",
    "AsyncBacktraceRequest",
    "AttachRequest",
    "BreakRequest",
    "ContinueRequest",
    "DetachRequest",
    "EvaluateRequest",
    "EvaluateResponse",
    "FinishRequest",
    "GetStateRequest",
    "HelloRequest",
    "NextRequest",
    "PauseRequest",
    "StackTraceRequest",
    "StartRequest",
    "StepInRequest",
    "StopRequest",
    "ThreadsRequest",
    "VariablesRequest",
    "WaitForEventRequest",
]

_ResponseSchema = create_model(
    "Response",
    __base__=Response,
    __doc__=Response.__doc__,
    body=(ResponseType, None),
)

_request_adapter = TypeAdapter(RequestType)
ProtocolRegistry.request_adapter = _request_adapter

_response_adapter = TypeAdapter(_ResponseSchema)
ProtocolRegistry.response_adapter = _response_adapter
