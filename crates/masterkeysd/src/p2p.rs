//! A private peer-to-peer D-Bus connection over a loopback TCP pair: the tests on every OS, and the
//! Windows app, whose core runs in-process with no session bus.

use crate::service::DaemonService;
use masterkeys_core::dbus::OBJECT_PATH;
use std::net::{TcpListener, TcpStream};
use zbus::{Guid, connection::Builder};

/// Serve `service` on one end of a fresh connection pair; returns (server, client).
pub async fn pair(service: DaemonService) -> zbus::Result<(zbus::Connection, zbus::Connection)> {
    // The connect completes from the listen backlog, so accepting afterwards doesn't block.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let client_stream = TcpStream::connect(listener.local_addr()?)?;
    let (server_stream, _) = listener.accept()?;
    let server = Builder::tcp_stream(server_stream).server(Guid::generate()).p2p().serve_at(OBJECT_PATH, service);
    let client = Builder::tcp_stream(client_stream).p2p();
    let (server, client) = tokio::join!(server.build(), client.build());
    Ok((server?, client?))
}
