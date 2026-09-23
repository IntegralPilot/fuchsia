// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_math::SizeU;
use fidl_fuchsia_ui_pointer::EventPhase;
use {fidl_fuchsia_ui_composition as fland, fidl_fuchsia_ui_views as fviews};

// Internal event loop messages used by simplest-app-flatland.  The example communicates
// asynchronously with a number of (mostly Scenic) protocols; InternalMessage helps to centralize
// the app's responses in a single handler, and thus greatly simplifies control flow.
pub enum InternalMessage {
    CreateView(fviews::ViewCreationToken, fviews::ViewIdentityOnCreation),
    OnPresentError { error: fland::FlatlandError },
    OnNextFrameBegin,
    KeyboardEvent { trace_id: fuchsia_trace::Id, change_color: bool },
    Relayout { size: SizeU },
    TouchEvent { phase: EventPhase, trace_id: fuchsia_trace::Id },
    MouseEvent { trace_id: fuchsia_trace::Id, change_color: bool },
}
