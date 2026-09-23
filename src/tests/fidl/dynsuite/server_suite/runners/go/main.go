// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"errors"
	"fmt"
	"syscall/zx"
	"syscall/zx/fidl"

	"go.fuchsia.dev/fuchsia/src/lib/component"
	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"

	"fidl/fidl/serversuite"
	"fidl/fuchsia/logger"
	fidlzx "fidl/zx"
)

// Note: The Runner.Start() implementation also disables all tests that use ajar
// or open protocols, since the Go bindings only support closed protocols.
var disabledTests = map[serversuite.Test]struct{}{
	// This is for testing the test disabling functionality itself.
	serversuite.TestIgnoreDisabled: {},
	// TODO(https://fxbug.dev/42064467): Peer-closed errors should be hidden from one-way calls.
	serversuite.TestEventSendingDoNotReportPeerClosed: {},
	serversuite.TestReplySendingDoNotReportPeerClosed: {},
	// TODO(https://fxbug.dev/42083233): Should validate txid.
	serversuite.TestOneWayWithNonZeroTxid:       {},
	serversuite.TestTwoWayNoPayloadWithZeroTxid: {},
	// TODO(https://fxbug.dev/42083231): After calling CloseWithEpitaph, component.Serve
	// still tries to close the channel at the end, and returns ErrBadHandle.
	serversuite.TestServerSendsEpitaph: {},
}

func getTeardownReason(err error) serversuite.TeardownReason {
	if err == nil {
		// When the server completes without an error, it could be PEER_CLOSED
		// or a voluntary shutdown.
		return serversuite.TeardownReasonPeerClosed | serversuite.TeardownReasonVoluntaryShutdown
	}
	var zxError *zx.Error
	if errors.As(err, &zxError) {
		if zxError.Text == "zx.Channel.WriteEtc" {
			return serversuite.TeardownReasonWriteFailure
		}
		panic(fmt.Sprintf("getTeardownReason: missing a case for this zx.Error: %s", err))
	}
	// The component library pre-emptively checks for these errors before calling zx.Channel.WriteEtc.
	if errors.Is(err, component.ErrTooManyBytesInResponse) || errors.Is(err, component.ErrTooManyHandlesInResponse) {
		return serversuite.TeardownReasonWriteFailure
	}
	var fidlError fidl.ErrorCode
	if !errors.As(err, &fidlError) {
		panic(fmt.Sprintf("getTeardownReason: expected a FIDL error, got %T: %s", err, err))
	}
	switch fidlError {
	case fidl.ErrUnknownMagic, fidl.ErrUnsupportedWireFormatVersion:
		return serversuite.TeardownReasonIncompatibleFormat
	case fidl.ErrUnknownOrdinal:
		return serversuite.TeardownReasonUnexpectedMessage
	case fidl.ErrInvalidInlineType, fidl.ErrInvalidPointerType, fidl.ErrVectorTooLong,
		fidl.ErrStringTooLong, fidl.ErrUnexpectedOrdinal, fidl.ErrUnexpectedNullRef,
		fidl.ErrInvalidBoolValue, fidl.ErrNotEnoughHandles, fidl.ErrTooManyBytesInMessage,
		fidl.ErrTooManyHandles, fidl.ErrBadHandleEncoding, fidl.ErrNonZeroPadding,
		fidl.ErrBadRefEncoding, fidl.ErrBadInlineIndicatorEncoding, fidl.ErrStructIsNotPayload,
		fidl.ErrInvalidUnionTag, fidl.ErrInvalidXUnionTag, fidl.ErrInvalidBitsValue,
		fidl.ErrInvalidEnumValue, fidl.ErrPayloadTooSmall, fidl.ErrInvalidEmptyStruct,
		fidl.ErrInvalidNumBytesInEnvelope, fidl.ErrInvalidNumHandlesInEnvelope, fidl.ErrStringNotUTF8,
		fidl.ErrMissingRequiredHandleRights, fidl.ErrUnableToReduceHandleRights, fidl.ErrUnspecifiedHandleRights,
		fidl.ErrIncorrectHandleType, fidl.ErrUnspecifiedHandleType, fidl.ErrValueTypeHandles,
		fidl.ErrExceededMaxOutOfLineDepth, fidl.ErrInvalidInlineBitValueInEnvelope:
		// The Go bindings do not differentiate between encode and decode failures.
		return serversuite.TeardownReasonEncodeFailure | serversuite.TeardownReasonDecodeFailure
	case fidl.ErrDuplicateTxidReceived, fidl.ErrDuplicateTxidWaiting, fidl.ErrResponseWithoutRequest:
		panic(fmt.Sprintf("getTeardownReason: this error should not happen in a server: %s", fidlError))
	case fidl.ErrMissingMarshalerContext:
		panic(fmt.Sprintf("getTeardownReason: this error should not happen in tests: %s", fidlError))
	default:
		panic(fmt.Sprintf("getTeardownReason: missing a case for this error: %s", fidlError))
	}
}

type closedTargetImpl struct {
	runnerEventProxy serversuite.RunnerEventProxy
	tag              string
}

var _ serversuite.ClosedTargetWithCtx = (*closedTargetImpl)(nil)

func (t *closedTargetImpl) OneWayNoPayload(_ fidl.Context) error {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: OneWayNoPayload")
	t.runnerEventProxy.OnReceivedClosedTargetOneWayNoPayload()
	return nil
}

func (t *closedTargetImpl) TwoWayNoPayload(_ fidl.Context) error {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: TwoWayNoPayload")
	return nil
}

func (t *closedTargetImpl) TwoWayStructPayload(_ fidl.Context, v int8) (int8, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: TwoWayStructPayload")
	return v, nil
}

func (t *closedTargetImpl) TwoWayTablePayload(_ fidl.Context, request serversuite.ClosedTargetTwoWayTablePayloadRequest) (serversuite.ClosedTargetTwoWayTablePayloadResponse, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: TwoWayTablePayload")
	var response serversuite.ClosedTargetTwoWayTablePayloadResponse
	response.SetV(request.V)
	return response, nil
}

func (t *closedTargetImpl) TwoWayUnionPayload(_ fidl.Context, request serversuite.ClosedTargetTwoWayUnionPayloadRequest) (serversuite.ClosedTargetTwoWayUnionPayloadResponse, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: TwoWayUnionPayload")
	var response serversuite.ClosedTargetTwoWayUnionPayloadResponse
	response.SetV(request.V)
	return response, nil
}

func (t *closedTargetImpl) TwoWayResult(_ fidl.Context, request serversuite.ClosedTargetTwoWayResultRequest) (serversuite.ClosedTargetTwoWayResultResult, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: TwoWayResult")
	var result serversuite.ClosedTargetTwoWayResultResult
	switch request.Which() {
	case serversuite.ClosedTargetTwoWayResultRequestPayload:
		result.SetResponse(serversuite.ClosedTargetTwoWayResultResponse{
			Payload: request.Payload,
		})
	case serversuite.ClosedTargetTwoWayResultRequestError:
		result.SetErr(request.Error)
	}
	return result, nil
}

func (t *closedTargetImpl) GetHandleRights(_ fidl.Context, handle zx.Handle) (fidlzx.Rights, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: GetHandleRights")
	info, err := handle.GetInfoHandleBasic()
	if err != nil {
		return 0, err
	}
	return fidlzx.Rights(info.Rights), nil
}

func (t *closedTargetImpl) GetSignalableEventRights(_ fidl.Context, handle zx.Event) (fidlzx.Rights, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: GetSignalableEventRights")
	info, err := handle.Handle().GetInfoHandleBasic()
	if err != nil {
		return 0, err
	}
	return fidlzx.Rights(info.Rights), nil
}

func (t *closedTargetImpl) EchoAsTransferableSignalableEvent(_ fidl.Context, handle zx.Handle) (zx.Event, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: EchoAsTransferableSignalableEvent")
	return zx.Event(handle), nil
}

func (t *closedTargetImpl) ByteVectorSize(_ fidl.Context, v []uint8) (uint32, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: ByteVectorSize")
	return uint32(len(v)), nil
}

func (t *closedTargetImpl) HandleVectorSize(_ fidl.Context, v []zx.Event) (uint32, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: HandleVectorSize")
	return uint32(len(v)), nil
}

func (t *closedTargetImpl) CreateNByteVector(_ fidl.Context, n uint32) ([]uint8, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: CreateNByteVector")
	return make([]uint8, n), nil
}

func (t *closedTargetImpl) CreateNHandleVector(_ fidl.Context, n uint32) ([]zx.Event, error) {
	syslog.InfoTf(t.tag, "Handling ClosedTarget request: CreateNHandleVector")
	out := make([]zx.Event, n)
	for i := range out {
		var err error
		out[i], err = zx.NewEvent(0)
		if err != nil {
			return nil, err
		}
	}
	return out, nil
}

type runnerImpl struct {
	runnerEventProxy serversuite.RunnerEventProxy
	targetChannel    zx.Channel
	tag              string
}

var _ serversuite.RunnerWithCtx = (*runnerImpl)(nil)

func (r *runnerImpl) GetVersion(_ fidl.Context) (uint64, error) {
	syslog.InfoTf(r.tag, "Handling Runner request: GetVersion")
	return serversuite.ServerSuiteVersion, nil
}

func (r *runnerImpl) CheckAlive(_ fidl.Context) error {
	syslog.InfoTf(r.tag, "Handling Runner request: CheckAlive")
	return nil
}

func (r *runnerImpl) Start(_ fidl.Context, test serversuite.Test, target serversuite.AnyTarget) (serversuite.RunnerStartResult, error) {
	// Include the GoogleTest test name from the server suite harness.
	// This helps to clarify which logs belong to which test.
	r.tag = fmt.Sprintf("ServerTest_%d", test)
	syslog.InfoTf(r.tag, "Handling Runner request: Start")
	if zx.Handle(r.targetChannel) != zx.HandleInvalid {
		panic("must only call Start() once")
	}
	var result serversuite.RunnerStartResult
	protocolIsSupported := target.Which() == serversuite.AnyTargetClosed
	if _, ok := disabledTests[test]; ok || !protocolIsSupported {
		result.SetErr(serversuite.StartErrorTestDisabled)
		return result, nil
	}
	r.targetChannel = target.Closed.Channel
	go func() {
		stub := serversuite.ClosedTargetWithCtxStub{
			Impl: &closedTargetImpl{
				runnerEventProxy: r.runnerEventProxy,
			},
		}
		syslog.InfoTf(r.tag, "Serving ClosedTarget...")
		var errorExpectedUnderTest error
		component.Serve(context.Background(), &stub, r.targetChannel, component.ServeOptions{
			OnError: func(err error) {
				errorExpectedUnderTest = err
			},
		})
		syslog.InfoTf(r.tag, "Target completed: %v", errorExpectedUnderTest)
		reason := getTeardownReason(errorExpectedUnderTest)
		syslog.InfoTf(r.tag, "Sending OnTeardown event: %s", reason)
		r.runnerEventProxy.OnTeardown(reason)
	}()
	result.SetResponse(serversuite.RunnerStartResponse{})
	return result, nil
}

func (r *runnerImpl) ShutdownWithEpitaph(_ fidl.Context, epitaph int32) error {
	syslog.InfoTf(r.tag, "Handling Runner request: ShutdownWithEpitaph")
	if err := component.CloseWithEpitaph(r.targetChannel, zx.Status(epitaph)); err != nil {
		panic(err)
	}
	return nil
}

func (r *runnerImpl) SendOpenTargetStrictEvent(_ fidl.Context) error {
	syslog.InfoTf(r.tag, "Handling Runner request: SendOpenTargetStrictEvent")
	panic("Go only supports closed protocols")
}

func (r *runnerImpl) SendOpenTargetFlexibleEvent(_ fidl.Context) error {
	syslog.InfoTf(r.tag, "Handling Runner request: SendOpenTargetFlexibleEvent")
	panic("Go only supports closed protocols")
}

func main() {
	ctx := component.NewContextFromStartupInfo()
	{
		req, logSink, err := logger.NewLogSinkWithCtxInterfaceRequest()
		if err != nil {
			panic(fmt.Sprintf("failed to create syslog request: %s", err))
		}
		ctx.ConnectToEnvService(req)
		options := syslog.LogInitOptions{LogLevel: syslog.InfoLevel}
		options.LogSink = logSink
		options.MinSeverityForFileAndLineInfo = syslog.InfoLevel
		options.Tags = []string{"go"}
		l, err := syslog.NewLogger(options)
		if err != nil {
			panic(err)
		}
		syslog.SetDefaultLogger(l)
	}
	ctx.OutgoingService.AddService(
		serversuite.RunnerName,
		func(ctx context.Context, c zx.Channel) error {
			stub := serversuite.RunnerWithCtxStub{
				Impl: &runnerImpl{
					runnerEventProxy: serversuite.RunnerEventProxy(fidl.ChannelProxy{Channel: c}),
				},
			}
			syslog.Infof("Serving Runner...")
			go component.Serve(ctx, &stub, c, component.ServeOptions{
				OnError: func(err error) {
					panic(fmt.Sprintf("Runner failed: %s", err))
				},
			})
			return nil
		},
	)
	syslog.Infof("Go serversuite server: ready!")
	ctx.BindStartupHandle(context.Background())
}
