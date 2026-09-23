// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"syscall/zx"
	"syscall/zx/fidl"

	"go.fuchsia.dev/fuchsia/src/lib/component"

	"fidl/fidl/clientsuite"
)

// classifyErr converts an error returned from the FIDL bindings into a FidlErrorKind.
func classifyErr(err error) clientsuite.FidlErrorKind {
	switch err := err.(type) {
	case *zx.Error:
		switch err.Status {
		case zx.ErrPeerClosed:
			return clientsuite.FidlErrorKindChannelPeerClosed
		case zx.ErrInvalidArgs:
			return clientsuite.FidlErrorKindDecodingError
		case zx.ErrNotSupported, zx.ErrNotFound:
			return clientsuite.FidlErrorKindUnexpectedMessage
		}
	case fidl.ValidationError:
		switch err.Code() {
		case fidl.ErrUnknownMagic, fidl.ErrUnexpectedOrdinal:
			return clientsuite.FidlErrorKindDecodingError
		}
	}
	return clientsuite.FidlErrorKindOtherError
}

type runnerImpl struct{}

var _ clientsuite.RunnerWithCtx = (*runnerImpl)(nil)

func (*runnerImpl) GetVersion(_ fidl.Context) (uint64, error) {
	return clientsuite.ClientSuiteVersion, nil
}

func (*runnerImpl) IsTestEnabled(_ fidl.Context, test clientsuite.Test) (bool, error) {
	isEnabled := func(test clientsuite.Test) bool {
		switch test {
		case clientsuite.TestReceiveEventNoPayload, clientsuite.TestReceiveEventStructPayload,
			clientsuite.TestReceiveEventTablePayload, clientsuite.TestReceiveEventUnionPayload,
			clientsuite.TestReceiveEventBadMagicNumber, clientsuite.TestReceiveEventUnexpectedTxid,
			clientsuite.TestReceiveEventUnknownOrdinal:
			// Go cannot receive events of arbitrary type.
			return false
		case clientsuite.TestOneWayStrictSend:
			return false
		case clientsuite.TestOneWayFlexibleSend:
			return false
		case clientsuite.TestTwoWayStrictSend:
			return false
		case clientsuite.TestTwoWayStrictSendMismatchedStrictness:
			return false
		case clientsuite.TestTwoWayStrictSendNonEmptyPayload:
			return false
		case clientsuite.TestTwoWayStrictErrorSyntaxSendSuccessResponse:
			return false
		case clientsuite.TestTwoWayStrictErrorSyntaxSendErrorResponse:
			return false
		case clientsuite.TestTwoWayStrictErrorSyntaxSendUnknownMethodResponse:
			return false
		case clientsuite.TestTwoWayStrictErrorSyntaxSendMismatchedStrictnessUnknownMethodResponse:
			return false
		case clientsuite.TestTwoWayStrictErrorSyntaxSendNonEmptyPayload:
			return false
		case clientsuite.TestTwoWayFlexibleSendSuccessResponse:
			return false
		case clientsuite.TestTwoWayFlexibleSendErrorResponse:
			return false
		case clientsuite.TestTwoWayFlexibleSendUnknownMethodResponse:
			return false
		case clientsuite.TestTwoWayFlexibleSendMismatchedStrictnessUnknownMethodResponse:
			return false
		case clientsuite.TestTwoWayFlexibleSendOtherFrameworkErrResponse:
			return false
		case clientsuite.TestTwoWayFlexibleSendNonEmptyPayloadSuccessResponse:
			return false
		case clientsuite.TestTwoWayFlexibleSendNonEmptyPayloadUnknownMethodResponse:
			return false
		case clientsuite.TestTwoWayFlexibleErrorSyntaxSendSuccessResponse:
			return false
		case clientsuite.TestTwoWayFlexibleErrorSyntaxSendErrorResponse:
			return false
		case clientsuite.TestTwoWayFlexibleErrorSyntaxSendUnknownMethodResponse:
			return false
		case clientsuite.TestTwoWayFlexibleErrorSyntaxSendMismatchedStrictnessUnknownMethodResponse:
			return false
		case clientsuite.TestTwoWayFlexibleErrorSyntaxSendOtherFrameworkErrResponse:
			return false
		case clientsuite.TestTwoWayFlexibleErrorSyntaxSendNonEmptyPayloadSuccessResponse:
			return false
		case clientsuite.TestTwoWayFlexibleErrorSyntaxSendNonEmptyPayloadUnknownMethodResponse:
			return false
		case clientsuite.TestReceiveStrictEvent:
			return false
		case clientsuite.TestReceiveStrictEventMismatchedStrictness:
			return false
		case clientsuite.TestReceiveFlexibleEvent:
			return false
		case clientsuite.TestReceiveFlexibleEventMismatchedStrictness:
			return false
		case clientsuite.TestUnknownStrictEventOpenProtocol:
			return false
		case clientsuite.TestUnknownFlexibleEventOpenProtocol:
			return false
		case clientsuite.TestUnknownStrictEventAjarProtocol:
			return false
		case clientsuite.TestUnknownFlexibleEventAjarProtocol:
			return false
		case clientsuite.TestUnknownStrictEventClosedProtocol:
			return false
		case clientsuite.TestUnknownFlexibleEventClosedProtocol:
			return false
		case clientsuite.TestUnknownStrictServerInitiatedTwoWay:
			return false
		case clientsuite.TestUnknownFlexibleServerInitiatedTwoWay:
			return false
		case clientsuite.TestV1TwoWayNoPayload, clientsuite.TestV1TwoWayStructPayload:
			// TODO(https://fxbug.dev/42182214): Go bindings should reject V1 wire format.
			return false
		case clientsuite.TestOneWayCallDoNotReportPeerClosed:
			// TODO(https://fxbug.dev/42064467): Peer-closed errors should be
			// hidden from one-way calls.
			return false
		default:
			return true
		}
	}
	return isEnabled(test), nil
}

func (*runnerImpl) CheckAlive(_ fidl.Context) error { return nil }

func (*runnerImpl) GetBindingsProperties(_ fidl.Context) (clientsuite.BindingsProperties, error) {
	var properties clientsuite.BindingsProperties
	properties.SetIoStyle(clientsuite.IoStyleSync)
	return properties, nil
}

func (*runnerImpl) CallTwoWayNoPayload(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface) (clientsuite.EmptyResultClassification, error) {
	err := target.TwoWayNoPayload(ctx)
	if err != nil {
		return clientsuite.EmptyResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.EmptyResultClassificationWithSuccess(clientsuite.Empty{}), nil
}

func (*runnerImpl) CallTwoWayStructPayload(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface) (clientsuite.NonEmptyResultClassification, error) {
	someField, err := target.TwoWayStructPayload(ctx)
	if err != nil {
		return clientsuite.NonEmptyResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.NonEmptyResultClassificationWithSuccess(clientsuite.NonEmptyPayload{SomeField: someField}), nil
}

func (*runnerImpl) CallTwoWayTablePayload(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface) (clientsuite.TableResultClassification, error) {
	tablePayload, err := target.TwoWayTablePayload(ctx)
	if err != nil {
		return clientsuite.TableResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.TableResultClassificationWithSuccess(tablePayload), nil
}

func (*runnerImpl) CallTwoWayUnionPayload(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface) (clientsuite.UnionResultClassification, error) {
	unionPayload, err := target.TwoWayUnionPayload(ctx)
	if err != nil {
		return clientsuite.UnionResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.UnionResultClassificationWithSuccess(unionPayload), nil
}

func (*runnerImpl) CallTwoWayStructPayloadErr(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface) (clientsuite.NonEmptyResultWithErrorClassification, error) {
	payload, err := target.TwoWayStructPayloadErr(ctx)
	if err != nil {
		return clientsuite.NonEmptyResultWithErrorClassificationWithFidlError(classifyErr(err)), nil
	}
	switch payload.Which() {
	case clientsuite.ClosedTargetTwoWayStructPayloadErrResultResponse:
		return clientsuite.NonEmptyResultWithErrorClassificationWithSuccess(payload.Response), nil
	case clientsuite.ClosedTargetTwoWayStructPayloadErrResultErr:
		return clientsuite.NonEmptyResultWithErrorClassificationWithApplicationError(payload.Err), nil
	default:
		return clientsuite.NonEmptyResultWithErrorClassification{}, errors.New("Unexpected payload variant")
	}
}

func (*runnerImpl) CallTwoWayStructRequest(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface, request clientsuite.NonEmptyPayload) (clientsuite.EmptyResultClassification, error) {
	err := target.TwoWayStructRequest(ctx, request.SomeField)
	if err != nil {
		return clientsuite.EmptyResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.EmptyResultClassificationWithSuccess(clientsuite.Empty{}), nil
}

func (*runnerImpl) CallTwoWayTableRequest(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface, request clientsuite.TablePayload) (clientsuite.EmptyResultClassification, error) {
	err := target.TwoWayTableRequest(ctx, request)
	if err != nil {
		return clientsuite.EmptyResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.EmptyResultClassificationWithSuccess(clientsuite.Empty{}), nil
}

func (*runnerImpl) CallTwoWayUnionRequest(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface, request clientsuite.UnionPayload) (clientsuite.EmptyResultClassification, error) {
	err := target.TwoWayUnionRequest(ctx, request)
	if err != nil {
		return clientsuite.EmptyResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.EmptyResultClassificationWithSuccess(clientsuite.Empty{}), nil
}

func (*runnerImpl) CallOneWayNoRequest(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface) (clientsuite.EmptyResultClassification, error) {
	err := target.OneWayNoRequest(ctx)
	if err != nil {
		return clientsuite.EmptyResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.EmptyResultClassificationWithSuccess(clientsuite.Empty{}), nil
}

func (*runnerImpl) CallOneWayStructRequest(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface, request clientsuite.NonEmptyPayload) (clientsuite.EmptyResultClassification, error) {
	err := target.OneWayStructRequest(ctx, request.SomeField)
	if err != nil {
		return clientsuite.EmptyResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.EmptyResultClassificationWithSuccess(clientsuite.Empty{}), nil
}

func (*runnerImpl) CallOneWayTableRequest(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface, request clientsuite.TablePayload) (clientsuite.EmptyResultClassification, error) {
	err := target.OneWayTableRequest(ctx, request)
	if err != nil {
		return clientsuite.EmptyResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.EmptyResultClassificationWithSuccess(clientsuite.Empty{}), nil
}

func (*runnerImpl) CallOneWayUnionRequest(ctx fidl.Context, target clientsuite.ClosedTargetWithCtxInterface, request clientsuite.UnionPayload) (clientsuite.EmptyResultClassification, error) {
	err := target.OneWayUnionRequest(ctx, request)
	if err != nil {
		return clientsuite.EmptyResultClassificationWithFidlError(classifyErr(err)), nil
	}
	return clientsuite.EmptyResultClassificationWithSuccess(clientsuite.Empty{}), nil
}

func (*runnerImpl) CallStrictOneWay(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.EmptyResultClassification, error) {
	return clientsuite.EmptyResultClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) CallFlexibleOneWay(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.EmptyResultClassification, error) {
	return clientsuite.EmptyResultClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) CallStrictTwoWay(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.EmptyResultClassification, error) {
	return clientsuite.EmptyResultClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) CallStrictTwoWayFields(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.NonEmptyResultClassification, error) {
	return clientsuite.NonEmptyResultClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) CallStrictTwoWayErr(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.EmptyResultWithErrorClassification, error) {
	return clientsuite.EmptyResultWithErrorClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) CallStrictTwoWayFieldsErr(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.NonEmptyResultWithErrorClassification, error) {
	return clientsuite.NonEmptyResultWithErrorClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) CallFlexibleTwoWay(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.EmptyResultClassification, error) {
	return clientsuite.EmptyResultClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) CallFlexibleTwoWayFields(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.NonEmptyResultClassification, error) {
	return clientsuite.NonEmptyResultClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) CallFlexibleTwoWayErr(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.EmptyResultWithErrorClassification, error) {
	return clientsuite.EmptyResultWithErrorClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) CallFlexibleTwoWayFieldsErr(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface) (clientsuite.NonEmptyResultWithErrorClassification, error) {
	return clientsuite.NonEmptyResultWithErrorClassification{}, errors.New("Go Bindings do not support Open protocols")
}
func (*runnerImpl) ReceiveClosedEvents(_ fidl.Context, target clientsuite.ClosedTargetWithCtxInterface, reporter clientsuite.ClosedTargetEventReporterWithCtxInterface) error {
	return errors.New("Go bindings event support is too restricted to support the dynsuite event client")
}
func (*runnerImpl) ReceiveAjarEvents(_ fidl.Context, target clientsuite.AjarTargetWithCtxInterface, reporter clientsuite.AjarTargetEventReporterWithCtxInterface) error {
	return errors.New("Go Bindings do not support Ajar protocols")
}
func (*runnerImpl) ReceiveOpenEvents(_ fidl.Context, target clientsuite.OpenTargetWithCtxInterface, reporter clientsuite.OpenTargetEventReporterWithCtxInterface) error {
	return errors.New("Go Bindings do not support Open protocols")
}

func main() {
	log.SetFlags(log.Lshortfile)

	log.Println("Go clientsuite server: starting")
	ctx := component.NewContextFromStartupInfo()
	ctx.OutgoingService.AddService(
		clientsuite.RunnerName,
		func(ctx context.Context, c zx.Channel) error {
			stub := clientsuite.RunnerWithCtxStub{
				Impl: &runnerImpl{},
			}
			go component.Serve(ctx, &stub, c, component.ServeOptions{
				OnError: func(err error) {
					// Panic because the test runner should never fail.
					panic(fmt.Sprintf("clientsuite.Runner errored: %s", err))
				},
			})
			return nil
		},
	)
	log.Println("Go clientsuite server: ready")
	ctx.BindStartupHandle(context.Background())
}
