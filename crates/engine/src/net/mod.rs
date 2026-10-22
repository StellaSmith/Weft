//! Client/server messages. Both sides only see channel endpoints; a transport (in-process, iroh,
//! later Steam) bridges those channels to wherever the peer is. The in-process transport moves
//! Rust values directly, network transports serialize in their bridge threads.

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use glam::{DVec3, IVec3};

use crate::world::Cube;

pub mod screens;

/// Ticket others can use to join this host, once hosting started.
pub fn current_ticket() -> Option<String> {
    None
}

pub fn host(_server: &crate::server::ServerHandle, _settings: &crate::settings::Settings) {
    log::warn!("hosting is not available yet");
}

pub fn join(_ticket: &str, _settings: &crate::settings::Settings) -> Result<ClientConn, String> {
    Err("joining is not available yet".into())
}

#[derive(Clone)]
pub enum ToClient {
    Welcome { player: u64, dim: u32, pos: DVec3, seed: u64 },
    Cube { dim: u32, pos: IVec3, cube: Arc<Cube> },
    Unload { dim: u32, pos: IVec3 },
    Blocks { dim: u32, changes: Vec<(IVec3, u32)> },
    Time { dim: u32, time: u64 },
    Teleport { dim: u32, pos: DVec3 },
    Disconnect(String),
}

#[derive(Clone)]
pub enum ToServer {
    Join { name: String },
    Move { pos: DVec3, yaw: f32, pitch: f32, on_ground: bool },
    Break { pos: IVec3 },
    Place { pos: IVec3, face: u8, state: u32, look: [f32; 3] },
    Use { pos: IVec3, face: u8 },
    Leave,
}

pub struct Conn<S, R> {
    tx: Sender<S>,
    rx: Receiver<R>,
    closed: bool,
}

impl<S, R> Conn<S, R> {
    pub fn new(tx: Sender<S>, rx: Receiver<R>) -> Self {
        Conn { tx, rx, closed: false }
    }

    pub fn send(&mut self, m: S) {
        if self.tx.send(m).is_err() {
            self.closed = true;
        }
    }

    pub fn recv(&mut self) -> Option<R> {
        match self.rx.try_recv() {
            Ok(m) => Some(m),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.closed = true;
                None
            }
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

pub type ClientConn = Conn<ToServer, ToClient>;
pub type ServerConn = Conn<ToClient, ToServer>;

pub fn local_pair() -> (ClientConn, ServerConn) {
    let (to_server_tx, to_server_rx) = unbounded();
    let (to_client_tx, to_client_rx) = unbounded();
    (Conn::new(to_server_tx, to_client_rx), Conn::new(to_client_tx, to_server_rx))
}
