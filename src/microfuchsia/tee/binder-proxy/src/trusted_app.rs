// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::{Arc, LazyLock};

use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::ITrustedApp::{ITrustedApp, BnTrustedApp};
use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::ITrustedAppSession::{ITrustedAppSession, BnTrustedAppSession};
use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::ParameterSet::ParameterSet;
use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::OpResult::OpResult;
use binder::{self, SpIBinder};
use fuchsia_component::client::connect_to_protocol_sync_at;

use crate::convert;

pub struct TrustedApp {
    fidl_ta: LazyLock<
        Arc<fidl_fuchsia_tee::ApplicationSynchronousProxy>,
        Box<dyn FnOnce() -> Arc<fidl_fuchsia_tee::ApplicationSynchronousProxy> + Send>,
    >,
}

impl binder::Interface for TrustedApp {}

impl ITrustedApp for TrustedApp {
    fn openSession(
        &self,
        params: &ParameterSet,
        op_result: &mut OpResult,
    ) -> Result<binder::Strong<dyn ITrustedAppSession>, binder::Status> {
        let fidl_params = convert::in_params_to_fidl(params);
        let (session_id, fidl_op_result) = self
            .fidl_ta
            .open_session2(fidl_params, zx::MonotonicInstant::INFINITE)
            .map_err(convert::error_to_binder_status)?;
        if fidl_op_result.return_code != Some(0) {
            return Err(binder::Status::new_exception(
                binder::ExceptionCode::TRANSACTION_FAILED,
                None,
            ));
        }
        *op_result = convert::op_result_to_aidl(fidl_op_result);
        Ok(Session::new_binder(self.fidl_ta.clone(), session_id))
    }
}

impl TrustedApp {
    fn new(uuid: &str) -> Self {
        let uuid = uuid.to_string();
        let fidl_ta = LazyLock::<_, Box<dyn FnOnce() -> Arc<_> + Send>>::new(Box::new(move || {
            let ta_path = "/ta/".to_owned() + &uuid;
            let connection =
                connect_to_protocol_sync_at::<fidl_fuchsia_tee::ApplicationMarker>(ta_path)
                    .expect("Connecting to TA {uuid}");
            Arc::new(connection)
        }));
        Self { fidl_ta }
    }

    pub fn new_binder(uuid: &str) -> SpIBinder {
        let trusted_app = TrustedApp::new(uuid);
        BnTrustedApp::new_binder(trusted_app, binder::BinderFeatures::default()).as_binder()
    }
}

pub struct Session {
    fidl_ta: Arc<fidl_fuchsia_tee::ApplicationSynchronousProxy>,
    session_id: u32,
}

impl binder::Interface for Session {}

impl ITrustedAppSession for Session {
    fn invokeCommand(
        &self,
        command_id: i32,
        params: &ParameterSet,
    ) -> Result<OpResult, binder::Status> {
        let fidl_params = convert::in_params_to_fidl(params);
        let fidl_op_result = self
            .fidl_ta
            .invoke_command(
                self.session_id,
                command_id as u32,
                fidl_params,
                zx::MonotonicInstant::INFINITE,
            )
            .map_err(convert::error_to_binder_status)?;
        Ok(convert::op_result_to_aidl(fidl_op_result))
    }
}

impl Session {
    fn new(fidl_ta: Arc<fidl_fuchsia_tee::ApplicationSynchronousProxy>, session_id: u32) -> Self {
        Self { fidl_ta, session_id }
    }

    pub fn new_binder(
        fidl_ta: Arc<fidl_fuchsia_tee::ApplicationSynchronousProxy>,
        session_id: u32,
    ) -> binder::Strong<dyn ITrustedAppSession> {
        let session = Session::new(fidl_ta, session_id);
        BnTrustedAppSession::new_binder(session, binder::BinderFeatures::default())
    }
}
