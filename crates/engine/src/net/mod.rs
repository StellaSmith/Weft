//! Client/server messages. Both sides only see channel endpoints; a transport (in-process, iroh,
//! later Steam) bridges those channels to wherever the peer is. The in-process transport moves
//! Rust values directly, network transports serialize in their bridge threads.

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use glam::{DVec3, IVec3};

use voxide_abi::ItemStack;

use crate::world::Cube;

pub mod codec;
pub mod p2p;
pub mod screens;

/// Ticket others can use to join this host, once hosting is online.
pub fn current_ticket() -> Option<String> {
    p2p::HOSTING.lock().as_ref().and_then(|h| h.ticket.clone())
}

pub fn host(server: &crate::server::ServerHandle, settings: &crate::settings::Settings) {
    server.shared.hosting.store(true, std::sync::atomic::Ordering::Relaxed);
    p2p::host(server.connect.clone(), settings);
}

pub fn join(ticket: &str, settings: &crate::settings::Settings) -> Result<ClientConn, String> {
    p2p::join(ticket, settings)
}

pub const KEEP_HIDDEN: u8 = 0xFF;

#[derive(Clone, Debug)]
pub struct EntityView {
    pub id: u64,
    pub kind: u32,
    pub pos: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub item: ItemStack,
    pub health: f32,
    pub worn: Vec<u32>,
}

/// A debug shape for a gizmo group (`client::gizmo`): `kind` 0 is a line from `a` to `b`, 1 a box
/// between corners `a` and `b`, 2 `text` at `a`. Shown for `ticks` server ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct GizmoShape {
    pub group: String,
    pub kind: u8,
    pub a: DVec3,
    pub b: DVec3,
    pub color: u32,
    pub ticks: u32,
    pub text: String,
}

#[derive(Clone)]
pub enum ToClient {
    Welcome { player: u64, dim: u32, pos: DVec3, seed: u64 },
    /// `hidden`: faces whose neighbour the server will not send (not visible from the player);
    /// `KEEP_HIDDEN` on resends keeps what the client already knows.
    Cube { dim: u32, pos: IVec3, cube: Arc<Cube>, hidden: u8 },
    Unload { dim: u32, pos: IVec3 },
    Blocks { dim: u32, changes: Vec<(IVec3, u32)> },
    Time { dim: u32, time: u64 },
    Teleport { dim: u32, pos: DVec3 },
    Spawn(Vec<EntityView>),
    /// id, position, yaw, pitch.
    Moves(Vec<(u64, DVec3, f32, f32)>),
    Despawn(Vec<u64>),
    Hurt(Vec<(u64, f32)>),
    /// Every known entity that is on fire, sent once a second.
    Burning(Vec<u64>),
    Worn(Vec<(u64, Vec<u32>)>),
    GameMode(crate::world::GameMode),
    /// Everyone online, sent when someone joins or leaves.
    Players(Vec<String>),
    /// Own inventory: (slot, stack) for changed slots.
    Inventory(Vec<(u32, ItemStack)>),
    /// Own synced components: (component id, bytes).
    Components(Vec<(u32, Vec<u8>)>),
    Health { health: f32, max: f32 },
    /// Hit by something at this horizontal direction from the player.
    Knockback([f32; 2]),
    OpenMenu { kind: u32, window: u32, slots: u32, owner: u64 },
    MenuSlots(Vec<(u32, ItemStack)>),
    MenuData(Vec<(u32, i64)>),
    Cursor(ItemStack),
    CloseMenu,
    Died,
    Chat(String),
    Disconnect(String),
    Gizmos(Vec<GizmoShape>),
    /// Mod channel message (`Host::reg_packet`).
    Packet { channel: u32, data: Vec<u8> },
    /// Components of other entities: (entity, component id, bytes).
    EntityComponents(Vec<(u64, u32, Vec<u8>)>),
}

#[derive(Clone)]
pub enum ToServer {
    /// `key`: the player's public key, their identity across sessions and addresses.
    Join { name: String, key: String },
    Move { pos: DVec3, yaw: f32, pitch: f32, on_ground: bool },
    Break { pos: IVec3 },
    /// Right click on a block: the block reacts first (chests), otherwise the held item is used or
    /// placed against the clicked face.
    Use { pos: IVec3, face: u8, look: [f32; 3], sneak: bool },
    UseItem { target: Option<(IVec3, u8)> },
    Select(u32),
    Attack(u64),
    Interact(u64),
    OpenInventory,
    Click { slot: u32, button: u8, shift: bool },
    /// Creative inventory: a full stack of this item onto the menu cursor.
    /// Click on a creative entry: 0 left (stack), 1 right (one).
    CreativeTake { item: u32, button: u8 },
    /// Middle click on a block whose item is not in the hotbar: bring it into the selected slot
    /// from the inventory, or in creative make one.
    Pick { item: u32 },
    /// The cursor stack spread over these slots by dragging (button 0 evenly, 1 one each).
    Drag { slots: Vec<u32>, button: u8 },
    CloseMenu,
    Drop { all: bool },
    Respawn,
    Chat(String),
    Leave,
    /// Gizmo groups this player shows; the server only sends shapes of these.
    Gizmos(Vec<String>),
    Packet { channel: u32, data: Vec<u8> },
}

/// Message kinds by name, for traffic counters (`net` log channel).
pub trait Message {
    const TO_SERVER: bool;
    const KINDS: &'static [&'static str];
    fn kind(&self) -> usize;
}

impl Message for ToClient {
    const TO_SERVER: bool = false;
    const KINDS: &'static [&'static str] = &[
        "Welcome", "Cube", "Unload", "Blocks", "Time", "Teleport", "Spawn", "Moves", "Despawn", "Hurt", "Burning", "Worn", "GameMode", "Players", "Inventory",
        "Components", "Health", "Knockback", "OpenMenu", "MenuSlots", "MenuData", "Cursor", "CloseMenu", "Died", "Chat", "Disconnect", "Gizmos", "Packet",
        "EntityComponents",
    ];
    fn kind(&self) -> usize {
        use ToClient::*;
        match self {
            Welcome { .. } => 0,
            Cube { .. } => 1,
            Unload { .. } => 2,
            Blocks { .. } => 3,
            Time { .. } => 4,
            Teleport { .. } => 5,
            Spawn(_) => 6,
            Moves(_) => 7,
            Despawn(_) => 8,
            Hurt(_) => 9,
            Burning(_) => 10,
            Worn(_) => 11,
            GameMode(_) => 12,
            Players(_) => 13,
            Inventory(_) => 14,
            Components(_) => 15,
            Health { .. } => 16,
            Knockback(_) => 17,
            OpenMenu { .. } => 18,
            MenuSlots(_) => 19,
            MenuData(_) => 20,
            Cursor(_) => 21,
            CloseMenu => 22,
            Died => 23,
            Chat(_) => 24,
            Disconnect(_) => 25,
            Gizmos(_) => 26,
            Packet { .. } => 27,
            EntityComponents(_) => 28,
        }
    }
}

impl Message for ToServer {
    const TO_SERVER: bool = true;
    const KINDS: &'static [&'static str] = &[
        "Join", "Move", "Break", "Use", "UseItem", "Select", "Attack", "Interact", "OpenInventory", "Click", "CreativeTake", "Pick", "Drag", "CloseMenu", "Drop",
        "Respawn", "Chat", "Leave", "Gizmos", "Packet",
    ];
    fn kind(&self) -> usize {
        use ToServer::*;
        match self {
            Join { .. } => 0,
            Move { .. } => 1,
            Break { .. } => 2,
            Use { .. } => 3,
            UseItem { .. } => 4,
            Select(_) => 5,
            Attack(_) => 6,
            Interact(_) => 7,
            OpenInventory => 8,
            Click { .. } => 9,
            CreativeTake { .. } => 10,
            Pick { .. } => 11,
            Drag { .. } => 12,
            CloseMenu => 13,
            Drop { .. } => 14,
            Respawn => 15,
            Chat(_) => 16,
            Leave => 17,
            Gizmos(_) => 18,
            Packet { .. } => 19,
        }
    }
}

pub struct Conn<S, R> {
    tx: Sender<S>,
    rx: Receiver<R>,
    closed: bool,
}

impl<S: Message, R> Conn<S, R> {
    pub fn new(tx: Sender<S>, rx: Receiver<R>) -> Self {
        Conn { tx, rx, closed: false }
    }

    pub fn send(&mut self, m: S) {
        crate::diag::net_message(S::TO_SERVER, m.kind(), 1, 0);
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
