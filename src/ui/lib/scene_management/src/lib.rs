// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(not(feature = "dso"))]
pub(crate) use ::input_pipeline as lib;
#[cfg(feature = "dso")]
pub(crate) use ::input_pipeline_dso as lib;

mod display_metrics;
mod graphics_utils;
mod pointerinjector_config;
mod scene_manager;

pub use display_metrics::{DisplayMetrics, ViewingDistance};
pub use graphics_utils::{ScreenCoordinates, ScreenSize};
pub use pointerinjector_config::InjectorViewportSubscriber;
pub use scene_manager::{
    SceneManager, SceneManagerTrait, handle_pointer_injector_configuration_setup_request_stream,
};
