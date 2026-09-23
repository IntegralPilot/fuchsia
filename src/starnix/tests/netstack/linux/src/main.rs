// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Display;
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};

/// Command line arguments for this Linux binary that listens on a socket and
/// initiates suspension.
#[derive(argh::FromArgs)]
struct Args {
    /// the transport layer protocol to operate on.
    #[argh(option)]
    protocol: Protocol,
}

enum Protocol {
    Tcp,
    Udp,
}

impl std::str::FromStr for Protocol {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tcp" => Ok(Self::Tcp),
            "udp" => Ok(Self::Udp),
            s => Err(format!("unknown protocol: {s}")),
        }
    }
}

impl Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp => write!(f, "tcp"),
            Self::Udp => write!(f, "udp"),
        }
    }
}

const LISTEN_PORT: u16 = 33333;

fn main() {
    let Args { protocol } = argh::from_env();

    match protocol {
        Protocol::Tcp => {
            let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, LISTEN_PORT)))
                .expect("bind socket");
            println!("listening on port {LISTEN_PORT}");

            let (mut stream, addr) = listener.accept().expect("accept incoming connection");
            println!("accepted incoming connection from {addr}");

            println!("suspending...");
            std::fs::write("/sys/power/state", "mem").expect("suspend Linux");

            println!("resumed");

            // We should have been woken up for an incoming message from the client.
            let mut buf = vec![0; 1024];
            let bytes = stream.read(&mut buf).expect("read message from client");
            stream.write_all(&buf[..bytes]).expect("echo message back to client");
        }
        Protocol::Udp => {
            let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, LISTEN_PORT)))
                .expect("bind socket");
            println!("listening on port {LISTEN_PORT}");

            // UDP is connectionless, so we can't accept an incoming connection
            // like with TCP, but we can wait until we get a datagram and echo
            // it back before suspension.
            let mut buf = vec![0; 1024];
            let (bytes, from) = socket.recv_from(&mut buf).expect("read message from client");
            let sent = socket.send_to(&buf[..bytes], from).expect("echo message back to client");
            assert_eq!(bytes, sent);
            println!("echoed back initial message from {from}");

            // Suspend the system.
            println!("suspending...");
            std::fs::write("/sys/power/state", "mem").expect("suspend Linux");

            println!("resumed");

            // We should have been woken up for an incoming message from the
            // client. The client might have timed out and sent another probe
            // prior to suspension, so we loop and echo all messages until we
            // receive the expected payload.
            const PAYLOAD: &[u8] = b"Hello, world!";
            loop {
                let (bytes, from) = socket.recv_from(&mut buf).expect("read message from client");
                let sent =
                    socket.send_to(&buf[..bytes], from).expect("echo message back to client");
                assert_eq!(bytes, sent);
                if &buf[..bytes] == PAYLOAD {
                    break;
                }
            }
        }
    }

    println!("exiting");
}
