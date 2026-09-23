// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_wlan_fullmac::{self as fidl_fullmac, WlanFullmacImpl_Request};
use futures::StreamExt;

// Wrapper type for WlanFullmacImpl_Request types without the responder.
#[derive(Clone, Debug, PartialEq)]
pub enum FullmacRequest {
    Query,
    QuerySecuritySupport,
    StartScan(fidl_fullmac::WlanFullmacImplStartScanRequest),
    Connect(fidl_fullmac::WlanFullmacImplConnectRequest),
    Reconnect(fidl_fullmac::WlanFullmacImplReconnectRequest),
    AuthResp(fidl_fullmac::WlanFullmacImplAuthRespRequest),
    Deauth(fidl_fullmac::WlanFullmacImplDeauthRequest),
    AssocResp(fidl_fullmac::WlanFullmacImplAssocRespRequest),
    Disassoc(fidl_fullmac::WlanFullmacImplDisassocRequest),
    StartBss(fidl_fullmac::WlanFullmacImplStartBssRequest),
    StopBss(fidl_fullmac::WlanFullmacImplStopBssRequest),
    SetKeys(fidl_fullmac::WlanFullmacImplSetKeysRequest),
    EapolTx(fidl_fullmac::WlanFullmacImplEapolTxRequest),
    GetIfaceStats,
    GetIfaceHistogramStats,
    SaeHandshakeResp(fidl_fullmac::WlanFullmacImplSaeHandshakeRespRequest),
    SaeFrameTx(fidl_fullmac::SaeFrame),
    WmmStatusReq,
    OnLinkStateChanged(fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest),

    // Note: WlanFullmacImpl::Start has a channel as an argument, but we don't keep the channel
    // here.
    Init,
}

/// A wrapper around WlanFullmacImpl_RequestStream that records each handled request in its
/// |history|. Users of this type should not access |request_stream| directly; instead, use
/// RecordedRequestStream::handle_request.
pub struct RecordedRequestStream {
    request_stream: fidl_fullmac::WlanFullmacImpl_RequestStream,
    history: Vec<FullmacRequest>,
}

impl RecordedRequestStream {
    pub fn new(request_stream: fidl_fullmac::WlanFullmacImpl_RequestStream) -> Self {
        Self { request_stream, history: Vec::new() }
    }

    pub fn history(&self) -> &[FullmacRequest] {
        &self.history[..]
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Retrieves a single request from the request stream.
    /// This records the request type in its history (copying the request payload out if one
    /// exists) before returning it.
    pub async fn next(&mut self) -> fidl_fullmac::WlanFullmacImpl_Request {
        let request = self
            .request_stream
            .next()
            .await
            .unwrap()
            .expect("Could not get next request in fullmac request stream");
        match &request {
            WlanFullmacImpl_Request::Query { .. } => self.history.push(FullmacRequest::Query),
            WlanFullmacImpl_Request::QuerySecuritySupport { .. } => {
                self.history.push(FullmacRequest::QuerySecuritySupport)
            }
            WlanFullmacImpl_Request::StartScan { payload, .. } => {
                self.history.push(FullmacRequest::StartScan(payload.clone()))
            }
            WlanFullmacImpl_Request::Connect { payload, .. } => {
                self.history.push(FullmacRequest::Connect(payload.clone()))
            }
            WlanFullmacImpl_Request::Reconnect { payload, .. } => {
                self.history.push(FullmacRequest::Reconnect(payload.clone()))
            }
            WlanFullmacImpl_Request::AuthResp { payload, .. } => {
                self.history.push(FullmacRequest::AuthResp(payload.clone()))
            }
            WlanFullmacImpl_Request::Deauth { payload, .. } => {
                self.history.push(FullmacRequest::Deauth(payload.clone()))
            }
            WlanFullmacImpl_Request::AssocResp { payload, .. } => {
                self.history.push(FullmacRequest::AssocResp(payload.clone()))
            }
            WlanFullmacImpl_Request::Disassoc { payload, .. } => {
                self.history.push(FullmacRequest::Disassoc(payload.clone()))
            }
            WlanFullmacImpl_Request::StartBss { payload, .. } => {
                self.history.push(FullmacRequest::StartBss(payload.clone()))
            }
            WlanFullmacImpl_Request::StopBss { payload, .. } => {
                self.history.push(FullmacRequest::StopBss(payload.clone()))
            }
            WlanFullmacImpl_Request::SetKeys { payload, .. } => {
                self.history.push(FullmacRequest::SetKeys(payload.clone()))
            }
            WlanFullmacImpl_Request::EapolTx { payload, .. } => {
                self.history.push(FullmacRequest::EapolTx(payload.clone()))
            }
            WlanFullmacImpl_Request::GetIfaceStats { .. } => {
                self.history.push(FullmacRequest::GetIfaceStats)
            }
            WlanFullmacImpl_Request::GetIfaceHistogramStats { .. } => {
                self.history.push(FullmacRequest::GetIfaceHistogramStats)
            }
            WlanFullmacImpl_Request::SaeHandshakeResp { payload, .. } => {
                self.history.push(FullmacRequest::SaeHandshakeResp(payload.clone()))
            }
            WlanFullmacImpl_Request::SaeFrameTx { frame, .. } => {
                self.history.push(FullmacRequest::SaeFrameTx(frame.clone()))
            }
            WlanFullmacImpl_Request::WmmStatusReq { .. } => {
                self.history.push(FullmacRequest::WmmStatusReq)
            }
            WlanFullmacImpl_Request::OnLinkStateChanged { payload, .. } => {
                self.history.push(FullmacRequest::OnLinkStateChanged(payload.clone()))
            }
            WlanFullmacImpl_Request::Init { .. } => self.history.push(FullmacRequest::Init),

            _ => panic!("Unrecognized Fullmac request {:?}", request),
        }
        request
    }
}
