// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"bytes"
	"context"
	"syscall/zx"
	"syscall/zx/fidl"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/inspect"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/inspect/vmobuffer"
	"go.fuchsia.dev/fuchsia/src/lib/component"

	validate "fidl/diagnostics/validate/deprecated"
)

type impl struct {
	vmo       *zx.VMO
	writer    *inspect.Writer
	nodes     map[uint32]uint32
	published bool
}

const inspectName = "root.inspect"

var _ component.Directory = (*impl)(nil)

func (i *impl) Get(name string) (component.Node, bool) {
	if i.published && name == inspectName {
		return &component.FileWrapper{File: i}, true
	}
	return nil, false
}

func (i *impl) ForEach(fn func(string, component.Node) error) error {
	if i.published {
		return fn(inspectName, &component.FileWrapper{File: i})
	}
	return nil
}

var _ component.File = (*impl)(nil)

func (i *impl) GetReader() (component.Reader, uint64, error) {
	h, err := i.vmo.Handle().Duplicate(zx.RightSameRights)
	if err != nil {
		return nil, 0, err
	}
	vmo := zx.VMO(h)
	size, err := vmo.Size()
	if err != nil {
		vmo.Close()
		return nil, 0, err
	}
	return vmobuffer.NewVMOReader(vmo, size), size, nil
}

func (i *impl) Initialize(_ fidl.Context, params validate.InitializationParams) (zx.Handle, validate.TestResult, error) {
	if !params.HasVmoSize() {
		return zx.HandleInvalid, validate.TestResultIllegal, nil
	}
	size := params.GetVmoSize()
	{
		vmoBuffer, err := vmobuffer.NewVMOBuffer(size, "validator-VMOBuffer")
		if err != nil {
			panic(err)
		}
		i.vmo = vmoBuffer.GetVMO()
		w, err := inspect.NewWriter(vmoBuffer)
		if err != nil {
			panic(err)
		}
		i.writer = w
	}

	h, err := i.vmo.Handle().Duplicate(zx.RightSameRights)
	if err != nil {
		panic(err)
	}

	return h, validate.TestResultOk, nil
}

func (i *impl) GetConfig(fidl.Context) (string, validate.Options, error) {
	return "golang-puppet", validate.Options{}, nil
}

func (i *impl) Publish(fidl.Context) (validate.TestResult, error) {
	i.published = true
	return validate.TestResultOk, nil
}

func (i *impl) Act(ctx fidl.Context, action validate.Action) (validate.TestResult, error) {
	switch action.Which() {
	case validate.ActionCreateNode:
		action := action.CreateNode
		index, err := i.writer.WriteNodeValueBlock(i.nodes[action.Parent], action.Name)
		if err != nil {
			panic(err)
		}
		i.nodes[action.Id] = index
		return validate.TestResultOk, nil
	case validate.ActionDeleteNode:
	case validate.ActionCreateNumericProperty:
	case validate.ActionCreateBytesProperty:
		action := action.CreateBytesProperty
		var r bytes.Reader
		r.Reset(action.Value)
		if err := i.writer.WriteBinary(i.nodes[action.Parent], action.Name, uint32(r.Len()), &r); err != nil {
			panic(err)
		}
		// WriteBinary might write lots of nodes, which index do we store? We'll
		// need to figure it out when/if we implement deletion.
		i.nodes[action.Id] = 0
		return validate.TestResultOk, nil
	}
	return validate.TestResultUnimplemented, nil
}

func main() {
	componentCtx := component.NewContextFromStartupInfo()

	i := impl{
		nodes: make(map[uint32]uint32),
	}

	stub := validate.InspectPuppetWithCtxStub{Impl: &i}
	componentCtx.OutgoingService.AddService(
		validate.InspectPuppetName,
		func(ctx context.Context, c zx.Channel) error {
			go component.Serve(ctx, &stub, c, component.ServeOptions{
				OnError: func(err error) {
					panic(err)
				},
			})
			return nil
		},
	)

	componentCtx.BindStartupHandle(context.Background())
}
