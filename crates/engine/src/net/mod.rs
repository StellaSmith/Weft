//! Client/server messages. Both sides only see channel endpoints; a transport (in-process, iroh,
//! later Steam) bridges those channels to wherever the peer is. The in-process transport moves
//! Rust values directly, network transports serialize in their bridge threads.

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use glam::{DVec3, IVec3};

use weft_abi::ItemStack;

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
    pub text: crate::ui::text::Msg,
}

/// An entity's move: id, position, yaw, pitch, crouching, and its frame: the solid body it
/// stands on (0: the world), which clients draw it relative to.
pub type Move = (u64, DVec3, f32, f32, bool, u64);

#[derive(Clone)]
pub enum ToClient {
    Welcome { player: u64, dim: u32, pos: DVec3, seed: u64 },
    /// `hidden`: faces whose neighbor the server will not send (not visible from the player);
    /// `KEEP_HIDDEN` on resends keeps what the client already knows.
    Cube { dim: u32, pos: IVec3, cube: Arc<Cube>, hidden: u8 },
    /// Cubes of a moving structure (entity `id`), in its own coordinates; `full`: all of them
    /// (the structure was not known), otherwise the ones that changed.
    Structure { id: u64, cubes: Vec<(IVec3, Arc<Cube>)>, full: bool },
    Unload { dim: u32, pos: IVec3 },
    Blocks { dim: u32, changes: Vec<(IVec3, u32)> },
    Time { dim: u32, time: u64 },
    /// `frame`: the solid body it was put on and the position relative to that body (clients
    /// draw bodies ahead of the server, so only the relative position is right for them).
    Teleport { dim: u32, pos: DVec3, frame: Option<(u64, DVec3)> },
    Spawn(Vec<EntityView>),
    /// Moves of one server tick (its number: clients time the moves by it, not by when they
    /// arrive).
    Moves(u64, Vec<Move>),
    Despawn(Vec<u64>),
    Hurt(Vec<(u64, f32)>),
    /// Items picked up: (item entity, who took it). They fly to the taker before going.
    Taken(Vec<(u64, u64)>),
    /// A use (right click) did something: the arm swings.
    Swing,
    /// Entities' own sizes (width, height, eye; width 0: the kind's) and looks (another kind,
    /// `u32::MAX`: its own).
    Shapes(Vec<(u64, [f32; 3], u32)>),
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
    /// Velocity change in blocks per tick, after halving the velocity when the flag says so
    /// (knockback, a mod's push).
    Push(([f32; 3], bool)),
    /// The player's walking speed and jump multipliers.
    Movement { speed: f32, jump: f32 },
    /// `block`: the state it was opened on (0: none), `name`: what it belongs to is called.
    OpenMenu { kind: u32, window: u32, slots: u32, owner: u64, block: u32, name: String },
    MenuSlots(Vec<(u32, ItemStack)>),
    Cursor(ItemStack),
    CloseMenu,
    Died,
    /// Formatted by the client in its own language.
    Chat(crate::ui::text::Msg),
    /// The answer to `ToServer::Command` `id`, for whoever ran it rather than the chat.
    CommandReply { id: u32, lines: Vec<crate::ui::text::Msg> },
    Disconnect(String),
    Gizmos(Vec<GizmoShape>),
    /// Mod channel message (`Host::reg_packet`).
    Packet { channel: u32, data: Vec<u8> },
    /// Components of other entities: (entity, component id, bytes).
    EntityComponents(Vec<(u64, u32, Vec<u8>)>),
    Particles(Vec<weft_abi::ParticleBurst>),
    /// The server's values of `SYNCED` settings: (config id, value).
    Configs(Vec<(u32, f64)>),
    /// Sound events: (event, position, volume, pitch or 0).
    Sounds(Vec<(u32, DVec3, f32, f32)>),
    /// Keyframe animations started on entities: (entity, name, looping); empty name = none.
    Animate(Vec<(u64, String, bool)>),
    /// A player's own skin (PNG), for their entity.
    Skin { entity: u64, png: Vec<u8> },
}

#[derive(Clone)]
pub enum ToServer {
    /// `key`: the player's public key, their identity across sessions and addresses.
    Join { name: String, key: String },
    /// `frame`: the body it rides and its position relative to that body as the client shows it
    /// (the client shows bodies behind the server; only the relative position agrees).
    Move { pos: DVec3, yaw: f32, pitch: f32, on_ground: bool, crouched: bool, frame: Option<(u64, DVec3)> },
    /// `frame`: the structure (entity) the block is part of, its position in the structure's
    /// coordinates; 0 for the world.
    Break { pos: IVec3, frame: u64 },
    /// Right click on a block: the block reacts first (chests), otherwise the held item is used or
    /// placed against the clicked face. `frame` like `Break`'s; `look` in world space.
    Use { pos: IVec3, face: u8, look: [f32; 3], sneak: bool, frame: u64 },
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
    /// A command run from code (`client_op::RUN_COMMAND`), answered with `CommandReply`.
    Command { id: u32, line: String },
    Leave,
    /// Gizmo groups this player shows; the server only sends shapes of these.
    Gizmos(Vec<String>),
    Packet { channel: u32, data: Vec<u8> },
    /// The player's skin (PNG, 64 x 64 or 64 x 32).
    Skin(Vec<u8>),
}

/// Message kinds by name, for traffic counters (`net` log channel).
pub trait Message {
    const TO_SERVER: bool;
    const KINDS: &'static [&'static str];
    fn kind(&self) -> usize;
}

/// Every message kind once: its wire tag (position), the name in traffic counters, `kind()`.
macro_rules! messages {
    ($msg:ident, $tag:ident, $to_server:expr, [$($v:ident),* $(,)?]) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum $tag {
            $($v),*
        }

        impl $tag {
            pub const ALL: &'static [$tag] = &[$($tag::$v),*];
        }

        impl Message for $msg {
            const TO_SERVER: bool = $to_server;
            const KINDS: &'static [&'static str] = &[$(stringify!($v)),*];
            fn kind(&self) -> usize {
                match self {
                    $($msg::$v { .. } => $tag::$v as usize,)*
                }
            }
        }
    };
}

messages!(ToClient, ClientTag, false, [
    Welcome, Cube, Unload, Blocks, Time, Teleport, Spawn, Moves, Despawn, Hurt, Burning, Worn,
    GameMode, Players, Inventory, Components, Health, Push, Movement, OpenMenu, MenuSlots,
    Cursor, CloseMenu, Died, Chat, Disconnect, Gizmos, Packet, EntityComponents, Particles,
    Configs, Sounds, Animate, Taken, Swing, Shapes, Structure, Skin, CommandReply,
]);

messages!(ToServer, ServerTag, true, [
    Join, Move, Break, Use, UseItem, Select, Attack, Interact, OpenInventory, Click, CreativeTake,
    Pick, Drag, CloseMenu, Drop, Respawn, Chat, Leave, Gizmos, Packet, Skin, Command,
]);

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
