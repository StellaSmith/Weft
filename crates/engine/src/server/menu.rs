//! Container menus: a list of slots pointing into inventories (the player's, a block entity's or
//! the menu's own temporary grid), the cursor stack, and synced integers. Click handling is generic;
//! mods only describe slots and react to changes (crafting).

use voxide_abi::{self as abi, ItemStack, slot_flags as sf};

use super::dim::Dim;
use crate::content::Content;
use crate::host;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    Temp,
    Player,
    Entity(u64),
}

#[derive(Clone, Copy, Debug)]
pub struct SlotRef {
    pub owner: Owner,
    pub index: u32,
    pub flags: u32,
    /// Only items with this tag or component (`u32::MAX`: any).
    pub accepts: u32,
}

pub struct OpenMenu {
    pub kind: u32,
    pub owner: u64,
    pub window: u32,
    pub slots: Vec<SlotRef>,
    pub temp: Vec<ItemStack>,
    pub cursor: ItemStack,
    pub sent_slots: Vec<ItemStack>,
    pub sent_cursor: ItemStack,
    /// The owner's menu-synced components went out once.
    pub comps_sent: bool,
}

/// What `MenuCtx` pointers handed to mods point at.
pub struct MenuCall {
    pub menu: *mut OpenMenu,
    pub dim: *mut Dim,
    pub player: u64,
}

impl MenuCall {
    fn m(&self) -> &mut OpenMenu {
        unsafe { &mut *self.menu }
    }

    fn d(&self) -> &mut Dim {
        unsafe { &mut *self.dim }
    }

    pub fn add_slots(&mut self, owner: u64, start: u32, count: u32, flags: u32) {
        self.add_slots_for(owner, start, count, flags, u32::MAX)
    }

    pub fn add_slots_for(&mut self, owner: u64, start: u32, count: u32, flags: u32, accepts: u32) {
        let owner = match owner {
            0 => Owner::Temp,
            u64::MAX => Owner::Player,
            id => Owner::Entity(id),
        };
        let m = self.m();
        for i in 0..count {
            m.slots.push(SlotRef { owner, index: start + i, flags, accepts });
        }
    }

    pub fn get(&self, slot: u32) -> ItemStack {
        get(self.m(), self.d(), self.player, slot)
    }

    pub fn set(&mut self, slot: u32, stack: ItemStack) {
        set(self.m(), self.d(), self.player, slot, stack)
    }

}

fn get(m: &OpenMenu, dim: &Dim, player: u64, slot: u32) -> ItemStack {
    let Some(r) = m.slots.get(slot as usize) else { return ItemStack::EMPTY };
    match r.owner {
        Owner::Temp => m.temp.get(r.index as usize).copied().unwrap_or_default(),
        Owner::Player => dim.entities.inventories.get(&player).and_then(|i| i.get(r.index as usize)).copied().unwrap_or_default(),
        Owner::Entity(id) => dim.entities.inventories.get(&id).and_then(|i| i.get(r.index as usize)).copied().unwrap_or_default(),
    }
}

fn set(m: &mut OpenMenu, dim: &mut Dim, player: u64, slot: u32, stack: ItemStack) {
    let Some(r) = m.slots.get(slot as usize).copied() else { return };
    let stack = if stack.count == 0 { ItemStack::EMPTY } else { stack };
    match r.owner {
        Owner::Temp => {
            if let Some(s) = m.temp.get_mut(r.index as usize) {
                *s = stack;
            }
        }
        Owner::Player => dim.entities.set_slot(player, r.index, stack),
        Owner::Entity(id) => dim.entities.set_slot(id, r.index, stack),
    }
}

fn call(m: &mut OpenMenu, dim: &mut Dim, c: &Content, player: u64, slot: u32, pick: impl Fn(&crate::registry::MenuDef) -> Option<abi::Callback>) {
    let def = &c.game.menus[m.kind as usize];
    let Some(cb) = pick(def) else { return };
    let owner = m.owner;
    let mut call = MenuCall { menu: m as *mut OpenMenu, dim: dim as *mut Dim, player };
    let mut ctx = host::CallCtx { dim: dim as *mut Dim };
    let mut args = abi::MenuArgs { world: ctx.raw(), menu: &mut call as *mut MenuCall as *mut abi::MenuCtx, player, owner, slot };
    host::invoke(def.owner, cb, &mut args);
}

pub fn open(dim: &mut Dim, c: &Content, player: u64, kind: u32, owner: u64, window: u32) -> OpenMenu {
    let def = &c.game.menus[kind as usize];
    let mut m = OpenMenu {
        kind,
        owner,
        window,
        slots: Vec::new(),
        temp: vec![ItemStack::EMPTY; def.temp_slots as usize],
        cursor: ItemStack::EMPTY,
        sent_slots: Vec::new(),
        sent_cursor: ItemStack::EMPTY,
        comps_sent: false,
    };
    call(&mut m, dim, c, player, 0, |d| d.setup);
    call(&mut m, dim, c, player, 0, |d| d.on_change);
    m
}

pub fn tick(m: &mut OpenMenu, dim: &mut Dim, c: &Content, player: u64) {
    call(m, dim, c, player, 0, |d| d.on_tick);
}

/// Returns temp slots and the cursor to the player (dropping what does not fit). Output slots are
/// previews, not real items, and are discarded.
pub fn close(m: OpenMenu, dim: &mut Dim, c: &Content, player: u64) {
    let outputs: Vec<u32> = m.slots.iter().filter(|s| s.owner == Owner::Temp && s.flags & sf::OUTPUT != 0).map(|s| s.index).collect();
    let mut back: Vec<ItemStack> =
        m.temp.iter().enumerate().filter(|(i, s)| !s.is_empty() && !outputs.contains(&(*i as u32))).map(|(_, s)| *s).collect();
    back.push(m.cursor);
    for s in back.into_iter().filter(|s| !s.is_empty()) {
        let rest = dim.entities.insert_item(c, player, s);
        if !rest.is_empty() {
            if let Some(p) = dim.entities.get(player).map(|e| e.pos) {
                dim.spawn_item(c, p + glam::DVec3::Y, rest);
            }
        }
    }
}

fn fits(c: &Content, s: &SlotRef, item: u32) -> bool {
    s.accepts == u32::MAX || item == 0 || c.components.has(crate::components::ITEM, item, s.accepts)
}

/// `button` 2..=10 over a slot: number key 1..9, swaps it with that hotbar slot (vanilla).
pub const HOTBAR_SWAP: u8 = 2;
/// Double click: gathers matching items from every slot onto the cursor, up to a full stack.
pub const GATHER: u8 = 11;

/// `button`: 0 left, 1 right, or `HOTBAR_SWAP + n`, `GATHER`. `slot == u32::MAX` means outside
/// the window (drop the cursor).
pub fn click(m: &mut OpenMenu, dim: &mut Dim, c: &'static Content, player: u64, slot: u32, button: u8, shift: bool) {
    if (HOTBAR_SWAP..HOTBAR_SWAP + 9).contains(&button) {
        let hot = (button - HOTBAR_SWAP) as u32;
        if hot >= c.game.player_layout.hotbar {
            return;
        }
        let Some(h) = m.slots.iter().position(|s| s.owner == Owner::Player && s.index == hot).map(|i| i as u32) else { return };
        let (Some(a), Some(b)) = (m.slots.get(slot as usize).copied(), m.slots.get(h as usize).copied()) else { return };
        if h == slot || !m.cursor.is_empty() {
            return;
        }
        let (sa, sb) = (get(m, dim, player, slot), get(m, dim, player, h));
        // Results only come out (into an empty hotbar slot), and fitted slots take what fits.
        let output = a.flags & sf::OUTPUT != 0;
        if (output && !sb.is_empty()) || !fits(c, &a, sb.item) || !fits(c, &b, sa.item) {
            return;
        }
        set(m, dim, player, slot, sb);
        set(m, dim, player, h, sa);
        if output {
            call(m, dim, c, player, slot, |d| d.on_take);
        }
        call(m, dim, c, player, slot, |d| d.on_change);
        call(m, dim, c, player, h, |d| d.on_change);
        return;
    }
    if button == GATHER {
        let want = m.cursor;
        if want.is_empty() {
            return;
        }
        let max = c.max_stack(want.item);
        // Partial stacks first, like vanilla, so full ones stay put when they are not needed.
        for full in [false, true] {
            for i in 0..m.slots.len() as u32 {
                if m.cursor.count >= max {
                    break;
                }
                let s = get(m, dim, player, i);
                if m.slots[i as usize].flags & sf::OUTPUT != 0 || s.is_empty() || s.item != want.item || s.data != want.data || (s.count >= max) != full {
                    continue;
                }
                let n = s.count.min(max - m.cursor.count);
                m.cursor.count += n;
                set(m, dim, player, i, if n == s.count { ItemStack::EMPTY } else { ItemStack { count: s.count - n, ..s } });
                call(m, dim, c, player, i, |d| d.on_change);
            }
        }
        return;
    }
    if slot == u32::MAX {
        if !m.cursor.is_empty() {
            let drop = if button == 1 { ItemStack { count: 1, ..m.cursor } } else { m.cursor };
            m.cursor.count -= drop.count;
            if m.cursor.count == 0 {
                m.cursor = ItemStack::EMPTY;
            }
            if let Some(p) = dim.entities.get(player).map(|e| e.drop_point(c)) {
                dim.spawn_item(c, p, drop);
            }
        }
        return;
    }
    let Some(r) = m.slots.get(slot as usize).copied() else { return };
    let cur = get(m, dim, player, slot);
    let output = r.flags & sf::OUTPUT != 0;
    let max = |item: u32| c.max_stack(item);

    if output {
        if cur.is_empty() {
            return;
        }
        if shift {
            // Craft into the inventory as long as the result stays the same and fits entirely;
            // a result that does not fit is not taken (no partial inserts, no lost ingredients).
            for _ in 0..max(cur.item).max(64) {
                let now = get(m, dim, player, slot);
                if now.is_empty() || now.item != cur.item || now.data != cur.data {
                    break;
                }
                let backup = dim.entities.inventories.get(&player).cloned();
                if !dim.entities.insert_item(c, player, now).is_empty() {
                    if let Some(b) = backup {
                        dim.entities.inventories.insert(player, b);
                    }
                    break;
                }
                dim.entities.dirty_inv.insert(player);
                set(m, dim, player, slot, ItemStack::EMPTY);
                call(m, dim, c, player, slot, |d| d.on_take);
                call(m, dim, c, player, slot, |d| d.on_change);
            }
            return;
        } else if m.cursor.is_empty() {
            m.cursor = cur;
        } else if m.cursor.item == cur.item && m.cursor.data == cur.data && m.cursor.count + cur.count <= max(cur.item) {
            m.cursor.count += cur.count;
        } else {
            return;
        }
        set(m, dim, player, slot, ItemStack::EMPTY);
        call(m, dim, c, player, slot, |d| d.on_take);
        call(m, dim, c, player, slot, |d| d.on_change);
        return;
    }

    if shift {
        if cur.is_empty() {
            return;
        }
        // Player slots move to the container part, container slots move to the player part.
        let from_player = r.owner == Owner::Player;
        let mut rest = cur;
        let targets: Vec<u32> = (0..m.slots.len() as u32)
            .filter(|&i| {
                let s = m.slots[i as usize];
                i != slot && s.flags & sf::OUTPUT == 0 && (s.owner == Owner::Player) != from_player && fits(c, &s, cur.item)
            })
            .collect();
        // Slots made for this item (armour) win over generic ones.
        let fitted: Vec<u32> = (0..m.slots.len() as u32)
            .filter(|&i| {
                let s = m.slots[i as usize];
                s.accepts != u32::MAX && fits(c, &s, cur.item) && get(m, dim, player, i).is_empty()
            })
            .collect();
        let layout = &c.game.player_layout;
        let targets = if r.accepts == u32::MAX && !fitted.is_empty() {
            fitted
        } else if targets.is_empty() && from_player {
            // Inventory screen: hotbar <-> main.
            let hotbar = r.index < layout.hotbar;
            (0..m.slots.len() as u32)
                .filter(|&i| {
                    let s = m.slots[i as usize];
                    s.owner == Owner::Player && s.index < layout.slots && !layout.worn.contains(&s.index) && (s.index < layout.hotbar) != hotbar
                })
                .collect()
        } else {
            targets
        };
        for pass in 0..2 {
            for &t in &targets {
                if rest.count == 0 {
                    break;
                }
                let ts = get(m, dim, player, t);
                if pass == 0 && !ts.is_empty() && ts.item == rest.item && ts.data == rest.data && ts.count < max(rest.item) {
                    let n = (max(rest.item) - ts.count).min(rest.count);
                    set(m, dim, player, t, ItemStack { count: ts.count + n, ..ts });
                    rest.count -= n;
                } else if pass == 1 && ts.is_empty() {
                    let n = rest.count.min(max(rest.item));
                    set(m, dim, player, t, ItemStack { count: n, ..rest });
                    rest.count -= n;
                }
            }
        }
        set(m, dim, player, slot, rest);
        call(m, dim, c, player, slot, |d| d.on_change);
        return;
    }

    if !fits(c, &r, m.cursor.item) {
        return;
    }
    let mut cursor = m.cursor;
    let mut slot_stack = cur;
    let same = !cursor.is_empty() && !slot_stack.is_empty() && cursor.item == slot_stack.item && cursor.data == slot_stack.data;
    match (button, cursor.is_empty(), slot_stack.is_empty()) {
        (_, true, true) => return,
        (0, true, false) => {
            cursor = slot_stack;
            slot_stack = ItemStack::EMPTY;
        }
        (_, true, false) => {
            let half = slot_stack.count.div_ceil(2);
            cursor = ItemStack { count: half, ..slot_stack };
            slot_stack.count -= half;
        }
        (0, false, true) => {
            let n = cursor.count.min(max(cursor.item));
            slot_stack = ItemStack { count: n, ..cursor };
            cursor.count -= n;
        }
        (_, false, true) => {
            slot_stack = ItemStack { count: 1, ..cursor };
            cursor.count -= 1;
        }
        (0, false, false) if same => {
            let n = (max(cursor.item) - slot_stack.count).min(cursor.count);
            slot_stack.count += n;
            cursor.count -= n;
        }
        (_, false, false) if same => {
            if slot_stack.count < max(cursor.item) {
                slot_stack.count += 1;
                cursor.count -= 1;
            }
        }
        (_, false, false) => std::mem::swap(&mut cursor, &mut slot_stack),
    }
    if cursor.count == 0 {
        cursor = ItemStack::EMPTY;
    }
    m.cursor = cursor;
    set(m, dim, player, slot, slot_stack);
    call(m, dim, c, player, slot, |d| d.on_change);
}

/// Dragging the cursor stack over `slots`: button 0 splits it evenly, button 1 leaves one item in
/// each. Slots that cannot take the item are skipped; whatever is left stays on the cursor.
pub fn drag(m: &mut OpenMenu, dim: &mut Dim, c: &'static Content, player: u64, slots: &[u32], button: u8) {
    let cur = m.cursor;
    if cur.is_empty() {
        return;
    }
    let max = c.max_stack(cur.item);
    let mut seen = rustc_hash::FxHashSet::default();
    let valid: Vec<u32> = slots
        .iter()
        .copied()
        .filter(|&s| seen.insert(s))
        .filter(|&s| {
            let Some(r) = m.slots.get(s as usize) else { return false };
            let t = get(m, dim, player, s);
            r.flags & sf::OUTPUT == 0 && fits(c, &r, cur.item) && (t.is_empty() || (t.item == cur.item && t.data == cur.data && t.count < max))
        })
        .take(cur.count as usize)
        .collect();
    if valid.is_empty() {
        return;
    }
    let per = if button == 1 { 1 } else { cur.count / valid.len() as u32 };
    let mut left = cur.count;
    for &s in &valid {
        let t = get(m, dim, player, s);
        let n = per.min(max - t.count.min(max)).min(left);
        if n > 0 {
            set(m, dim, player, s, ItemStack { count: t.count + n, ..cur });
            left -= n;
        }
    }
    m.cursor = if left == 0 { ItemStack::EMPTY } else { ItemStack { count: left, ..cur } };
    for &s in &valid {
        call(m, dim, c, player, s, |d| d.on_change);
    }
}

/// Slots and cursor that changed since the last call.
pub fn changes(m: &mut OpenMenu, dim: &Dim, player: u64) -> (Vec<(u32, ItemStack)>, Option<ItemStack>) {
    let n = m.slots.len();
    m.sent_slots.resize(n, ItemStack { count: u32::MAX, ..ItemStack::EMPTY });
    let mut slots = Vec::new();
    for i in 0..n as u32 {
        let s = get(m, dim, player, i);
        if m.sent_slots[i as usize] != s {
            m.sent_slots[i as usize] = s;
            slots.push((i, crate::stack::for_client(s)));
        }
    }
    let cursor = (m.sent_cursor != m.cursor).then(|| {
        m.sent_cursor = m.cursor;
        crate::stack::for_client(m.cursor)
    });
    (slots, cursor)
}
