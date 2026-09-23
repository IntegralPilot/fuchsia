// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::Stream;
use itertools::Itertools;
use nix::ifaddrs::{InterfaceAddress, getifaddrs};
use nix::net::if_::InterfaceFlags;
use nix::sys::socket::{SockaddrLike, SockaddrStorage};
use regex::Regex;
use std::cell::RefCell;
use std::ffi::CString;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex};
use std::task::{Context, Poll};
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

pub trait IsLocalAddr {
    /// is_local_addr returns true if the address is not globally routable.
    fn is_local_addr(&self) -> bool;

    /// is_link_local_addr returns true if the address is an IPv6 link local address.
    fn is_link_local_addr(&self) -> bool;
}

pub struct TokioAsyncWrapper<T: ?Sized>(Rc<RefCell<T>>);

impl<T> Clone for TokioAsyncWrapper<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> futures::io::AsyncRead for TokioAsyncWrapper<T>
where
    T: tokio::io::AsyncRead + Unpin + ?Sized,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<futures::io::Result<usize>> {
        let mut inner = self.0.borrow_mut();
        let mut buf = tokio::io::ReadBuf::new(buf);
        tokio::io::AsyncRead::poll_read(Pin::new(&mut *inner), cx, &mut buf)
            .map_ok(|_| buf.filled().len())
    }
}

impl<T> futures::io::AsyncWrite for TokioAsyncWrapper<T>
where
    T: tokio::io::AsyncWrite + Unpin + ?Sized,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<futures::io::Result<usize>> {
        let mut inner = self.0.borrow_mut();
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut *inner), cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<futures::io::Result<()>> {
        let mut inner = self.0.borrow_mut();
        tokio::io::AsyncWrite::poll_flush(Pin::new(&mut *inner), cx)
    }

    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<futures::io::Result<()>> {
        let mut inner = self.0.borrow_mut();
        tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut *inner), cx)
    }
}

pub trait TokioAsyncReadExt: tokio::io::AsyncRead + tokio::io::AsyncWrite + Sized {
    fn into_futures_stream(self) -> TokioAsyncWrapper<Self> {
        TokioAsyncWrapper(Rc::new(RefCell::new(self)))
    }

    fn into_multithreaded_futures_stream(self) -> MultithreadedTokioAsyncWrapper<Self>
    where
        Self: Send,
    {
        MultithreadedTokioAsyncWrapper(Arc::new(Mutex::new(self)))
    }
}

impl<T> TokioAsyncReadExt for T where T: tokio::io::AsyncRead + tokio::io::AsyncWrite {}

pub struct MultithreadedTokioAsyncWrapper<T: ?Sized>(Arc<Mutex<T>>);

impl<T> Clone for MultithreadedTokioAsyncWrapper<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> futures::io::AsyncRead for MultithreadedTokioAsyncWrapper<T>
where
    T: tokio::io::AsyncRead + Unpin + ?Sized,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<futures::io::Result<usize>> {
        let mut inner = self.0.lock().unwrap();
        let mut buf = tokio::io::ReadBuf::new(buf);
        tokio::io::AsyncRead::poll_read(Pin::new(&mut *inner), cx, &mut buf)
            .map_ok(|_| buf.filled().len())
    }
}

impl<T> futures::io::AsyncWrite for MultithreadedTokioAsyncWrapper<T>
where
    T: tokio::io::AsyncWrite + Unpin + ?Sized,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<futures::io::Result<usize>> {
        let mut inner = self.0.lock().unwrap();
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut *inner), cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<futures::io::Result<()>> {
        let mut inner = self.0.lock().unwrap();
        tokio::io::AsyncWrite::poll_flush(Pin::new(&mut *inner), cx)
    }

    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<futures::io::Result<()>> {
        let mut inner = self.0.lock().unwrap();
        tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut *inner), cx)
    }
}

pub struct UnixListenerStream(pub UnixListener);

impl Stream for UnixListenerStream {
    type Item = Result<UnixStream, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let listener = &mut this.0;

        match listener.poll_accept(cx) {
            Poll::Ready(value) => Poll::Ready(Some(value.map(|(stream, _)| stream))),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct TcpListenerRefStream<'a>(pub &'a mut TcpListener);

impl<'a> Stream for TcpListenerRefStream<'a> {
    type Item = Result<TcpStream, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let listener = &mut this.0;

        match listener.poll_accept(cx) {
            Poll::Ready(value) => Poll::Ready(Some(value.map(|(stream, _)| stream))),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct TcpListenerStream(pub TcpListener);

impl Stream for TcpListenerStream {
    type Item = Result<TcpStream, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let listener = &mut this.0;

        match listener.poll_accept(cx) {
            Poll::Ready(value) => Poll::Ready(Some(value.map(|(stream, _)| stream))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl IsLocalAddr for IpAddr {
    fn is_local_addr(&self) -> bool {
        match self {
            IpAddr::V4(ip) => ip.is_local_addr(),
            IpAddr::V6(ip) => ip.is_local_addr(),
        }
    }

    fn is_link_local_addr(&self) -> bool {
        match self {
            IpAddr::V4(ip) => ip.is_link_local_addr(),
            IpAddr::V6(ip) => ip.is_link_local_addr(),
        }
    }
}

impl IsLocalAddr for Ipv4Addr {
    fn is_local_addr(&self) -> bool {
        // TODO(https://fxbug.dev/42136483): add the various RFC reserved addresses and ranges too
        match self.octets() {
            [10, _, _, _] => true,
            [127, _, _, 1] => true,
            [172, 16..=31, _, _] => true,
            [192, 168, _, _] => true,
            [169, 254, 1..=254, _] => true,
            _ => false,
        }
    }

    fn is_link_local_addr(&self) -> bool {
        false
    }
}

impl IsLocalAddr for Ipv6Addr {
    fn is_local_addr(&self) -> bool {
        let segments = self.segments();

        // localhost
        if segments[..7].iter().all(|n| *n == 0) && segments[7] == 1 {
            return true;
        }

        // ULA
        if segments[0] & 0xfe00 == 0xfc00 {
            return true;
        }

        self.is_link_local_addr()
    }

    fn is_link_local_addr(&self) -> bool {
        let segments = self.segments();

        return segments[0] & 0xffff == 0xfe80
            && segments[1] & 0xffff == 0
            && segments[2] & 0xffff == 0
            && segments[3] & 0xffff == 0;
    }
}

/// Represents a SocketAddr with an optional string for the ScopeID.
///
/// The reason for the existence of this is that strings for scope ID's are generally more stable
/// than using scope ID's as integers, but `std::net::SocketAddr` is limited to representing them
/// as integers. This can be a problem because if a CDC ethernet device, for example, is unplugged
/// and then plugged back in, the scope ID as a string will remain the same for most setups, while
/// the scope ID as an integer will be incremented by one.
///
/// This can result in unstable connection recovery, as things that may deactivate a CDC Ethernet
/// connection (rebooting, adb, etc), will cause the scope ID to increment without changing the
/// string scope ID.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ScopedSocketAddr {
    addr: SocketAddr,
    scope_id: Option<String>,
}

impl ScopedSocketAddr {
    /// Attempts to construct a scoped socket addr by taking a socketaddr and
    /// converting its numeric scope ID into a string by looking it up.
    pub fn from_socket_addr(addr: SocketAddr) -> Result<Self, InvalidInterfaceIdError> {
        match &addr {
            SocketAddr::V6(a) => {
                // This should also apply to link-scope multicast, but this is only really being
                // used for `ssh`, so does not apply here.
                if addr.ip().is_link_local_addr() && a.scope_id() > 0 {
                    return Ok(Self {
                        addr,
                        scope_id: Some(scope_id_to_name_checked(a.scope_id())?),
                    });
                } else {
                    Ok(Self { addr, scope_id: None })
                }
            }
            _ => Ok(Self { addr, scope_id: None }),
        }
    }

    pub fn addr(&self) -> &SocketAddr {
        &self.addr
    }

    pub fn set_scope_id(&mut self, scope_id: u32) -> Result<(), NetExtError> {
        match &mut self.addr {
            SocketAddr::V6(inner) => {
                let scope_id_str = scope_id_to_name_checked(scope_id)?;
                inner.set_scope_id(scope_id);
                self.scope_id.replace(scope_id_str);
            }
            _ => (),
        }
        Ok(())
    }

    pub fn scope_id(&self) -> Option<&str> {
        self.scope_id.as_ref().map(|s| s.as_str())
    }

    pub fn scope_id_integer(&self) -> u32 {
        match &self.addr {
            SocketAddr::V6(inner) => inner.scope_id(),
            _ => 0,
        }
    }

    pub fn ip_string(&self) -> String {
        if self.addr.is_ipv6() && self.addr.ip().is_link_local_addr() {
            let sid = self.scope_id.clone().map_or_else(|| "".to_string(), |s| format!("%{}", s));
            format!("{}{}", self.addr.ip(), sid)
        } else {
            format!("{}", self.addr.ip())
        }
    }
}

impl Hash for ScopedSocketAddr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.addr.hash(state);
        self.scope_id_integer().hash(state);
    }
}

impl Display for ScopedSocketAddr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.addr.is_ipv6() && self.addr.ip().is_link_local_addr() {
            write!(f, "[{}", self.addr.ip())?;
            if let Some(scope) = &self.scope_id {
                write!(f, "%{}", scope)?;
            }
            write!(f, "]:{}", self.addr.port())?;
            Ok(())
        } else {
            write!(f, "{}", self.addr)
        }
    }
}

impl Deref for ScopedSocketAddr {
    type Target = SocketAddr;

    fn deref(&self) -> &Self::Target {
        &self.addr
    }
}

impl DerefMut for ScopedSocketAddr {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.addr
    }
}

/// An Mcast interface is:
/// -- Not a loopback.
/// -- Up (as opposed to down).
/// -- Has mcast enabled.
/// -- Has at least one non-globally routed address.
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct McastInterface {
    pub name: String,
    pub addrs: Vec<SocketAddr>,
}

impl McastInterface {
    pub fn id(&self) -> Result<u32, NetExtError> {
        Ok(nix::net::if_::if_nametoindex(self.name.as_str())?)
    }
}

fn is_local_multicast_addr(addr: &InterfaceAddress) -> bool {
    if !(addr.flags.contains(InterfaceFlags::IFF_UP)
        && addr.flags.contains(InterfaceFlags::IFF_MULTICAST)
        && !addr.flags.contains(InterfaceFlags::IFF_LOOPBACK))
    {
        return false;
    }
    ifaddr_to_socketaddr(addr.clone()).map(|address| address.ip().is_local_addr()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn is_not_apple_touchbar(addr: &InterfaceAddress) -> bool {
    // TOUCHBAR is the link-local IPv6 address used by the Apple Touchbar
    // interface on some MacBooks. This interface is always "up", declares
    // MULTICAST routable, and always configured with the same
    // link-local address.
    // Despite this, the interface never has a valid multicast route, and so
    // it is desirable to exclude it.
    const TOUCHBAR: IpAddr =
        IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0xaede, 0x48ff, 0xfe00, 0x1122));

    if let Some(address) = addr.address {
        let inet_addr = match address.as_sockaddr_in6() {
            Some(inet) => inet,
            _ => return true,
        };

        inet_addr.ip() != TOUCHBAR
    } else {
        true
    }
}

#[cfg(not(target_os = "macos"))]
fn is_not_apple_touchbar(_addr: &InterfaceAddress) -> bool {
    true
}

// Convert from nix socket storage type to standard socketaddr that we can use for safe comparison.
fn sockaddr_storage_to_socket_addr(address: SockaddrStorage) -> Option<std::net::SocketAddr> {
    match address.family() {
        Some(nix::sys::socket::AddressFamily::Inet) => {
            let sin4 = *address.as_sockaddr_in().unwrap();
            let v4: std::net::SocketAddrV4 = sin4.into();
            Some(v4.into())
        }
        Some(nix::sys::socket::AddressFamily::Inet6) => {
            let sin6 = *address.as_sockaddr_in6().unwrap();
            let v6: std::net::SocketAddrV6 = sin6.into();
            Some(v6.into())
        }
        _ => None,
    }
}

// ifaddr_to_socketaddr returns Some(std::net::SocketAddr) if ifaddr contains an inet addr, none otherwise.
fn ifaddr_to_socketaddr(ifaddr: InterfaceAddress) -> Option<std::net::SocketAddr> {
    ifaddr.address.and_then(sockaddr_storage_to_socket_addr)
}

/// scope_id_to_name attempts to convert a scope_id to an interface name, otherwise it returns the
/// scopeid formatted as a string.
pub fn scope_id_to_name(scope_id: u32) -> String {
    scope_id_to_name_checked(scope_id).unwrap_or_else(|_| scope_id.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidInterfaceIdError(pub u32);

impl std::fmt::Display for InvalidInterfaceIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is not a valid network interface ID", self.0)
    }
}

impl std::error::Error for InvalidInterfaceIdError {}

#[derive(Debug, Error)]
pub enum NetExtError {
    #[error("Invalid interface ID: {0}")]
    InvalidInterfaceId(#[from] InvalidInterfaceIdError),

    #[error("Could not parse '{0}'. Invalid address")]
    ParseAddress(String),

    #[error("'{0}' is not a valid network interface name.")]
    InvalidInterfaceName(String),

    #[error("Failed to get all interface addresses: {0}")]
    GetIfAddrs(#[from] nix::Error),

    #[error("CString error: {0}")]
    CString(#[from] std::ffi::NulError),
}

pub fn scope_id_to_name_checked(scope_id: u32) -> Result<String, InvalidInterfaceIdError> {
    let mut buf = vec![0; libc::IF_NAMESIZE];
    let res = unsafe { libc::if_indextoname(scope_id, buf.as_mut_ptr() as *mut libc::c_char) };
    if res.is_null() {
        return Err(InvalidInterfaceIdError(scope_id));
    } else {
        Ok(String::from_utf8_lossy(&buf.split(|&c| c == 0u8).next().unwrap_or(&[0u8])).to_string())
    }
}

/// Attempts to look up a scope_id's index. If an index could not be found, or the
/// string `name` is not a compatible CString (containing an interior null byte),
/// will return 0.
pub fn name_to_scope_id(name: &str) -> u32 {
    name_to_scope_id_checked(name).unwrap_or(0)
}

fn name_to_scope_id_checked(name: &str) -> Result<u32, NetExtError> {
    let s = CString::new(name)?;
    let idx = unsafe { libc::if_nametoindex(s.as_ptr()) };
    if idx == 0 { Err(NetExtError::InvalidInterfaceName(name.to_string())) } else { Ok(idx) }
}

/// Takes a string and attempts to parse it into the relevant parts of an address.
///
/// Examples:
///
/// example with a scoped link local address:
/// ```rust
/// let (addr, scope, port) = parse_address_parts("fe80::1%eno1").unwrap();
/// assert_eq!(addr, "fe80::1".parse::<IpAddr>().unwrap());
/// assert_eq!(scope, Some("eno1"));
/// assert_eq!(port, None);
/// ```
///
/// example with a scoped link local address and port:
/// ```rust
/// let (addr, scope, port) = parse_address_parts("[fe80::1%eno1]:1234").unwrap();
/// assert_eq!(addr, "fe80::1".parse::<IpAddr>().unwrap());
/// assert_eq!(scope, Some("eno1"));
/// assert_eq!(port, Some(1234));
/// ```
///
/// Works with both IPv6 and IPv4 addresses.
///
/// Returns:
///
/// `Ok(_)` if the address is in a valid format. [anyhow::Error] otherwise.
///
/// Returned values should not be considered correct and should be verified. For example,
/// `"[::1%foobar]:9898"` would parse, but there is no scope for the loopback device, and
/// furthermore "foobar" may not even exist as a scope, and should be verified.
///
/// The returned scope could also be a stringified integer, and should be verified.
pub fn parse_address_parts(
    addr_str: &str,
) -> Result<(IpAddr, Option<&str>, Option<u16>), NetExtError> {
    static V6_BRACKET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\[([^\]]+?[:]{1,2}[^\]]+)\](:\d+)?$").unwrap());
    static V4_PORT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d+\.\d+\.\d+\.\d+)(:\d+)?$").unwrap());
    static WITH_SCOPE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([^%]+)%([^%/ ]+)$").unwrap());

    let (addr, port) =
        if let Some(caps) = V6_BRACKET.captures(addr_str).or_else(|| V4_PORT.captures(addr_str)) {
            (caps.get(1).map(|x| x.as_str()).unwrap(), caps.get(2).map(|x| x.as_str()))
        } else {
            (addr_str, None)
        };

    let port = if let Some(port) = port { port[1..].parse::<u16>().ok() } else { None };

    let (addr, scope) = if let Some(caps) = WITH_SCOPE.captures(addr) {
        (caps.get(1).map(|x| x.as_str()).unwrap(), Some(caps.get(2).map(|x| x.as_str()).unwrap()))
    } else {
        (addr, None)
    };

    let addr = addr.parse::<IpAddr>().map_err(|_| NetExtError::ParseAddress(addr.to_string()))?;
    // Successfully parsing the address is the most important part. If this doesn't work,
    // then everything else is no longer valid.
    Ok((addr, scope, port))
}

/// Takes a string purporting to be a scope ID, then verifies that it exists on the system.
///
/// Examples:
///
/// ```rust
/// // If `en0` is a real interface on the system with the scope ID being 1.
/// assert_eq!(get_verified_scope_id("en0").unwrap(), 1u32);
///
/// // If `foober` isn't a real interface on the system.
/// assert!(get_verified_scope_id("foober").is_err());
///
/// // If "1" is the scope ID of, say, `en0`.
/// assert_eq!(get_verified_scope_id("1").unwrap(), 1u32);
///
/// // If "25" is not a scope ID of any interface.
/// assert!(get_verified_scope_id("25").is_err())
/// ```
pub fn get_verified_scope_id(scope: &str) -> Result<u32, NetExtError> {
    let s = match scope.parse::<u32>() {
        Ok(i) => scope_id_to_name_checked(i)?,
        Err(_e) => scope.to_owned(),
    };
    name_to_scope_id_checked(s.as_str())
}

// select_mcast_interfaces iterates over a set of InterfaceAddresses,
// selecting only those that meet the McastInterface criteria (see
// McastInterface), and returns them in a McastInterface representation.
fn select_mcast_interfaces(
    iter: &mut dyn Iterator<Item = InterfaceAddress>,
) -> Vec<McastInterface> {
    iter.filter(is_local_multicast_addr)
        .filter(is_not_apple_touchbar)
        .sorted_by_key(|ifaddr| ifaddr.interface_name.to_string())
        .chunk_by(|ifaddr| ifaddr.interface_name.to_string())
        .into_iter()
        .map(|(name, ifaddrs)| McastInterface {
            name: name,
            addrs: ifaddrs.filter_map(ifaddr_to_socketaddr).collect(),
        })
        .collect()
}

/// get_mcast_interfaces retrieves all local interfaces that are local
/// multicast enabled. See McastInterface for more detials.
// TODO(https://fxbug.dev/42121315): This needs to be e2e tested.
pub fn get_mcast_interfaces() -> Result<Vec<McastInterface>, NetExtError> {
    Ok(select_mcast_interfaces(&mut getifaddrs()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn sockaddr(s: &str) -> SockaddrStorage {
        std::net::SocketAddr::from_str(s).unwrap().into()
    }

    #[test]
    fn test_scope_id_to_name_known_interface() {
        let mut ifaddrs = getifaddrs().unwrap();
        let addr = ifaddrs.next().unwrap();
        let index = nix::net::if_::if_nametoindex(addr.interface_name.as_str()).unwrap();
        assert_eq!(scope_id_to_name(index), addr.interface_name.to_string());
    }

    #[test]
    fn test_verified_scope_id_by_name() {
        let mut ifaddrs = getifaddrs().unwrap();
        let addr = ifaddrs.next().unwrap();
        let index = name_to_scope_id_checked(addr.interface_name.as_str()).unwrap();
        assert_eq!(get_verified_scope_id(addr.interface_name.as_str()).unwrap(), index);
    }

    #[test]
    fn test_verified_scope_id_nonsense_name() {
        assert!(get_verified_scope_id("foihaofhoaw").is_err());
    }

    #[test]
    fn test_verified_scope_id_unused_scope_id() {
        let ifaddrs = getifaddrs().unwrap();
        let mut used_indices = ifaddrs
            .map(|addr| name_to_scope_id(addr.interface_name.as_str()))
            .collect::<Vec<u32>>();
        used_indices.sort();
        let unused_index = used_indices.as_slice().last().unwrap() + 1;
        assert!(get_verified_scope_id(format!("{}", unused_index).as_str()).is_err());
    }

    #[test]
    fn test_scope_id_to_name_unknown_interface() {
        let ifaddrs = getifaddrs().unwrap();
        let mut used_indices = ifaddrs
            .map(|addr| nix::net::if_::if_nametoindex(addr.interface_name.as_str()).unwrap_or(0))
            .collect::<Vec<u32>>();
        used_indices.sort();
        let unused_index = used_indices[used_indices.len() - 1] + 1;
        assert_eq!(scope_id_to_name(unused_index), format!("{}", unused_index));
    }

    #[test]
    fn test_local_interfaces_and_ids() {
        // This is an integration test. It may fail on a host system that has no
        // interfaces.
        let interfaces = get_mcast_interfaces().unwrap();
        assert!(interfaces.len() >= 1);
        for iface in &interfaces {
            // Note: could race if the host system is reconfigured in the
            // between the interface gathering above and this call, which is
            // unlikely.
            iface.id().unwrap();
        }

        // Assert that we find each interface and address from a raw getifaddrs call in the set of returned interfaces.
        for exiface in getifaddrs().unwrap() {
            if !is_local_multicast_addr(&exiface) {
                continue;
            }
            if !is_not_apple_touchbar(&exiface) {
                continue;
            }
            assert!(interfaces.iter().find(|iface| iface.name == exiface.interface_name).is_some());
            if let Some(exaddr) = exiface.address {
                assert!(
                    interfaces
                        .iter()
                        .find(|iface| {
                            iface
                                .addrs
                                .iter()
                                .find(|addr| {
                                    **addr == sockaddr_storage_to_socket_addr(exaddr).unwrap()
                                })
                                .is_some()
                        })
                        .is_some()
                );
            }
        }
    }

    #[test]
    fn test_select_mcast_interfaces() {
        let multicast_interface = InterfaceAddress {
            interface_name: "test-interface".to_string(),
            flags: InterfaceFlags::IFF_UP | InterfaceFlags::IFF_MULTICAST,
            address: Some(sockaddr("192.168.0.1:1234")),
            netmask: Some(sockaddr("255.255.255.0:0")),
            broadcast: None,
            destination: None,
        };

        let mut down_interface = multicast_interface.clone();
        down_interface.interface_name = "down-interface".to_string();
        down_interface.flags.remove(InterfaceFlags::IFF_UP);

        let mut mult_disabled = multicast_interface.clone();
        mult_disabled.interface_name = "no_multi-interface".to_string();
        mult_disabled.flags.remove(InterfaceFlags::IFF_MULTICAST);

        let mut no_addr = multicast_interface.clone();
        no_addr.interface_name = "no_addr-interface".to_string();
        no_addr.address = None;

        let mut mult2 = multicast_interface.clone();
        mult2.interface_name = "test-interface2".to_string();

        let mut addr2 = multicast_interface.clone();
        addr2.address = Some(sockaddr("192.168.0.2:1234"));

        let interfaces =
            vec![multicast_interface, mult2, addr2, down_interface, mult_disabled, no_addr];

        let result = select_mcast_interfaces(&mut interfaces.into_iter());
        assert_eq!(2, result.len());

        let ti = result.iter().find(|mcast| mcast.name == "test-interface");
        assert!(ti.is_some());
        assert!(result.iter().find(|mcast| mcast.name == "test-interface2").is_some());

        let ti_addrs =
            ti.unwrap().addrs.iter().map(|addr| addr.to_string()).sorted().collect::<Vec<String>>();
        assert_eq!(ti_addrs, ["192.168.0.1:1234", "192.168.0.2:1234"]);
    }

    #[test]
    fn test_is_local_multicast_addr() {
        let multicast_interface = InterfaceAddress {
            interface_name: "test-interface".to_string(),
            flags: InterfaceFlags::IFF_UP | InterfaceFlags::IFF_MULTICAST,
            address: Some(sockaddr("192.168.0.1:1234")),
            netmask: Some(sockaddr("255.255.255.0:0")),
            broadcast: None,
            destination: None,
        };

        assert!(is_local_multicast_addr(&multicast_interface));

        let mut down_interface = multicast_interface.clone();
        down_interface.flags.remove(InterfaceFlags::IFF_UP);
        assert!(!is_local_multicast_addr(&down_interface));

        let mut mult_disabled = multicast_interface.clone();
        mult_disabled.flags.remove(InterfaceFlags::IFF_MULTICAST);
        assert!(!is_local_multicast_addr(&mult_disabled));

        let mut no_addr = multicast_interface.clone();
        no_addr.address = None;
        assert!(!is_local_multicast_addr(&no_addr));
    }

    #[test]
    fn test_is_local_addr() {
        let local_addresses = vec![
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 1, 6, 7, 8)),
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)),
        ];
        let not_local_addresses = vec![
            IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            IpAddr::V6(Ipv6Addr::new(0x2607, 0xf8b0, 0x4005, 0x805, 0, 0, 0, 0x200e)),
        ];

        for addr in local_addresses {
            assert!(&addr.is_local_addr());
        }
        for addr in not_local_addresses {
            assert!(!&addr.is_local_addr());
        }
    }

    #[test]
    fn test_is_link_local_addr() {
        let link_local_addresses = vec![IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 1, 6, 7, 8))];
        let not_link_local_addresses = vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            IpAddr::V6(Ipv6Addr::new(0x2607, 0xf8b0, 0x4005, 0x805, 0, 0, 0, 0x200e)),
        ];

        for addr in link_local_addresses {
            assert!(&addr.is_link_local_addr());
        }
        for addr in not_link_local_addresses {
            assert!(!&addr.is_link_local_addr());
        }
    }

    #[test]
    fn test_is_local_v4() {
        let local_addresses = vec![
            Ipv4Addr::new(192, 168, 0, 1),
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(172, 16, 0, 1),
        ];

        let not_local_addresses = vec![
            Ipv4Addr::new(8, 8, 8, 8),
            Ipv4Addr::new(4, 4, 4, 4),
            Ipv4Addr::new(1, 1, 1, 1),
            Ipv4Addr::new(224, 1, 1, 1),
        ];

        for addr in local_addresses {
            assert!(&addr.is_local_addr());
        }
        for addr in not_local_addresses {
            assert!(!&addr.is_local_addr());
        }
    }

    #[test]
    fn test_is_local_v6() {
        let local_addresses = vec![
            Ipv6Addr::new(0xfe80, 0, 0, 0, 1, 6, 7, 8),
            Ipv6Addr::new(0xfc07, 0, 0, 0, 1, 6, 7, 8),
        ];

        let not_local_addresses = vec![
            Ipv6Addr::new(0xfe81, 0, 0, 0, 1, 6, 7, 8),
            Ipv6Addr::new(0xfe79, 0, 0, 0, 1, 6, 7, 8),
            Ipv6Addr::new(0x2607, 0xf8b0, 0x4005, 0x805, 0, 0, 0, 0x200e),
        ];

        for addr in local_addresses {
            assert!(&addr.is_local_addr());
        }
        for addr in not_local_addresses {
            assert!(!&addr.is_local_addr());
        }
    }

    #[test]
    fn test_is_not_apple_touchbar() {
        let not_touchbar = InterfaceAddress {
            interface_name: "not-touchbar".to_string(),
            flags: InterfaceFlags::IFF_UP | InterfaceFlags::IFF_MULTICAST,
            address: Some(sockaddr("[fe80::2]:1234")),
            netmask: Some(sockaddr("255.255.255.0:0")),
            broadcast: None,
            destination: None,
        };

        let touchbar = InterfaceAddress {
            interface_name: "touchbar".to_string(),
            flags: InterfaceFlags::IFF_UP | InterfaceFlags::IFF_MULTICAST,
            address: Some(sockaddr("[fe80::aede:48ff:fe00:1122]:1234")),
            netmask: Some(sockaddr("255.255.255.0:0")),
            broadcast: None,
            destination: None,
        };

        assert!(is_not_apple_touchbar(&not_touchbar));

        #[cfg(target_os = "macos")]
        assert!(!is_not_apple_touchbar(&touchbar));
        #[cfg(not(target_os = "macos"))]
        assert!(is_not_apple_touchbar(&touchbar));
    }

    #[test]
    fn test_parse_address_parts_scoped_no_port() {
        let (addr, scope, port) = parse_address_parts("fe80::1%eno1").unwrap();
        assert_eq!(addr, "fe80::1".parse::<IpAddr>().unwrap());
        assert_eq!(scope, Some("eno1"));
        assert_eq!(port, None);
    }

    #[test]
    fn test_parse_address_parts_scoped_no_port_odd_characters() {
        let (addr, scope, port) = parse_address_parts("fe80::1%zx-eno1").unwrap();
        assert_eq!(addr, "fe80::1".parse::<IpAddr>().unwrap());
        assert_eq!(scope, Some("zx-eno1"));
        assert_eq!(port, None);
    }

    #[test]
    fn test_parse_address_parts_scoped_spaces() {
        assert!(parse_address_parts("fe80::1%zx eno1").is_err());
    }

    #[test]
    fn test_parse_address_parts_scoped_with_port() {
        let (addr, scope, port) = parse_address_parts("[fe80::1%eno1]:1234").unwrap();
        assert_eq!(addr, "fe80::1".parse::<IpAddr>().unwrap());
        assert_eq!(scope, Some("eno1"));
        assert_eq!(port, Some(1234));
    }

    #[test]
    fn test_parse_address_parts_ipv6_addr_only() {
        let (addr, scope, port) = parse_address_parts("fe80::1").unwrap();
        assert_eq!(addr, "fe80::1".parse::<IpAddr>().unwrap());
        assert_eq!(scope, None);
        assert_eq!(port, None);
    }

    #[test]
    fn test_parse_address_parts_ipv4_with_port() {
        let (addr, scope, port) = parse_address_parts("192.168.1.2:1234").unwrap();
        assert_eq!(addr, "192.168.1.2".parse::<IpAddr>().unwrap());
        assert_eq!(scope, None);
        assert_eq!(port, Some(1234));
    }

    #[test]
    fn test_parse_address_parts_ipv4_no_port() {
        let (addr, scope, port) = parse_address_parts("8.8.8.8").unwrap();
        assert_eq!(addr, "8.8.8.8".parse::<IpAddr>().unwrap());
        assert_eq!(scope, None);
        assert_eq!(port, None);
    }

    #[test]
    fn test_parse_address_parts_ipv4_in_brackets() {
        assert!(parse_address_parts("[8.8.8.8%eno1]:1234").is_err());
    }

    #[test]
    fn test_parse_address_parts_ipv6_no_scope_in_brackets() {
        let (addr, scope, port) = parse_address_parts("[::1]:1234").unwrap();
        assert_eq!(addr, "::1".parse::<IpAddr>().unwrap());
        assert_eq!(scope, None);
        assert_eq!(port, Some(1234));
    }

    #[test]
    fn test_parse_address_parts_embedded_ipv4_address() {
        // https://www.rfc-editor.org/rfc/rfc6052#section-2
        let (addr, scope, port) = parse_address_parts("[64:ff9b::192.0.2.33%foober]:999").unwrap();
        assert_eq!(addr, "64:ff9b::192.0.2.33".parse::<IpAddr>().unwrap());
        assert_eq!(scope, Some("foober"));
        assert_eq!(port, Some(999));
    }

    #[test]
    fn test_parse_address_parts_loopback() {
        let (addr, scope, port) = parse_address_parts("[::1%eno1]:1234").unwrap();
        assert_eq!(addr, "::1".parse::<IpAddr>().unwrap());
        assert_eq!(scope, Some("eno1"));
        assert_eq!(port, Some(1234));
    }

    #[test]
    fn test_parse_address_parts_too_many_percents() {
        assert!(parse_address_parts("64:ff9b::192.0.2.33%fo%ober").is_err());
    }

    #[test]
    fn test_scoped_socket_addr_formatting() {
        let addr: SocketAddr = "[fe80::12%1]:8022".parse().unwrap();
        let saddr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let expect = "[fe80::12%lo]:8022".to_owned();
        assert_eq!(expect, saddr.to_string());
        assert_eq!(Some("lo"), saddr.scope_id());
        assert_eq!(1, saddr.scope_id_integer());
        let addr: SocketAddr = "[fe80::12%1]:0".parse().unwrap();
        let saddr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let expect = "[fe80::12%lo]:0".to_owned();
        assert_eq!(expect, saddr.to_string());
        assert_eq!(Some("lo"), saddr.scope_id());
        assert_eq!(1, saddr.scope_id_integer());
        let addr: SocketAddr = "192.168.4.2:22".parse().unwrap();
        let saddr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        let expect = "192.168.4.2:22".to_owned();
        assert_eq!(expect, saddr.to_string());
        assert_eq!(saddr.scope_id(), None);
        assert_eq!(0, saddr.scope_id_integer());
    }

    #[test]
    fn test_setting_scope_id_scoped_socketaddr() {
        // The error cases are not covered here, as in infra there doesn't appear to be a
        // consistent programmatic way to find a scope ID that does not exist. So far the only
        // approach attempted has been to find the maximum scope ID, then attempt to set the scope
        // ID to that plus one, which is a bit flakey.
        let addr: SocketAddr = "[fe80::12%1]:8022".parse().unwrap();
        let mut saddr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        saddr.set_scope_id(1).unwrap();
        assert_eq!(Some("lo"), saddr.scope_id());
        assert_eq!(1, saddr.scope_id_integer());
        // Testing Deref trait.
        assert_eq!(8022, saddr.port());
        saddr.set_port(22);
        assert_eq!(22, saddr.port());
        let addr: SocketAddr = "[fe80::12%1]:0".parse().unwrap();
        let mut saddr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        saddr.set_scope_id(1).unwrap();
        assert_eq!(Some("lo"), saddr.scope_id());
        assert_eq!(1, saddr.scope_id_integer());
        let addr: SocketAddr = "192.168.4.2:22".parse().unwrap();
        let mut saddr = ScopedSocketAddr::from_socket_addr(addr).unwrap();
        saddr.set_scope_id(2).unwrap();
        assert_eq!(saddr.scope_id(), None);
        assert_eq!(saddr.scope_id_integer(), 0);
    }
}
