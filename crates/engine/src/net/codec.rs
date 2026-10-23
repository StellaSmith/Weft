//! Binary encoding of messages for network transports. The in-process transport never uses it.
//! Runtime ids are sent as is: peers verify they run the same content before playing.

use std::sync::Arc;

use glam::{DVec3, IVec3};
use voxide_abi::ItemStack;

use super::{EntityView, ToClient, ToServer};
use crate::world::Cube;

#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn ivec(&mut self, v: IVec3) {
        self.i32(v.x);
        self.i32(v.y);
        self.i32(v.z);
    }
    fn dvec(&mut self, v: DVec3) {
        self.f64(v.x);
        self.f64(v.y);
        self.f64(v.z);
    }
    fn stack(&mut self, s: ItemStack) {
        self.u32(s.item);
        self.u32(s.count);
        self.u32(s.damage);
        self.u32(s.extra);
    }
    fn len(&mut self, n: usize) {
        self.u32(n as u32);
    }
}

pub struct R<'a>(pub &'a [u8]);

type Res<T> = Option<T>;

impl R<'_> {
    fn take(&mut self, n: usize) -> Res<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Res<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Res<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn i32(&mut self) -> Res<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Res<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Res<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f64(&mut self) -> Res<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn bytes(&mut self) -> Res<Vec<u8>> {
        let n = self.u32()? as usize;
        if n > 64 << 20 {
            return None;
        }
        Some(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Res<String> {
        String::from_utf8(self.bytes()?).ok()
    }
    fn ivec(&mut self) -> Res<IVec3> {
        Some(IVec3::new(self.i32()?, self.i32()?, self.i32()?))
    }
    fn dvec(&mut self) -> Res<DVec3> {
        Some(DVec3::new(self.f64()?, self.f64()?, self.f64()?))
    }
    fn stack(&mut self) -> Res<ItemStack> {
        Some(ItemStack { item: self.u32()?, count: self.u32()?, damage: self.u32()?, extra: self.u32()? })
    }
    fn len(&mut self) -> Res<usize> {
        let n = self.u32()? as usize;
        (n <= 1 << 20).then_some(n)
    }
}

pub fn encode_client(m: &ToClient) -> Vec<u8> {
    let mut w = W::default();
    match m {
        ToClient::Welcome { player, dim, pos, seed } => {
            w.u8(0);
            w.u64(*player);
            w.u32(*dim);
            w.dvec(*pos);
            w.u64(*seed);
        }
        ToClient::Cube { dim, pos, cube, hidden } => {
            w.u8(1);
            w.u32(*dim);
            w.ivec(*pos);
            w.u8(*hidden);
            let mut raw = Vec::new();
            cube.write(&mut raw, |s| s);
            w.bytes(&raw);
        }
        ToClient::Unload { dim, pos } => {
            w.u8(2);
            w.u32(*dim);
            w.ivec(*pos);
        }
        ToClient::Blocks { dim, changes } => {
            w.u8(3);
            w.u32(*dim);
            w.len(changes.len());
            for (p, s) in changes {
                w.ivec(*p);
                w.u32(*s);
            }
        }
        ToClient::Time { dim, time } => {
            w.u8(4);
            w.u32(*dim);
            w.u64(*time);
        }
        ToClient::Teleport { dim, pos } => {
            w.u8(5);
            w.u32(*dim);
            w.dvec(*pos);
        }
        ToClient::Spawn(list) => {
            w.u8(6);
            w.len(list.len());
            for e in list {
                w.u64(e.id);
                w.u32(e.kind);
                w.dvec(e.pos);
                w.f32(e.yaw);
                w.f32(e.pitch);
                w.stack(e.item);
                w.f32(e.health);
                w.len(e.worn.len());
                for i in &e.worn {
                    w.u32(*i);
                }
            }
        }
        ToClient::Moves(list) => {
            w.u8(7);
            w.len(list.len());
            for (id, p, y, pi) in list {
                w.u64(*id);
                w.dvec(*p);
                w.f32(*y);
                w.f32(*pi);
            }
        }
        ToClient::Despawn(ids) => {
            w.u8(8);
            w.len(ids.len());
            for id in ids {
                w.u64(*id);
            }
        }
        ToClient::Hurt(list) => {
            w.u8(9);
            w.len(list.len());
            for (id, h) in list {
                w.u64(*id);
                w.f32(*h);
            }
        }
        ToClient::Inventory(slots) => {
            w.u8(10);
            w.len(slots.len());
            for (i, s) in slots {
                w.u32(*i);
                w.stack(*s);
            }
        }
        ToClient::Components(list) => {
            w.u8(11);
            w.len(list.len());
            for (k, v) in list {
                w.u32(*k);
                w.bytes(v);
            }
        }
        ToClient::Health { health, max } => {
            w.u8(12);
            w.f32(*health);
            w.f32(*max);
        }
        ToClient::OpenMenu { kind, window, slots } => {
            w.u8(13);
            w.u32(*kind);
            w.u32(*window);
            w.u32(*slots);
        }
        ToClient::MenuSlots(list) => {
            w.u8(14);
            w.len(list.len());
            for (i, s) in list {
                w.u32(*i);
                w.stack(*s);
            }
        }
        ToClient::MenuData(list) => {
            w.u8(15);
            w.len(list.len());
            for (i, v) in list {
                w.u32(*i);
                w.i32(*v);
            }
        }
        ToClient::Cursor(s) => {
            w.u8(16);
            w.stack(*s);
        }
        ToClient::CloseMenu => w.u8(17),
        ToClient::Died => w.u8(18),
        ToClient::Chat(t) => {
            w.u8(19);
            w.str(t);
        }
        ToClient::Disconnect(t) => {
            w.u8(20);
            w.str(t);
        }
        ToClient::Worn(list) => {
            w.u8(21);
            w.len(list.len());
            for (id, items) in list {
                w.u64(*id);
                w.len(items.len());
                for i in items {
                    w.u32(*i);
                }
            }
        }
    }
    w.0
}

pub fn decode_client(data: &[u8]) -> Option<ToClient> {
    let mut r = R(data);
    Some(match r.u8()? {
        0 => ToClient::Welcome { player: r.u64()?, dim: r.u32()?, pos: r.dvec()?, seed: r.u64()? },
        1 => {
            let dim = r.u32()?;
            let pos = r.ivec()?;
            let hidden = r.u8()?;
            let raw = r.bytes()?;
            let max = crate::content::content().states.len() as u32;
            let cube = Cube::read(&mut &raw[..], |s| if s < max { s } else { 0 })?;
            ToClient::Cube { dim, pos, cube: Arc::new(cube), hidden }
        }
        2 => ToClient::Unload { dim: r.u32()?, pos: r.ivec()? },
        3 => {
            let dim = r.u32()?;
            let n = r.len()?;
            let mut changes = Vec::with_capacity(n);
            for _ in 0..n {
                changes.push((r.ivec()?, r.u32()?));
            }
            ToClient::Blocks { dim, changes }
        }
        4 => ToClient::Time { dim: r.u32()?, time: r.u64()? },
        5 => ToClient::Teleport { dim: r.u32()?, pos: r.dvec()? },
        6 => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                let mut e = EntityView { id: r.u64()?, kind: r.u32()?, pos: r.dvec()?, yaw: r.f32()?, pitch: r.f32()?, item: r.stack()?, health: r.f32()?, worn: Vec::new() };
                for _ in 0..r.len()? {
                    e.worn.push(r.u32()?);
                }
                list.push(e);
            }
            ToClient::Spawn(list)
        }
        7 => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u64()?, r.dvec()?, r.f32()?, r.f32()?));
            }
            ToClient::Moves(list)
        }
        8 => {
            let n = r.len()?;
            let mut ids = Vec::with_capacity(n);
            for _ in 0..n {
                ids.push(r.u64()?);
            }
            ToClient::Despawn(ids)
        }
        9 => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u64()?, r.f32()?));
            }
            ToClient::Hurt(list)
        }
        10 => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u32()?, r.stack()?));
            }
            ToClient::Inventory(list)
        }
        11 => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u32()?, r.bytes()?));
            }
            ToClient::Components(list)
        }
        12 => ToClient::Health { health: r.f32()?, max: r.f32()? },
        13 => ToClient::OpenMenu { kind: r.u32()?, window: r.u32()?, slots: r.u32()? },
        14 => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u32()?, r.stack()?));
            }
            ToClient::MenuSlots(list)
        }
        15 => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u32()?, r.i32()?));
            }
            ToClient::MenuData(list)
        }
        16 => ToClient::Cursor(r.stack()?),
        17 => ToClient::CloseMenu,
        18 => ToClient::Died,
        19 => ToClient::Chat(r.str()?),
        20 => ToClient::Disconnect(r.str()?),
        21 => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                let id = r.u64()?;
                let mut items = Vec::new();
                for _ in 0..r.len()? {
                    items.push(r.u32()?);
                }
                list.push((id, items));
            }
            ToClient::Worn(list)
        }
        _ => return None,
    })
}

pub fn encode_server(m: &ToServer) -> Vec<u8> {
    let mut w = W::default();
    match m {
        ToServer::Join { name, key } => {
            w.u8(0);
            w.str(name);
            w.str(key);
        }
        ToServer::Move { pos, yaw, pitch, on_ground } => {
            w.u8(1);
            w.dvec(*pos);
            w.f32(*yaw);
            w.f32(*pitch);
            w.u8(*on_ground as u8);
        }
        ToServer::Break { pos } => {
            w.u8(2);
            w.ivec(*pos);
        }
        ToServer::Use { pos, face, look, sneak } => {
            w.u8(3);
            w.ivec(*pos);
            w.u8(*face);
            for v in look {
                w.f32(*v);
            }
            w.u8(*sneak as u8);
        }
        ToServer::UseItem { target } => {
            w.u8(4);
            match target {
                Some((p, f)) => {
                    w.u8(1);
                    w.ivec(*p);
                    w.u8(*f);
                }
                None => w.u8(0),
            }
        }
        ToServer::Select(s) => {
            w.u8(5);
            w.u32(*s);
        }
        ToServer::Attack(id) => {
            w.u8(6);
            w.u64(*id);
        }
        ToServer::Interact(id) => {
            w.u8(7);
            w.u64(*id);
        }
        ToServer::OpenInventory => w.u8(8),
        ToServer::Click { slot, button, shift } => {
            w.u8(9);
            w.u32(*slot);
            w.u8(*button);
            w.u8(*shift as u8);
        }
        ToServer::CloseMenu => w.u8(10),
        ToServer::Drop { all } => {
            w.u8(11);
            w.u8(*all as u8);
        }
        ToServer::Respawn => w.u8(12),
        ToServer::Chat(t) => {
            w.u8(13);
            w.str(t);
        }
        ToServer::Leave => w.u8(14),
    }
    w.0
}

pub fn decode_server(data: &[u8]) -> Option<ToServer> {
    let mut r = R(data);
    Some(match r.u8()? {
        0 => ToServer::Join { name: r.str()?, key: r.str()? },
        1 => ToServer::Move { pos: r.dvec()?, yaw: r.f32()?, pitch: r.f32()?, on_ground: r.u8()? != 0 },
        2 => ToServer::Break { pos: r.ivec()? },
        3 => ToServer::Use { pos: r.ivec()?, face: r.u8()?.min(5), look: [r.f32()?, r.f32()?, r.f32()?], sneak: r.u8()? != 0 },
        4 => ToServer::UseItem {
            target: match r.u8()? {
                1 => Some((r.ivec()?, r.u8()?.min(5))),
                _ => None,
            },
        },
        5 => ToServer::Select(r.u32()?),
        6 => ToServer::Attack(r.u64()?),
        7 => ToServer::Interact(r.u64()?),
        8 => ToServer::OpenInventory,
        9 => ToServer::Click { slot: r.u32()?, button: r.u8()?, shift: r.u8()? != 0 },
        10 => ToServer::CloseMenu,
        11 => ToServer::Drop { all: r.u8()? != 0 },
        12 => ToServer::Respawn,
        13 => ToServer::Chat(r.str()?),
        14 => ToServer::Leave,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_messages_roundtrip() {
        let msgs = [
            ToServer::Join { name: "Ann".into(), key: "k".into() },
            ToServer::Use { pos: IVec3::new(-5, 70, 3), face: 3, look: [0.1, -0.5, 0.8], sneak: true },
            ToServer::Click { slot: 7, button: 1, shift: true },
            ToServer::Chat("/give stone 5".into()),
        ];
        for m in msgs {
            let back = decode_server(&encode_server(&m)).unwrap();
            assert_eq!(encode_server(&back), encode_server(&m));
        }
    }
}
