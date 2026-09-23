// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_avctp::{self as avctp, AvctpCommand};
use futures::Future;
use log::{debug, info, warn};
use packet_encoding::{Decodable, Encodable};
use std::sync::Arc;

use crate::packets::*;
use crate::peer_manager::TargetDelegate;
use crate::types::PeerError;

/// According to AVRCP v1.6.2 Section 6.10.3.1, Database Unaware Players shall always
/// return UIDcounter=0
const UID_COUNTER: u16 = 0x00;

#[derive(Debug)]
pub struct BrowseChannelHandler {
    target_delegate: Arc<TargetDelegate>,
}

impl BrowseChannelHandler {
    pub fn new(target_delegate: Arc<TargetDelegate>) -> Self {
        Self { target_delegate }
    }

    /// Given an AvctpCommand header and body:
    ///
    /// 1) Decodes the packet body to create an `BrowsePreamble`.
    /// 2) Extracts the PduId from the BrowsePreamble.
    ///
    /// Returns the decoded PduId and the packet body.
    pub fn decode_command(command: &AvctpCommand) -> Result<(PduId, Vec<u8>), Error> {
        let packet_body = command.body();
        if !command.header().is_type(&avctp::AvctpMessageType::Command) {
            // Invalid header type. Send back general reject.
            warn!("Received invalid AVCTP request{:?}", command.header());
            return Err(Error::InvalidMessage);
        }

        let packet = match BrowsePreamble::decode(packet_body) {
            Err(e) => {
                // There was an issue parsing the AVCTP Preamble.
                warn!("Invalid AVCTP Preamble: {:?}", e);
                return Err(Error::InvalidMessage);
            }
            Ok(p) => p,
        };

        let pdu_id = match PduId::try_from(packet.pdu_id) {
            Err(e) => {
                warn!("Received unsupported PduId {:?}: {:?}", packet.pdu_id, e);
                // There was an issue parsing the packet PDU ID.
                return Err(Error::InvalidParameter);
            }
            Ok(id) => id,
        };

        Ok((pdu_id, packet.body))
    }

    /// Handles the specific browse channel command specified by `pdu_id`.
    ///
    /// Returns an encoded response buffer to be sent.
    /// Upon error, returns a StatusCode indicating point of failure to send over the
    /// general reject.
    async fn handle_browse_command(
        pdu_id: PduId,
        parameters: Vec<u8>,
        target_delegate: Arc<TargetDelegate>,
    ) -> Result<Vec<u8>, StatusCode> {
        match pdu_id {
            PduId::GetFolderItems => {
                let get_folder_items_cmd = GetFolderItemsCommand::decode(&parameters)
                    .map_err(|_| StatusCode::InvalidParameter)?;
                debug!("Received GetFolderItems Command {:?}", get_folder_items_cmd);

                // TODO(fxbug.dev/332331774): currently, we only support
                // MediaPlayerList scope since none of the players are
                // browsable for folder items.
                if get_folder_items_cmd.scope() != Scope::MediaPlayerList {
                    return Err(StatusCode::InvalidScope);
                }

                // Get the media player items from the TargetDelegate.
                let folder_items = target_delegate
                    .send_get_media_player_items_command()
                    .await
                    .map_err(StatusCode::from)?;

                let media_player_items = folder_items.into_iter().map(Into::into).collect();
                let resp = GetFolderItemsResponse::new_success(UID_COUNTER, media_player_items);

                // Encode the result into the output buffer.
                let mut buf = vec![0; resp.encoded_len()];
                resp.encode(&mut buf[..]).map_err(|_| StatusCode::ParameterContentError)?;

                Ok(buf)
            }
            PduId::GetTotalNumberOfItems => {
                let get_total_items_cmd = GetTotalNumberOfItemsCommand::decode(&parameters)
                    .map_err(|_| StatusCode::InvalidParameter)?;
                debug!("Received GetTotalNumberOfItems Command {:?}", get_total_items_cmd);

                // TODO(b/343223304): currently, we only support
                // MediaPlayerList scope since none of the players are
                // browsable for folder items.
                if get_total_items_cmd.scope() != Scope::MediaPlayerList {
                    return Err(StatusCode::InvalidScope);
                }

                // Use an arbitrary uid_counter for creating the response. Don't support
                // multiple players, so this value is irrelevant.
                let uid_counter: u16 = 0x1234;
                let resp = GetTotalNumberOfItemsResponse::new(StatusCode::Success, uid_counter, 1);

                // Encode the result into the output buffer.
                let mut buf = vec![0; resp.encoded_len()];
                resp.encode(&mut buf[..]).map_err(|_| StatusCode::ParameterContentError)?;

                Ok(buf)
            }
            PduId::GetItemAttributes => {
                let get_item_attrs_cmd = GetItemAttributesCommand::decode(&parameters)
                    .map_err(|_| StatusCode::InvalidParameter)?;
                if get_item_attrs_cmd.scope() != Scope::NowPlaying {
                    // TODO(b/343223304): Return an error here since we don't
                    // support any browsable players and we only support
                    // now playing scope for this command.
                    return Err(StatusCode::InvalidScope);
                }
                if get_item_attrs_cmd.uid() == 0x0 {
                    // From AVRCP 1.6.2 section 6.10.4.3, UID of 0 is not allowed.
                    return Err(StatusCode::InvalidParameter);
                }
                let media_attrs = target_delegate
                    .send_get_media_attributes_command()
                    .await
                    .map_err(StatusCode::from)?;

                let resp = GetItemAttributesResponse::Success(media_attrs.into());

                // Encode the result into the output buffer.
                let mut buf = vec![0; resp.encoded_len()];
                resp.encode(&mut buf[..]).map_err(|_| StatusCode::ParameterContentError)?;

                Ok(buf)
            }
            _ => {
                info!("[Browse Channel] Command unimplemented: {:?}", pdu_id);
                // Per AVRCP 1.6, Section 6.15.3, the TG will send `InvalidParameter` for a PDU ID
                // that it did not understand.
                return Err(StatusCode::InvalidParameter);
            }
        }
    }

    /// Given an AvctpCommand, handle command:
    ///
    /// 1) Gets the PduId and packet body from `decode_command`.
    /// 2) Fulfills the command associated with `pdu_id`.
    ///
    /// In any error case (Packet can't be parsed, missing parameters, unsupported PDU, ...),
    /// sends a GeneralReject over the browse channel.
    pub fn handle_command(
        &self,
        command: AvctpCommand,
    ) -> impl Future<Output = Result<(), PeerError>> + use<> {
        let target_delegate = self.target_delegate.clone();

        async move {
            // Decode the provided `command` into a PduId and command parameters.
            let (pdu_id, parameters) = match BrowseChannelHandler::decode_command(&command) {
                Ok((id, packet)) => (id, packet),
                Err(e) => {
                    warn!("[Browse Channel] Received invalid command: {:?}", e);
                    return send_general_reject(command, StatusCode::InvalidCommand);
                }
            };

            // Serve the command based on pdu_id. Returns a buffer payload to send.
            // If an error occurs at any stage of handling the command, a general_reject will be sent.
            let payload = match BrowseChannelHandler::handle_browse_command(
                pdu_id,
                parameters,
                target_delegate,
            )
            .await
            {
                Ok(buf) => buf,
                Err(e) => {
                    warn!("[Browse Channel] Couldn't handle command: {:?}", e);
                    return send_general_reject(command, e);
                }
            };

            // Send the response back to the remote peer.
            let response_packet = BrowsePreamble::new(u8::from(&pdu_id), payload);
            let mut response_buf = vec![0; response_packet.encoded_len()];
            response_packet.encode(&mut response_buf[..]).expect("Encoding should work");

            command.send_response(&response_buf[..]).map_err(Into::into)
        }
    }

    /// Clears any state.
    ///
    /// For now, there is no state to be cleared.
    pub fn reset(&self) {}
}

/// Attempts to send a GeneralReject response with the provided `status_code`.
fn send_general_reject(command: AvctpCommand, status_code: StatusCode) -> Result<(), PeerError> {
    let reject_response = BrowsePreamble::general_reject(status_code);
    let mut buf = vec![0; reject_response.encoded_len()];
    reject_response.encode(&mut buf[..]).expect("unable to encode reject packet");
    command.send_response(&buf[..]).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use bt_channel_test_support::{Transport, create_test_channels};
    use futures::{StreamExt, TryStreamExt, pin_mut};
    use std::task::Poll;
    use test_case::test_case;

    use fidl_fuchsia_bluetooth_avrcp::{
        MediaAttributes, TargetHandlerMarker, TargetHandlerRequest,
    };
    use fuchsia_async as fasync;

    #[fuchsia::test]
    /// Tests handling an invalid browse channel PDU returns an error.
    async fn test_handle_browse_command_invalid_pdu() {
        let delegate = Arc::new(TargetDelegate::new());

        let unsupported_pdu = PduId::GetPlayStatus; // Not a browse channel PDU.
        let args = vec![0x02, 0x0, 0x0];
        let res =
            BrowseChannelHandler::handle_browse_command(unsupported_pdu, args, delegate).await;

        assert_eq!(res, Err(StatusCode::InvalidParameter));
    }

    #[fuchsia::test]
    /// Tests handling  a valid browse channel PDU with no TargetHandler set returns an error.
    async fn test_handle_browse_command_no_handler() {
        let delegate = Arc::new(TargetDelegate::new());

        let supported_pdu = PduId::GetFolderItems;
        let args = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00];
        let res = BrowseChannelHandler::handle_browse_command(supported_pdu, args, delegate).await;

        assert_eq!(res, Err(StatusCode::NoAvailablePlayers));
    }

    /// Builds and returns the browse channel handler. Also returns a local and remote AVCTP peer
    /// which can be used to send/receive AVCTP commands.
    fn setup_handler_with_remote_peer(
        transport: Transport,
    ) -> (BrowseChannelHandler, avctp::AvctpPeer, avctp::AvctpCommandStream, avctp::AvctpPeer) {
        let handler = BrowseChannelHandler::new(Arc::new(TargetDelegate::new()));

        let (client, server) = create_test_channels(transport);
        let (local, remote) = match transport {
            Transport::Socket => (client, server),
            Transport::Fidl => (server, client),
        };
        let local_peer = avctp::AvctpPeer::new(local);
        let local_stream = local_peer.take_command_stream();

        let remote_peer = avctp::AvctpPeer::new(remote);

        (handler, local_peer, local_stream, remote_peer)
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn handle_invalid_browse_command_sends_reject_but_no_error(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (handler, _local_peer, mut local_stream, remote_peer) =
            setup_handler_with_remote_peer(transport);

        let invalid_command = [0x00, 0x01, 0x02];
        let mut remote_response_stream =
            remote_peer.send_command(&invalid_command).expect("can send command");

        // `bt-avrcp` (e.g local) should receive the command.
        let received_command = exec
            .run_until_stalled(&mut local_stream.next())
            .expect("should be ready")
            .unwrap()
            .expect("should receive command");
        // Once handled, we expect the outgoing general reject. Even though it's an invalid command,
        // we don't expect to Error.
        let handle_fut = handler.handle_command(received_command);
        pin_mut!(handle_fut);
        assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Ready(Ok(_)));

        // The general reject should be received by the remote.
        let response = exec
            .run_until_stalled(&mut remote_response_stream.next())
            .expect("should be ready")
            .unwrap()
            .expect("should receive response");
        let payload = BrowsePreamble::decode(response.body()).expect("valid response");
        assert_eq!(payload.pdu_id, u8::from(&PduId::GeneralReject));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn handle_unimplemented_browse_channel_sends_reject_but_no_error(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (handler, _local_peer, mut local_stream, remote_peer) =
            setup_handler_with_remote_peer(transport);

        // Valid PduId
        let unsupported_command = [u8::from(&PduId::PlayItem), 0, 1, 55];
        let mut remote_response_stream =
            remote_peer.send_command(&unsupported_command).expect("can send command");

        // `bt-avrcp` (e.g local) should receive the command.
        let received_command = exec
            .run_until_stalled(&mut local_stream.next())
            .expect("should be ready")
            .unwrap()
            .expect("should receive command");
        // Unimplemented command is OK - general reject to let peer know but no Error.
        let handle_fut = handler.handle_command(received_command);
        pin_mut!(handle_fut);
        assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Ready(Ok(_)));

        // The general reject should be received by the remote.
        let response = exec
            .run_until_stalled(&mut remote_response_stream.next())
            .expect("should be ready")
            .unwrap()
            .expect("should receive response");
        let payload = BrowsePreamble::decode(response.body()).expect("valid response");
        assert_eq!(payload.pdu_id, u8::from(&PduId::GeneralReject));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn handle_get_total_number_of_items_command(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (handler, _local_peer, mut local_stream, remote_peer) =
            setup_handler_with_remote_peer(transport);

        // Remote peer sends us a GetTotalNumberOfItems command.
        let cmd = GetTotalNumberOfItemsCommand::new(Scope::MediaPlayerList);
        let mut cmd_buf = vec![0; cmd.encoded_len()];
        let _ = cmd.encode(&mut cmd_buf[..]).expect("valid command");
        let total_command = BrowsePreamble::new(u8::from(&PduId::GetTotalNumberOfItems), cmd_buf);
        let mut total_buf = vec![0; total_command.encoded_len()];
        total_command.encode(&mut total_buf[..]).expect("Encoding should work");
        let mut remote_response_stream =
            remote_peer.send_command(&total_buf).expect("can send command");

        // `bt-avrcp` (e.g local) should receive the command.
        let received_command = exec
            .run_until_stalled(&mut local_stream.next())
            .expect("should be ready")
            .unwrap()
            .expect("should receive command");
        // Should handle okay.
        let handle_fut = handler.handle_command(received_command);
        pin_mut!(handle_fut);
        assert_matches!(exec.run_until_stalled(&mut handle_fut), Poll::Ready(Ok(_)));

        // Response should be received by remote.
        let response = exec
            .run_until_stalled(&mut remote_response_stream.next())
            .expect("should be ready")
            .unwrap()
            .expect("should receive response");
        let payload = BrowsePreamble::decode(response.body()).expect("valid response");
        assert_eq!(payload.pdu_id, u8::from(&PduId::GetTotalNumberOfItems));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn handle_get_item_attribues_command(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (handler, _local_peer, mut local_stream, remote_peer) =
            setup_handler_with_remote_peer(transport);

        let (target_proxy, mut target_stream) =
            fidl::endpoints::create_proxy_and_stream::<TargetHandlerMarker>();
        handler.target_delegate.set_target_handler(target_proxy).expect("should not fail");
        let delegator_request_fut = target_stream.select_next_some();
        pin_mut!(delegator_request_fut);

        // Remote peer sends us a GetItemAttributes command.
        let cmd = GetItemAttributesCommand::from_now_playing_list(1, 1, vec![]);
        let mut cmd_buf = vec![0; cmd.encoded_len()];
        let _ = cmd.encode(&mut cmd_buf[..]).expect("valid command");
        let total_command = BrowsePreamble::new(u8::from(&PduId::GetItemAttributes), cmd_buf);
        let mut total_buf = vec![0; total_command.encoded_len()];
        total_command.encode(&mut total_buf[..]).expect("Encoding should work");
        let mut remote_response_stream =
            remote_peer.send_command(&total_buf).expect("can send command");

        // `bt-avrcp` (e.g local) should receive the command.
        let received_command = exec
            .run_until_stalled(&mut local_stream.try_next())
            .expect("should be ready")
            .unwrap()
            .expect("has valid command");
        exec.run_until_stalled(&mut delegator_request_fut).expect_pending("should be pending");

        let handle_fut = handler.handle_command(received_command);
        pin_mut!(handle_fut);
        exec.run_until_stalled(&mut handle_fut).expect_pending("should be pending");
        match exec.run_until_stalled(&mut delegator_request_fut).expect("should be ready") {
            Ok(TargetHandlerRequest::GetMediaAttributes { responder }) => {
                responder.send(Ok(&MediaAttributes::default())).expect("should be ok");
            }
            _ => panic!("unexpected stream state"),
        }

        // Should handle okay.
        exec.run_until_stalled(&mut handle_fut)
            .expect("should be ok")
            .expect("should have succeeded");

        // Response should be received by remote.
        let response = exec
            .run_until_stalled(&mut remote_response_stream.try_next())
            .expect("should be ready")
            .unwrap()
            .expect("has valid command");
        let payload = BrowsePreamble::decode(response.body()).expect("valid response");
        assert_eq!(payload.pdu_id, u8::from(&PduId::GetItemAttributes));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn handle_get_item_attribues_command_reject(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();

        let (handler, _local_peer, mut local_stream, remote_peer) =
            setup_handler_with_remote_peer(transport);

        let (target_proxy, mut target_stream) =
            fidl::endpoints::create_proxy_and_stream::<TargetHandlerMarker>();
        handler.target_delegate.set_target_handler(target_proxy).expect("should not fail");
        let delegator_request_fut = target_stream.select_next_some();
        pin_mut!(delegator_request_fut);

        // Send GetItemAttributes command with unsupported scope, which should be rejected.
        let cmd = GetItemAttributesCommand::new(Scope::MediaPlayerVirtualFilesystem, 1, 1, vec![]);
        let mut cmd_buf = vec![0; cmd.encoded_len()];
        let _ = cmd.encode(&mut cmd_buf[..]).expect("valid command");
        let total_command = BrowsePreamble::new(u8::from(&PduId::GetItemAttributes), cmd_buf);
        let mut total_buf = vec![0; total_command.encoded_len()];
        total_command.encode(&mut total_buf[..]).expect("Encoding should work");
        let mut remote_response_stream =
            remote_peer.send_command(&total_buf).expect("can send command");

        // `bt-avrcp` (e.g local) should receive the command.
        let received_command = exec
            .run_until_stalled(&mut local_stream.try_next())
            .expect("should be ready")
            .unwrap()
            .expect("has valid command");
        exec.run_until_stalled(&mut delegator_request_fut).expect_pending("should be pending");

        // Once handled, we expect the outgoing general reject. Even though it's an invalid command,
        // we don't expect to Error.
        let handle_fut = handler.handle_command(received_command);
        pin_mut!(handle_fut);
        let res = exec.run_until_stalled(&mut handle_fut).expect("should be pending");
        assert_matches!(res, Ok(_));

        // The general reject should be received by the remote.
        let response = exec
            .run_until_stalled(&mut remote_response_stream.try_next())
            .expect("should be ready")
            .unwrap()
            .expect("should receive response");
        let payload = BrowsePreamble::decode(response.body()).expect("valid response");
        assert_eq!(payload.pdu_id, u8::from(&PduId::GeneralReject));
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn peer_disconnects_while_handling_command_returns_error(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (handler, _local_peer, mut local_stream, remote_peer) =
            setup_handler_with_remote_peer(transport);

        // Remote peer sends us a GetTotalNumberOfItems command.
        let cmd = GetTotalNumberOfItemsCommand::new(Scope::MediaPlayerList);
        let mut cmd_buf = vec![0; cmd.encoded_len()];
        let _ = cmd.encode(&mut cmd_buf[..]).expect("valid command");
        let total_command = BrowsePreamble::new(u8::from(&PduId::GetTotalNumberOfItems), cmd_buf);
        let mut total_buf = vec![0; total_command.encoded_len()];
        total_command.encode(&mut total_buf[..]).expect("Encoding should work");
        let remote_response_stream =
            remote_peer.send_command(&total_buf).expect("can send command");

        // `bt-avrcp` (e.g local) should receive the command.
        let received_command = exec
            .run_until_stalled(&mut local_stream.next())
            .expect("should be ready")
            .unwrap()
            .expect("should receive command");

        // Before we get to handle it, the peer disconnects.
        drop(remote_peer);
        drop(remote_response_stream);

        // Yield to let background tasks run and see the closure.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Handling should return Error.
        let handle_fut = handler.handle_command(received_command);
        assert_matches!(exec.run_singlethreaded(handle_fut), Err(_));
    }
}
