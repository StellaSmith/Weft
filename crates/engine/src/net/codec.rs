//! Binary encoding of messages for network transports. The in-process transport never uses it.
//! Runtime ids are sent as is: peers verify they run the same content before playing.

use std::sync::Arc;

use glam::{DVec3, IVec3};
use weft_abi::ItemStack;

use super::{ClientTag, EntityView, Message, ServerTag, ToClient, ToServer};
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
        let data = if s.data == 0 { Vec::new() } else { crate::stack::to_names(s.data) };
        self.len(data.len());
        for (name, bytes) in &data {
            self.str(name);
            self.bytes(bytes);
        }
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
        let (item, count) = (self.u32()?, self.u32()?);
        let n = self.len()?;
        let mut data = Vec::with_capacity(n.min(16));
        for _ in 0..n {
            data.push((self.str()?, self.bytes()?));
        }
        Some(ItemStack { item, count, data: crate::stack::from_names(data) })
    }
    fn len(&mut self) -> Res<usize> {
        let n = self.u32()? as usize;
        (n <= 1 << 20).then_some(n)
    }
}

pub fn encode_client(m: &ToClient) -> Vec<u8> {
    let mut w = W::default();
    w.u8(m.kind() as u8);
    match m {
        ToClient::Welcome { player, dim, pos, seed } => {
            w.u64(*player);
            w.u32(*dim);
            w.dvec(*pos);
            w.u64(*seed);
        }
        ToClient::Cube { dim, pos, cube, hidden } => {
            w.u32(*dim);
            w.ivec(*pos);
            w.u8(*hidden);
            let mut raw = Vec::new();
            cube.write(&mut raw, |s| s);
            w.bytes(&raw);
        }
        ToClient::Unload { dim, pos } => {
            w.u32(*dim);
            w.ivec(*pos);
        }
        ToClient::Blocks { dim, changes } => {
            w.u32(*dim);
            w.len(changes.len());
            for (p, s) in changes {
                w.ivec(*p);
                w.u32(*s);
            }
        }
        ToClient::Time { dim, time } => {
            w.u32(*dim);
            w.u64(*time);
        }
        ToClient::Teleport { dim, pos, frame } => {
            w.u32(*dim);
            w.dvec(*pos);
            let (body, rel) = frame.unwrap_or_default();
            w.u64(body);
            w.dvec(rel);
        }
        ToClient::Spawn(list) => {
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
                w.u32(e.block);
            }
        }
        ToClient::Moves(tick, list) => {
            w.u64(*tick);
            w.len(list.len());
            for (id, p, y, pi, crouched, frame) in list {
                w.u64(*id);
                w.dvec(*p);
                w.f32(*y);
                w.f32(*pi);
                w.u8(*crouched as u8);
                w.u64(*frame);
            }
        }
        ToClient::Despawn(ids) => {
            w.len(ids.len());
            for id in ids {
                w.u64(*id);
            }
        }
        ToClient::Burning(ids) => {
            w.len(ids.len());
            for id in ids {
                w.u64(*id);
            }
        }
        ToClient::Hurt(list) => {
            w.len(list.len());
            for (id, h) in list {
                w.u64(*id);
                w.f32(*h);
            }
        }
        ToClient::Taken(list) => {
            w.len(list.len());
            for (item, by) in list {
                w.u64(*item);
                w.u64(*by);
            }
        }
        ToClient::Structure { id, cubes, full } => {
            w.u64(*id);
            w.u8(*full as u8);
            w.len(cubes.len());
            for (pos, cube) in cubes {
                w.ivec(*pos);
                let mut raw = Vec::new();
                cube.write(&mut raw, |s| s);
                w.bytes(&raw);
            }
        }
        ToClient::Shapes(list) => {
            w.len(list.len());
            for (id, size, look) in list {
                w.u64(*id);
                for v in size {
                    w.f32(*v);
                }
                w.u32(*look);
            }
        }
        ToClient::Inventory(slots) => {
            w.len(slots.len());
            for (i, s) in slots {
                w.u32(*i);
                w.stack(*s);
            }
        }
        ToClient::Components(list) => {
            w.len(list.len());
            for (k, v) in list {
                w.u32(*k);
                w.bytes(v);
            }
        }
        ToClient::Health { health, max } => {
            w.f32(*health);
            w.f32(*max);
        }
        ToClient::Animate(list) => {
            w.len(list.len());
            for (id, name, looping) in list {
                w.u64(*id);
                w.str(name);
                w.u8(*looping as u8);
            }
        }
        ToClient::Sounds(list) => {
            w.len(list.len());
            for (s, pos, volume, pitch) in list {
                w.u32(*s);
                w.dvec(*pos);
                w.f32(*volume);
                w.f32(*pitch);
            }
        }
        ToClient::Configs(list) => {
            w.len(list.len());
            for (id, v) in list {
                w.u32(*id);
                w.u64(v.to_bits());
            }
        }
        ToClient::Particles(list) => {
            w.len(list.len());
            for b in list {
                w.u32(b.kind);
                w.dvec(DVec3::from_array(b.pos));
                for v in b.spread.into_iter().chain(b.velocity).chain([b.speed]) {
                    w.f32(v);
                }
                w.u32(b.count);
                w.u32(b.block);
            }
        }
        ToClient::EntityComponents(list) => {
            w.len(list.len());
            for (id, comp, b) in list {
                w.u64(*id);
                w.u32(*comp);
                w.bytes(b);
            }
        }
        ToClient::OpenMenu { kind, window, slots, owner } => {
            w.u64(*owner);
            w.u32(*kind);
            w.u32(*window);
            w.u32(*slots);
        }
        ToClient::MenuSlots(list) => {
            w.len(list.len());
            for (i, s) in list {
                w.u32(*i);
                w.stack(*s);
            }
        }
        ToClient::Cursor(s) => {
            w.stack(*s);
        }
        ToClient::CloseMenu => {}
        ToClient::Died | ToClient::Swing => {}
        ToClient::Knockback([x, z]) => {
            w.f32(*x);
            w.f32(*z);
        }
        ToClient::Chat(t) => {
            w.str(t);
        }
        ToClient::Disconnect(t) => {
            w.str(t);
        }
        ToClient::GameMode(mode) => {
            w.u8(*mode as u8);
        }
        ToClient::Players(names) => {
            w.len(names.len());
            for n in names {
                w.str(n);
            }
        }
        ToClient::Gizmos(list) => {
            w.len(list.len());
            for g in list {
                w.str(&g.group);
                w.u8(g.kind);
                w.dvec(g.a);
                w.dvec(g.b);
                w.u32(g.color);
                w.u32(g.ticks);
                w.str(&g.text);
            }
        }
        ToClient::Packet { channel, data } => {
            w.u32(*channel);
            w.bytes(data);
        }
        ToClient::Skin { entity, png } => {
            w.u64(*entity);
            w.bytes(png);
        }
        ToClient::Worn(list) => {
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
    Some(match *ClientTag::ALL.get(r.u8()? as usize)? {
        ClientTag::Welcome => ToClient::Welcome { player: r.u64()?, dim: r.u32()?, pos: r.dvec()?, seed: r.u64()? },
        ClientTag::Cube => {
            let dim = r.u32()?;
            let pos = r.ivec()?;
            let hidden = r.u8()?;
            let raw = r.bytes()?;
            let max = crate::content::content().states.len() as u32;
            let cube = Cube::read(&mut &raw[..], |s| if s < max { s } else { 0 })?;
            ToClient::Cube { dim, pos, cube: Arc::new(cube), hidden }
        }
        ClientTag::Unload => ToClient::Unload { dim: r.u32()?, pos: r.ivec()? },
        ClientTag::Blocks => {
            let dim = r.u32()?;
            let n = r.len()?;
            let mut changes = Vec::with_capacity(n);
            for _ in 0..n {
                changes.push((r.ivec()?, r.u32()?));
            }
            ToClient::Blocks { dim, changes }
        }
        ClientTag::Time => ToClient::Time { dim: r.u32()?, time: r.u64()? },
        ClientTag::Teleport => {
            let (dim, pos, body, rel) = (r.u32()?, r.dvec()?, r.u64()?, r.dvec()?);
            ToClient::Teleport { dim, pos, frame: (body != 0).then_some((body, rel)) }
        }
        ClientTag::Spawn => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                let mut e = EntityView { id: r.u64()?, kind: r.u32()?, pos: r.dvec()?, yaw: r.f32()?, pitch: r.f32()?, item: r.stack()?, health: r.f32()?, worn: Vec::new(), block: 0 };
                for _ in 0..r.len()? {
                    e.worn.push(r.u32()?);
                }
                e.block = r.u32()?;
                list.push(e);
            }
            ToClient::Spawn(list)
        }
        ClientTag::Moves => {
            let tick = r.u64()?;
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u64()?, r.dvec()?, r.f32()?, r.f32()?, r.u8()? != 0, r.u64()?));
            }
            ToClient::Moves(tick, list)
        }
        ClientTag::Despawn => {
            let n = r.len()?;
            let mut ids = Vec::with_capacity(n);
            for _ in 0..n {
                ids.push(r.u64()?);
            }
            ToClient::Despawn(ids)
        }
        ClientTag::Hurt => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u64()?, r.f32()?));
            }
            ToClient::Hurt(list)
        }
        ClientTag::Structure => {
            let (id, full, n) = (r.u64()?, r.u8()? != 0, r.len()?);
            let max = crate::content::content().states.len() as u32;
            let mut cubes = Vec::with_capacity(n);
            for _ in 0..n {
                let pos = r.ivec()?;
                let raw = r.bytes()?;
                cubes.push((pos, Arc::new(Cube::read(&mut &raw[..], |s| if s < max { s } else { 0 })?)));
            }
            ToClient::Structure { id, cubes, full }
        }
        ClientTag::Shapes => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u64()?, [r.f32()?, r.f32()?, r.f32()?], r.u32()?));
            }
            ToClient::Shapes(list)
        }
        ClientTag::Taken => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u64()?, r.u64()?));
            }
            ToClient::Taken(list)
        }
        ClientTag::Inventory => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u32()?, r.stack()?));
            }
            ToClient::Inventory(list)
        }
        ClientTag::Components => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u32()?, r.bytes()?));
            }
            ToClient::Components(list)
        }
        ClientTag::Health => ToClient::Health { health: r.f32()?, max: r.f32()? },
        ClientTag::OpenMenu => {
            let owner = r.u64()?;
            ToClient::OpenMenu { kind: r.u32()?, window: r.u32()?, slots: r.u32()?, owner }
        }
        ClientTag::EntityComponents => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                list.push((r.u64()?, r.u32()?, r.bytes()?));
            }
            ToClient::EntityComponents(list)
        }
        ClientTag::Particles => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                let (kind, pos) = (r.u32()?, r.dvec()?.to_array());
                let spread = [r.f32()?, r.f32()?, r.f32()?];
                let velocity = [r.f32()?, r.f32()?, r.f32()?];
                let speed = r.f32()?;
                list.push(weft_abi::ParticleBurst { kind, pos, spread, velocity, speed, count: r.u32()?, block: r.u32()?, ..weft_abi::ParticleBurst::DEFAULT });
            }
            ToClient::Particles(list)
        }
        ClientTag::Configs => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                list.push((r.u32()?, f64::from_bits(r.u64()?)));
            }
            ToClient::Configs(list)
        }
        ClientTag::Sounds => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                list.push((r.u32()?, r.dvec()?, r.f32()?, r.f32()?));
            }
            ToClient::Sounds(list)
        }
        ClientTag::Animate => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                list.push((r.u64()?, r.str()?, r.u8()? != 0));
            }
            ToClient::Animate(list)
        }
        ClientTag::MenuSlots => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push((r.u32()?, r.stack()?));
            }
            ToClient::MenuSlots(list)
        }
        ClientTag::Cursor => ToClient::Cursor(r.stack()?),
        ClientTag::CloseMenu => ToClient::CloseMenu,
        ClientTag::Died => ToClient::Died,
        ClientTag::Swing => ToClient::Swing,
        ClientTag::Chat => ToClient::Chat(r.str()?),
        ClientTag::Disconnect => ToClient::Disconnect(r.str()?),
        ClientTag::Worn => {
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
        ClientTag::Players => {
            let n = r.len()?;
            let mut names = Vec::with_capacity(n.min(1024));
            for _ in 0..n {
                names.push(r.str()?);
            }
            ToClient::Players(names)
        }
        ClientTag::GameMode => ToClient::GameMode(crate::world::GameMode::from_u8(r.u8()?)),
        ClientTag::Burning => {
            let n = r.len()?;
            let mut ids = Vec::with_capacity(n);
            for _ in 0..n {
                ids.push(r.u64()?);
            }
            ToClient::Burning(ids)
        }
        ClientTag::Knockback => ToClient::Knockback([r.f32()?, r.f32()?]),
        ClientTag::Gizmos => {
            let n = r.len()?;
            let mut list = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                list.push(crate::net::GizmoShape { group: r.str()?, kind: r.u8()?, a: r.dvec()?, b: r.dvec()?, color: r.u32()?, ticks: r.u32()?, text: r.str()? });
            }
            ToClient::Gizmos(list)
        }
        ClientTag::Packet => ToClient::Packet { channel: r.u32()?, data: r.bytes()? },
        ClientTag::Skin => ToClient::Skin { entity: r.u64()?, png: r.bytes()? },
    })
}

pub fn encode_server(m: &ToServer) -> Vec<u8> {
    let mut w = W::default();
    w.u8(m.kind() as u8);
    match m {
        ToServer::Join { name, key } => {
            w.str(name);
            w.str(key);
        }
        ToServer::Move { pos, yaw, pitch, on_ground, crouched, frame } => {
            w.dvec(*pos);
            w.f32(*yaw);
            w.f32(*pitch);
            w.u8(*on_ground as u8 | (*crouched as u8) << 1);
            let (body, rel) = frame.unwrap_or_default();
            w.u64(body);
            w.dvec(rel);
        }
        ToServer::Break { pos, frame } => {
            w.ivec(*pos);
            w.u64(*frame);
        }
        ToServer::Use { pos, face, look, sneak, frame } => {
            w.ivec(*pos);
            w.u8(*face);
            for v in look {
                w.f32(*v);
            }
            w.u8(*sneak as u8);
            w.u64(*frame);
        }
        ToServer::UseItem { target } => {
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
            w.u32(*s);
        }
        ToServer::Attack(id) => {
            w.u64(*id);
        }
        ToServer::Interact(id) => {
            w.u64(*id);
        }
        ToServer::OpenInventory => {}
        ToServer::Click { slot, button, shift } => {
            w.u32(*slot);
            w.u8(*button);
            w.u8(*shift as u8);
        }
        ToServer::CloseMenu => {}
        ToServer::Drop { all } => {
            w.u8(*all as u8);
        }
        ToServer::Respawn => {}
        ToServer::Chat(t) => {
            w.str(t);
        }
        ToServer::Leave => {}
        ToServer::Pick { item } => {
            w.u32(*item);
        }
        ToServer::CreativeTake { item, button } => {
            w.u32(*item);
            w.u8(*button);
        }
        ToServer::Gizmos(names) => {
            w.len(names.len());
            for n in names {
                w.str(n);
            }
        }
        ToServer::Packet { channel, data } => {
            w.u32(*channel);
            w.bytes(data);
        }
        ToServer::Skin(png) => w.bytes(png),
        ToServer::Drag { slots, button } => {
            w.u8(*button);
            w.len(slots.len());
            for s in slots {
                w.u32(*s);
            }
        }
    }
    w.0
}

pub fn decode_server(data: &[u8]) -> Option<ToServer> {
    let mut r = R(data);
    Some(match *ServerTag::ALL.get(r.u8()? as usize)? {
        ServerTag::Join => ToServer::Join { name: r.str()?, key: r.str()? },
        ServerTag::Move => {
            let (pos, yaw, pitch, flags, body, rel) = (r.dvec()?, r.f32()?, r.f32()?, r.u8()?, r.u64()?, r.dvec()?);
            ToServer::Move { pos, yaw, pitch, on_ground: flags & 1 != 0, crouched: flags & 2 != 0, frame: (body != 0).then_some((body, rel)) }
        }
        ServerTag::Break => ToServer::Break { pos: r.ivec()?, frame: r.u64()? },
        ServerTag::Use => ToServer::Use { pos: r.ivec()?, face: r.u8()?.min(5), look: [r.f32()?, r.f32()?, r.f32()?], sneak: r.u8()? != 0, frame: r.u64()? },
        ServerTag::UseItem => ToServer::UseItem {
            target: match r.u8()? {
                1 => Some((r.ivec()?, r.u8()?.min(5))),
                _ => None,
            },
        },
        ServerTag::Select => ToServer::Select(r.u32()?),
        ServerTag::Attack => ToServer::Attack(r.u64()?),
        ServerTag::Interact => ToServer::Interact(r.u64()?),
        ServerTag::OpenInventory => ToServer::OpenInventory,
        ServerTag::Click => ToServer::Click { slot: r.u32()?, button: r.u8()?, shift: r.u8()? != 0 },
        ServerTag::CloseMenu => ToServer::CloseMenu,
        ServerTag::Drop => ToServer::Drop { all: r.u8()? != 0 },
        ServerTag::Respawn => ToServer::Respawn,
        ServerTag::Chat => ToServer::Chat(r.str()?),
        ServerTag::Leave => ToServer::Leave,
        ServerTag::Drag => {
            let button = r.u8()?;
            let n = r.len()?;
            let mut slots = Vec::with_capacity(n.min(256));
            for _ in 0..n {
                slots.push(r.u32()?);
            }
            ToServer::Drag { slots, button }
        }
        ServerTag::CreativeTake => ToServer::CreativeTake { item: r.u32()?, button: r.u8()? },
        ServerTag::Pick => ToServer::Pick { item: r.u32()? },
        ServerTag::Gizmos => {
            let n = r.len()?;
            let mut names = Vec::with_capacity(n.min(256));
            for _ in 0..n {
                names.push(r.str()?);
            }
            ToServer::Gizmos(names)
        }
        ServerTag::Packet => ToServer::Packet { channel: r.u32()?, data: r.bytes()? },
        ServerTag::Skin => ToServer::Skin(r.bytes()?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_messages_roundtrip() {
        let msgs = [
            ToServer::Join { name: "Ann".into(), key: "k".into() },
            ToServer::Use { pos: IVec3::new(-5, 70, 3), face: 3, look: [0.1, -0.5, 0.8], sneak: true, frame: 0 },
            ToServer::Click { slot: 7, button: 1, shift: true },
            ToServer::Drag { slots: vec![3, 4, 9], button: 0 },
            ToServer::Chat("/give stone 5".into()),
            ToServer::Gizmos(vec!["regions".into(), "mymod:pipes".into()]),
            ToServer::Packet { channel: 3, data: vec![1, 2, 255] },
        ];
        for m in msgs {
            let back = decode_server(&encode_server(&m)).unwrap();
            assert_eq!(encode_server(&back), encode_server(&m));
        }
    }

    #[test]
    fn tags_follow_the_message_table() {
        for m in [ToClient::Died, ToClient::Chat("hi".into()), ToClient::Burning(vec![4]), ToClient::Animate(Vec::new())] {
            let bytes = encode_client(&m);
            assert_eq!(bytes[0] as usize, m.kind());
            assert_eq!(decode_client(&bytes).map(|b| b.kind()), Some(m.kind()));
        }
        assert_eq!(encode_server(&ToServer::Leave)[0] as usize, ToServer::Leave.kind());
    }

    #[test]
    fn gizmos_roundtrip() {
        let g = crate::net::GizmoShape { group: "regions".into(), kind: 1, a: DVec3::new(1.0, 2.0, 3.0), b: DVec3::splat(-4.5), color: 0xFF8000FF, ticks: 21, text: "r0".into() };
        let Some(ToClient::Gizmos(back)) = decode_client(&encode_client(&ToClient::Gizmos(vec![g.clone()]))) else { panic!("not decoded") };
        assert_eq!(back, vec![g]);
    }
}
