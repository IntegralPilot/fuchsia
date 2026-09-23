// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

use core::fmt::{Debug, Formatter};

/// DNS-SD Service Name for TREL
pub const TREL_DNSSD_SERVICE_NAME: &str = "_trel._udp";

/// DNS-SD Service Name for TREL, with a dot at the end.
pub const TREL_DNSSD_SERVICE_NAME_WITH_DOT: &str = "_trel._udp.";

/// Iterator type for iterating over TREL peer table entries.
#[allow(missing_debug_implementations)]
pub struct TrelPeerIterator<'a, T: ?Sized> {
    ot_instance: &'a T,
    ot_iter: otTrelPeerIterator,
}

impl<T: ?Sized + Trel> Iterator for TrelPeerIterator<'_, T> {
    type Item = TrelPeer;
    fn next(&mut self) -> Option<Self::Item> {
        self.ot_instance.iter_next_trel_peer(&mut self.ot_iter)
    }
}

/// This structure represents a Thread TREL Peer.
///
/// Functional equivalent of [`otsys::otTrelPeer`](crate::otsys::otTrelPeer).
#[derive(Default, Clone)]
#[repr(transparent)]
pub struct TrelPeer(pub otTrelPeer);

impl_ot_castable!(TrelPeer, otTrelPeer);

impl Debug for TrelPeer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrelPeer")
            .field("ext_address", &self.ext_address())
            .field("ext_pan_id", &self.ext_pan_id())
            .field("sock_addr", &self.sock_address())
            .finish()
    }
}

impl TrelPeer {
    /// Returns the Extended MAC Address of TREL peer.
    pub fn ext_address(&self) -> ExtAddress {
        self.0.mExtAddress.into()
    }

    /// Returns the Extended PAN Identifier of TREL peer.
    pub fn ext_pan_id(&self) -> ExtendedPanId {
        self.0.mExtPanId.into()
    }

    /// Returns the IPv6 socket address of TREL peer.
    pub fn sock_address(&self) -> SockAddr {
        SockAddr::from_ot(self.0.mSockAddr)
    }
}

/// Methods from the [OpenThread TREL Module][1].
///
/// [1]: https://openthread.io/reference/group/api-trel
pub trait Trel {
    /// Enables or disables TREL operation.
    fn trel_set_enabled(&self, enabled: bool);

    /// Returns true if TREL is enabled.
    fn trel_is_enabled(&self) -> bool;

    /// Return all the TREL counters
    fn trel_get_counters(&self) -> Option<&TrelCounters>;

    /// Reset TREL counters
    fn trel_reset_counters(&self);

    /// Return the count of TREL peer
    fn trel_get_number_of_peers(&self) -> u16;

    /// Functional equivalent of [`otsys::otTrelInitPeerIterator`]
    /// (crate::otsys::otTrelInitPeerIterator).
    fn trel_init_peer_iterator(&self, iter: &mut otTrelPeerIterator);

    /// Get the trel peer iterator instance.
    fn trel_peer_get_iterator(&self) -> TrelPeerIterator<'_, Self> {
        let mut ot_iter = otTrelPeerIterator::default();
        self.trel_init_peer_iterator(&mut ot_iter);
        TrelPeerIterator { ot_instance: self, ot_iter }
    }

    /// Functional equivalent of
    /// [`otsys::otTrelGetNextPeer`](crate::otsys::otTrelGetNextPeer).
    fn iter_next_trel_peer(&self, ot_iter: &mut otTrelPeerIterator) -> Option<TrelPeer>;
}

impl<T: Trel + Boxable> Trel for ot::Box<T> {
    fn trel_set_enabled(&self, enabled: bool) {
        self.as_ref().trel_set_enabled(enabled);
    }

    fn trel_is_enabled(&self) -> bool {
        self.as_ref().trel_is_enabled()
    }

    fn trel_get_counters(&self) -> Option<&TrelCounters> {
        self.as_ref().trel_get_counters()
    }

    fn trel_reset_counters(&self) {
        self.as_ref().trel_reset_counters()
    }

    fn trel_get_number_of_peers(&self) -> u16 {
        self.as_ref().trel_get_number_of_peers()
    }

    fn trel_init_peer_iterator(&self, iter: &mut otTrelPeerIterator) {
        self.as_ref().trel_init_peer_iterator(iter)
    }

    fn iter_next_trel_peer(&self, ot_iter: &mut otTrelPeerIterator) -> Option<TrelPeer> {
        self.as_ref().iter_next_trel_peer(ot_iter)
    }
}

impl Trel for Instance {
    fn trel_set_enabled(&self, enabled: bool) {
        unsafe { otTrelSetEnabled(self.as_ot_ptr(), enabled) }
    }

    fn trel_is_enabled(&self) -> bool {
        unsafe { otTrelIsEnabled(self.as_ot_ptr()) }
    }

    fn trel_get_counters(&self) -> Option<&TrelCounters> {
        unsafe { TrelCounters::ref_from_ot_ptr(otTrelGetCounters(self.as_ot_ptr())) }
    }

    fn trel_reset_counters(&self) {
        unsafe { otTrelResetCounters(self.as_ot_ptr()) }
    }

    fn trel_get_number_of_peers(&self) -> u16 {
        unsafe { otTrelGetNumberOfPeers(self.as_ot_ptr()) }
    }

    fn trel_init_peer_iterator(&self, iter: &mut otTrelPeerIterator) {
        unsafe { otTrelInitPeerIterator(self.as_ot_ptr(), iter) }
    }

    fn iter_next_trel_peer(&self, ot_iter: &mut otTrelPeerIterator) -> Option<TrelPeer> {
        unsafe {
            let peer_ptr = otTrelGetNextPeer(self.as_ot_ptr(), ot_iter as *mut otTrelPeerIterator);

            peer_ptr.as_ref().map(|raw| TrelPeer(*raw))
        }
    }
}

/// Functional equivalent of [`otsys::otPlatTrelPeerInfo`](crate::otsys::otPlatTrelPeerInfo).
#[derive(Clone)]
#[repr(transparent)]
pub struct PlatTrelPeerInfo<'a>(
    otPlatTrelPeerInfo,
    PhantomData<*mut otMessage>,
    PhantomData<&'a ()>,
);
impl_ot_castable!(
    lifetime
    PlatTrelPeerInfo<'_>,
    otPlatTrelPeerInfo,
    PhantomData,
    PhantomData
);

impl std::fmt::Debug for PlatTrelPeerInfo<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatTrelPeerInfo")
            .field("removed", &self.is_removed())
            .field("txt", &self.txt_escaped())
            .field("sockaddr", &self.sockaddr())
            .finish()
    }
}

impl<'a> PlatTrelPeerInfo<'a> {
    /// Creates a new instance of `PlatTrelPeerInfo`.
    pub fn new(removed: bool, txt: &[u8], sockaddr: ot::SockAddr) -> PlatTrelPeerInfo<'_> {
        PlatTrelPeerInfo::from_ot(otPlatTrelPeerInfo {
            mRemoved: removed,
            mTxtData: txt.as_ptr(),
            mTxtLength: txt.len().try_into().unwrap(),
            mSockAddr: sockaddr.into_ot(),
        })
    }

    /// Returns true if this peer is being removed.
    pub fn is_removed(&self) -> bool {
        self.0.mRemoved
    }

    /// Returns the raw value of the TXT field.
    pub fn txt(&self) -> &'a [u8] {
        unsafe { core::slice::from_raw_parts(self.0.mTxtData, self.0.mTxtLength.into()) }
    }

    /// Returns the TXT field as an escaped ASCII string.
    pub fn txt_escaped(&self) -> String {
        self.txt()
            .iter()
            .map(Clone::clone)
            .flat_map(std::ascii::escape_default)
            .map(char::from)
            .collect::<String>()
    }

    /// Returns the SockAddr for this peer.
    pub fn sockaddr(&self) -> SockAddr {
        SockAddr::from_ot(self.0.mSockAddr)
    }
}

/// Platform methods from the [OpenThread TREL Module][1].
///
/// [1]: https://openthread.io/reference/group/plat-trel
pub trait PlatTrel {
    /// This function is a callback from platform to notify of a received TREL UDP packet.
    fn plat_trel_handle_received(&self, packet: &[u8], sock_addr: &ot::SockAddr);

    /// This is a callback function from platform layer to report a discovered TREL peer info.
    fn plat_trel_handle_discovered_peer_info(&self, peer_info: &PlatTrelPeerInfo<'_>);
}

impl<T: PlatTrel + Boxable> PlatTrel for ot::Box<T> {
    fn plat_trel_handle_received(&self, packet: &[u8], sock_addr: &ot::SockAddr) {
        self.as_ref().plat_trel_handle_received(packet, sock_addr);
    }

    fn plat_trel_handle_discovered_peer_info(&self, peer_info: &PlatTrelPeerInfo<'_>) {
        self.as_ref().plat_trel_handle_discovered_peer_info(peer_info);
    }
}

impl PlatTrel for Instance {
    fn plat_trel_handle_received(&self, packet: &[u8], sock_addr: &ot::SockAddr) {
        unsafe {
            otPlatTrelHandleReceived(
                self.as_ot_ptr(),
                // TODO(https://fxbug.dev/42175496): Make sure they won't actually mutate.
                packet.as_ptr() as *mut u8,
                packet.len().try_into().unwrap(),
                sock_addr.as_ot_ptr(),
            )
        }
    }

    fn plat_trel_handle_discovered_peer_info(&self, peer_info: &PlatTrelPeerInfo<'_>) {
        unsafe { otPlatTrelHandleDiscoveredPeerInfo(self.as_ot_ptr(), peer_info.as_ot_ptr()) }
    }
}
