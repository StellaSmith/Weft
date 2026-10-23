//! Engine side of the mod ABI: the function table mods call, and the wrapper the engine uses to
//! call mod callbacks (profiling and fault isolation live there).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use glam::{DVec3, IVec3};
use voxide_abi::{self as abi, Callback, Host, ItemStack, ModId, Str};

use crate::content::{self, RawRegistrar, content, try_content};
use crate::registry;
use crate::server::Dim;
use crate::server::menu::MenuCall;

pub static HOST: Host = Host {
    size: size_of::<Host>(),
    log: host_log,
    reg_block,
    reg_item,
    reg_biome,
    reg_dimension,
    get_block,
    set_block,
    schedule_tick,
    world_time,
    random,
    find_block,
    find_item,
    block_of,
    state_flags,
    reg_entity,
    reg_component,
    reg_entity_hook,
    reg_recipe,
    reg_menu,
    reg_hud,
    set_player_menu,
    set_block_entity,
    light,
    daylight,
    entity_spawn,
    entity_remove,
    entity_get,
    entity_set,
    entity_walk,
    entity_look_at,
    entity_damage,
    entities_near,
    component,
    player_count,
    player_at,
    inv_size,
    inv_get,
    inv_set,
    inv_insert,
    selected_slot,
    spawn_item,
    block_entity,
    open_menu,
    menu_slots,
    menu_get,
    menu_set,
    menu_set_data,
    recipe_match,
    recipe_process,
    item_prop,
    item_info,
    item_has_tag,
    block_has_tag,
    ui_open: crate::client::modui::ui_open,
    ui_close: crate::client::modui::ui_close,
    ui_text: crate::client::modui::ui_text,
    client_player: crate::client::modui::client_player,
    client_component: crate::client::modui::client_component,
    client_entity: crate::client::modui::client_entity,
    client_menu_data: crate::client::modui::client_menu_data,
    find_component,
    energy_insert,
    energy_extract,
    reg_attach,
    reg_hud_element,
    reg_command,
    command_reply,
    is_creative,
    client_creative: crate::client::modui::client_creative,
    reg_ore,
    reg_creative_tab,
    reg_tag,
};

// ---------------------------------------------------------------- profiling per mod

pub const MAX_MODS: usize = 256;

pub struct ModStats {
    pub nanos: AtomicU64,
    pub calls: AtomicU64,
    pub faults: AtomicU32,
}

static STATS: [ModStats; MAX_MODS] =
    [const { ModStats { nanos: AtomicU64::new(0), calls: AtomicU64::new(0), faults: AtomicU32::new(0) } }; MAX_MODS];

pub fn stats(m: ModId) -> &'static ModStats {
    &STATS[(m.0 as usize).min(MAX_MODS - 1)]
}

/// Calls a mod callback. Time is charged to `owner`; a failing callback is logged with the mod's
/// name and the engine carries on.
pub fn invoke<A>(owner: ModId, cb: Callback, args: &mut A) -> bool {
    let Some(f) = cb.func else { return true };
    let start = Instant::now();
    let r = unsafe { f(cb.data, args as *mut A as *mut c_void) };
    let s = stats(owner);
    s.nanos.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    s.calls.fetch_add(1, Ordering::Relaxed);
    if r != 0 {
        let n = s.faults.fetch_add(1, Ordering::Relaxed);
        if n < 10 {
            let name = try_content().map_or("?".to_string(), |c| c.mod_name(owner).to_string());
            log::error!("callback of mod '{name}' failed (see its panic above), continuing");
        }
        return false;
    }
    true
}

// ---------------------------------------------------------------- world access from callbacks

/// `WorldCtx` pointers handed to mods point at one of these. Only valid during the callback.
pub struct CallCtx {
    pub dim: *mut Dim,
}

impl CallCtx {
    pub fn raw(&mut self) -> *mut abi::WorldCtx {
        self as *mut CallCtx as *mut abi::WorldCtx
    }
}

unsafe fn dim<'a>(w: *mut abi::WorldCtx) -> &'a mut Dim {
    unsafe { &mut *(*(w as *mut CallCtx)).dim }
}

fn ip(p: *const [i32; 3]) -> IVec3 {
    IVec3::from_array(unsafe { *p })
}

unsafe extern "C" fn get_block(w: *mut abi::WorldCtx, pos: *const [i32; 3]) -> u32 {
    unsafe { dim(w).get(ip(pos)).unwrap_or(content::AIR) }
}

unsafe extern "C" fn set_block(w: *mut abi::WorldCtx, pos: *const [i32; 3], state: u32) -> u32 {
    if state as usize >= content().states.len() {
        log::error!("mod tried to set invalid state {state}");
        return 0;
    }
    unsafe { dim(w).set(content(), ip(pos), state).is_some() as u32 }
}

unsafe extern "C" fn schedule_tick(w: *mut abi::WorldCtx, pos: *const [i32; 3], delay: u32) {
    unsafe { dim(w).schedule(ip(pos), delay) }
}

unsafe extern "C" fn world_time(w: *mut abi::WorldCtx) -> u64 {
    unsafe { dim(w).time }
}

unsafe extern "C" fn random(w: *mut abi::WorldCtx) -> u64 {
    unsafe { dim(w).random() }
}

unsafe extern "C" fn light(w: *mut abi::WorldCtx, pos: *const [i32; 3]) -> u32 {
    unsafe { dim(w).light(ip(pos)) }
}

unsafe extern "C" fn daylight(w: *mut abi::WorldCtx) -> f32 {
    unsafe { dim(w).daylight(content()) }
}

unsafe extern "C" fn entity_spawn(w: *mut abi::WorldCtx, kind: u32, pos: *const [f64; 3]) -> u64 {
    if kind as usize >= content().game.entities.len() {
        return 0;
    }
    unsafe { dim(w).spawn(content(), kind, DVec3::from_array(*pos)) }
}

unsafe extern "C" fn entity_remove(w: *mut abi::WorldCtx, id: u64) {
    unsafe { dim(w).entities.remove(id) }
}

unsafe extern "C" fn entity_get(w: *mut abi::WorldCtx, id: u64, out: *mut abi::EntityState) -> u32 {
    unsafe {
        match dim(w).entities.get(id) {
            Some(e) => {
                *out = e.state(content());
                1
            }
            None => 0,
        }
    }
}

unsafe extern "C" fn entity_set(w: *mut abi::WorldCtx, id: u64, s: *const abi::EntityState) {
    unsafe {
        let s = &*s;
        if let Some(e) = dim(w).entities.get_mut(id) {
            let pos = DVec3::from_array(s.pos);
            if pos.is_finite() {
                e.pos = pos;
            }
            let vel = DVec3::from_array(s.vel);
            if vel.is_finite() {
                e.vel = vel;
            }
            e.yaw = s.yaw;
            e.pitch = s.pitch;
            e.health = s.health.min(s.max_health.max(e.health));
        }
    }
}

unsafe extern "C" fn entity_walk(w: *mut abi::WorldCtx, id: u64, dir: *const [f32; 2], speed: f32, jump: u32) {
    unsafe {
        if let Some(e) = dim(w).entities.get_mut(id) {
            e.walk = Some((*dir, speed, jump != 0));
        }
    }
}

unsafe extern "C" fn entity_look_at(w: *mut abi::WorldCtx, id: u64, max: f64, liquids: u32, pos: *mut [i32; 3], face: *mut u32) -> u32 {
    unsafe {
        let c = content();
        let d = dim(w);
        let Some(e) = d.entities.get(id) else { return 0 };
        // Eyes at nine tenths of the height: 1.62 for players.
        let eye = e.pos + DVec3::Y * (c.game.entities[e.kind as usize].height as f64 * 0.9);
        let (sy, cy) = (e.yaw as f64).sin_cos();
        let (sp, cp) = (e.pitch as f64).sin_cos();
        let dir = DVec3::new(-sy * cp, sp, -cy * cp);
        let Some(hit) = crate::physics::raycast(c, &|p| d.get(p), eye, dir, max, liquids != 0) else { return 0 };
        *pos = hit.block.to_array();
        *face = hit.face as u32;
        hit.state + 1
    }
}

unsafe extern "C" fn entity_damage(w: *mut abi::WorldCtx, id: u64, amount: f32, source: u64, cause: u32) -> u32 {
    unsafe { dim(w).damage(content(), id, amount, source, cause) as u32 }
}

unsafe extern "C" fn entities_near(w: *mut abi::WorldCtx, pos: *const [f64; 3], radius: f64, kind: u32, out: *mut u64, max: usize) -> usize {
    unsafe {
        let ids = dim(w).entities.near(DVec3::from_array(*pos), radius, kind);
        for (i, id) in ids.iter().take(max).enumerate() {
            *out.add(i) = *id;
        }
        ids.len()
    }
}

unsafe extern "C" fn component(w: *mut abi::WorldCtx, id: u64, comp: u32) -> *mut u8 {
    unsafe {
        let d = dim(w);
        match d.entities.comps.get_mut(&(id, comp)) {
            Some(b) => {
                // Written through the pointer: assume dirty so owners get the new value.
                d.entities.dirty_comp.insert((id, comp));
                b.as_mut_ptr()
            }
            None => std::ptr::null_mut(),
        }
    }
}

unsafe extern "C" fn player_count(w: *mut abi::WorldCtx) -> u32 {
    unsafe { dim(w).entities.list.iter().filter(|e| visible_player(e)).count() as u32 }
}

unsafe extern "C" fn player_at(w: *mut abi::WorldCtx, index: u32) -> u64 {
    unsafe { dim(w).entities.list.iter().filter(|e| visible_player(e)).nth(index as usize).map_or(0, |e| e.id) }
}

/// Players mods get to see: spectators are left out, so mobs neither chase nor spawn around them.
fn visible_player(e: &crate::server::entity::Entity) -> bool {
    e.player && !e.removed && e.mode != crate::world::GameMode::Spectator
}

unsafe extern "C" fn inv_size(w: *mut abi::WorldCtx, owner: u64) -> u32 {
    unsafe { dim(w).entities.inventories.get(&owner).map_or(0, |i| i.len() as u32) }
}

unsafe extern "C" fn inv_get(w: *mut abi::WorldCtx, owner: u64, slot: u32) -> ItemStack {
    unsafe { dim(w).entities.inventories.get(&owner).and_then(|i| i.get(slot as usize)).copied().unwrap_or_default() }
}

unsafe extern "C" fn inv_set(w: *mut abi::WorldCtx, owner: u64, slot: u32, stack: ItemStack) {
    unsafe { dim(w).entities.set_slot(owner, slot, stack) }
}

unsafe extern "C" fn inv_insert(w: *mut abi::WorldCtx, owner: u64, stack: ItemStack) -> ItemStack {
    unsafe { dim(w).entities.insert_item(content(), owner, stack) }
}

unsafe extern "C" fn selected_slot(w: *mut abi::WorldCtx, player: u64) -> u32 {
    unsafe { dim(w).entities.get(player).map_or(0, |e| e.selected) }
}

unsafe extern "C" fn spawn_item(w: *mut abi::WorldCtx, pos: *const [f64; 3], stack: ItemStack) {
    unsafe {
        dim(w).spawn_item(content(), DVec3::from_array(*pos), stack);
    }
}

unsafe extern "C" fn block_entity(w: *mut abi::WorldCtx, pos: *const [i32; 3]) -> u64 {
    unsafe { dim(w).entities.anchors.get(&ip(pos)).copied().unwrap_or(0) }
}

unsafe extern "C" fn open_menu(w: *mut abi::WorldCtx, player: u64, menu: u32, owner: u64) {
    unsafe { dim(w).events.push(crate::server::DimEvent::OpenMenu { player, menu, owner }) }
}

unsafe extern "C" fn find_component(name: Str) -> u32 {
    let name = unsafe { name.as_str() };
    try_content().and_then(|c| c.game.component(name)).unwrap_or(u32::MAX)
}

unsafe extern "C" fn energy_insert(w: *mut abi::WorldCtx, id: u64, amount: i64, simulate: u32) -> i64 {
    unsafe { crate::server::energy::insert(&mut dim(w).entities, content(), id, amount, simulate != 0) }
}

unsafe extern "C" fn energy_extract(w: *mut abi::WorldCtx, id: u64, amount: i64, simulate: u32) -> i64 {
    unsafe { crate::server::energy::extract(&mut dim(w).entities, content(), id, amount, simulate != 0) }
}

// ---------------------------------------------------------------- menus

unsafe fn menu<'a>(m: *mut abi::MenuCtx) -> &'a mut MenuCall {
    unsafe { &mut *(m as *mut MenuCall) }
}

unsafe extern "C" fn menu_slots(m: *mut abi::MenuCtx, owner: u64, start: u32, count: u32, flags: u32) {
    unsafe { menu(m).add_slots(owner, start, count, flags) }
}

unsafe extern "C" fn menu_get(m: *mut abi::MenuCtx, slot: u32) -> ItemStack {
    unsafe { menu(m).get(slot) }
}

unsafe extern "C" fn menu_set(m: *mut abi::MenuCtx, slot: u32, stack: ItemStack) {
    unsafe { menu(m).set(slot, stack) }
}

unsafe extern "C" fn menu_set_data(m: *mut abi::MenuCtx, index: u32, value: i32) {
    unsafe { menu(m).set_data(index, value) }
}

// ---------------------------------------------------------------- lookups

fn tags(item: u32, tag: &str) -> bool {
    content().item_has_tag(item, tag)
}

unsafe extern "C" fn recipe_match(kind: Str, grid: *const ItemStack, width: u32, height: u32) -> ItemStack {
    let kind = unsafe { kind.as_str() };
    let cells: Vec<u32> = (0..(width * height) as usize).map(|i| unsafe { (*grid.add(i)).item }).collect();
    content()
        .game
        .match_grid(kind, &cells, width, height, &tags)
        .map_or(ItemStack::EMPTY, |r| ItemStack::new(r.output, r.count))
}

unsafe extern "C" fn recipe_process(kind: Str, input: u32, time_out: *mut u32) -> ItemStack {
    let kind = unsafe { kind.as_str() };
    match content().game.process(kind, input, &tags) {
        Some(r) => {
            if !time_out.is_null() {
                unsafe { *time_out = r.time }
            }
            ItemStack::new(r.output, r.count)
        }
        None => ItemStack::EMPTY,
    }
}

unsafe extern "C" fn item_prop(item: u32, key: Str, len: *mut usize) -> *const u8 {
    let key = unsafe { key.as_str() };
    match content().item_prop(item, key) {
        Some(b) => {
            if !len.is_null() {
                unsafe { *len = b.len() }
            }
            b.as_ptr()
        }
        None => std::ptr::null(),
    }
}

unsafe extern "C" fn item_info(item: u32, max_stack: *mut u32, durability: *mut u32) -> u32 {
    let Some(d) = content().items.get(item as usize) else { return 0 };
    unsafe {
        if !max_stack.is_null() {
            *max_stack = d.max_stack;
        }
        if !durability.is_null() {
            *durability = d.durability;
        }
    }
    d.block.unwrap_or(0)
}

unsafe extern "C" fn block_has_tag(state: u32, tag: Str) -> u32 {
    content().block_has_tag(state, unsafe { tag.as_str() }) as u32
}

unsafe extern "C" fn item_has_tag(item: u32, tag: Str) -> u32 {
    content().item_has_tag(item, unsafe { tag.as_str() }) as u32
}

unsafe extern "C" fn find_block(name: Str) -> u32 {
    let name = unsafe { name.as_str() };
    try_content().and_then(|c| c.find_state(name)).unwrap_or(u32::MAX)
}

unsafe extern "C" fn find_item(name: Str) -> u32 {
    let name = unsafe { name.as_str() };
    try_content().and_then(|c| c.item_names.get(name).copied()).unwrap_or(u32::MAX)
}

unsafe extern "C" fn block_of(state: u32) -> u32 {
    let c = content();
    c.states.get(state as usize).map_or(0, |s| c.blocks[s.block as usize].first_state)
}

unsafe extern "C" fn state_flags(state: u32) -> u32 {
    content().states.get(state as usize).map_or(0, |s| s.flags)
}

// ---------------------------------------------------------------- registration

unsafe extern "C" fn host_log(me: ModId, level: u32, msg: Str) {
    let msg = unsafe { msg.as_str() };
    let name = crate::modloader::mod_id_name(me);
    let target = format!("mod:{name}");
    let lvl = match level {
        abi::level::ERROR => log::Level::Error,
        abi::level::WARN => log::Level::Warn,
        abi::level::DEBUG => log::Level::Debug,
        _ => log::Level::Info,
    };
    log::log!(target: &target, lvl, "{msg}");
}

unsafe fn reg<'a>(r: *mut abi::Registrar) -> &'a mut RawRegistrar {
    unsafe { &mut *(r as *mut RawRegistrar) }
}

unsafe extern "C" fn reg_block(r: *mut abi::Registrar, d: *const abi::BlockDesc) {
    unsafe {
        let b = content::copy_block(&*d);
        let r = reg(r);
        if b.states.is_empty() {
            r.errors.push(format!("block {} has no states", b.name));
            return;
        }
        r.blocks.push(b);
    }
}

unsafe extern "C" fn reg_item(r: *mut abi::Registrar, d: *const abi::ItemDesc) {
    unsafe {
        let r = reg(r);
        let mut item = content::copy_item(&*d);
        item.seq = r.items.len() as u32;
        r.items.push(item);
    }
}

unsafe extern "C" fn reg_biome(r: *mut abi::Registrar, d: *const abi::BiomeDesc) {
    unsafe {
        let d = &*d;
        reg(r).biomes.push((
            content::BiomeDef {
                name: d.name.as_str().to_string(),
                grass: d.grass,
                foliage: d.foliage,
                water: d.water,
                temperature: d.temperature,
                rainfall: d.rainfall,
            },
            d.cell as usize,
        ));
    }
}

unsafe extern "C" fn reg_dimension(r: *mut abi::Registrar, d: *const abi::DimensionDesc) {
    unsafe {
        let r = reg(r);
        let owner = r.owner;
        r.dims.push(content::copy_dimension(owner, &*d));
    }
}

unsafe extern "C" fn reg_entity(r: *mut abi::Registrar, d: *const abi::EntityDesc) {
    unsafe {
        let r = reg(r);
        let owner = r.owner;
        r.game.entities.push(registry::copy_entity(owner, &*d));
    }
}

unsafe extern "C" fn reg_component(r: *mut abi::Registrar, d: *const abi::ComponentDesc) {
    unsafe {
        let d = &*d;
        let mut default = d.default.as_slice().to_vec();
        default.resize(d.size as usize, 0);
        reg(r).game.components.push(registry::RawComponent {
            cell: d.cell as usize,
            def: registry::ComponentDef { name: d.name.as_str().to_string(), size: d.size, flags: d.flags, default },
            attach_to: d.attach_to.as_slice().iter().map(|s| s.as_str().to_string()).collect(),
        });
    }
}

unsafe extern "C" fn reg_entity_hook(r: *mut abi::Registrar, kind: Str, hook: u32, cb: Callback) {
    unsafe {
        let r = reg(r);
        let owner = r.owner;
        r.game.hooks.push((owner, kind.as_str().to_string(), hook, cb));
    }
}

unsafe extern "C" fn reg_recipe(r: *mut abi::Registrar, d: *const abi::RecipeDesc) {
    unsafe { reg(r).game.recipes.push(registry::copy_recipe(&*d)) }
}

unsafe extern "C" fn reg_menu(r: *mut abi::Registrar, d: *const abi::MenuDesc) {
    unsafe {
        let r = reg(r);
        let owner = r.owner;
        r.game.menus.push(registry::copy_menu(owner, &*d));
    }
}

unsafe extern "C" fn reg_hud(r: *mut abi::Registrar, cb: Callback) {
    unsafe {
        let r = reg(r);
        let owner = r.owner;
        let name = format!("{}:hud{}", crate::modloader::mod_id_name(owner), r.game.huds.len());
        r.game.huds.push(registry::HudElement { name, anchor: abi::hud_anchor::HOTBAR_LEFT.into(), order: 0, owner, cb });
    }
}

unsafe extern "C" fn reg_tag(r: *mut abi::Registrar, tag: Str, item: Str) {
    unsafe { reg(r).game.tags.push((tag.as_str().to_string(), item.as_str().to_string())) }
}

unsafe extern "C" fn reg_creative_tab(r: *mut abi::Registrar, name: Str, icon: Str) {
    unsafe {
        let r = reg(r);
        r.game.creative_tabs.push((name.as_str().to_string(), icon.as_str().to_string()));
    }
}

unsafe extern "C" fn reg_ore(r: *mut abi::Registrar, d: *const abi::OreDesc) {
    unsafe {
        let r = reg(r);
        let d = &*d;
        r.game.ores.push(registry::RawOre {
            dimension: d.dimension.as_str().to_string(),
            block: d.block.as_str().to_string(),
            replace: d.replace.as_str().split_whitespace().map(str::to_string).collect(),
            min_y: d.min_y,
            max_y: d.max_y,
            veins: d.veins,
            size: d.size,
        });
    }
}

unsafe extern "C" fn reg_command(r: *mut abi::Registrar, name: Str, usage: Str, cb: Callback) {
    unsafe {
        let r = reg(r);
        let owner = r.owner;
        r.game.commands.push((owner, name.as_str().to_string(), usage.as_str().to_string(), cb));
    }
}

unsafe extern "C" fn command_reply(args: *mut abi::CommandArgs, text: Str) {
    unsafe {
        let reply = &mut *((*args).reply as *mut String);
        if !reply.is_empty() {
            reply.push('\n');
        }
        reply.push_str(text.as_str());
    }
}

unsafe extern "C" fn is_creative(w: *mut abi::WorldCtx, entity: u64) -> u32 {
    unsafe { dim(w).entities.get(entity).is_some_and(|e| e.mode.invulnerable()) as u32 }
}

unsafe extern "C" fn reg_hud_element(r: *mut abi::Registrar, name: Str, anchor: Str, order: i32, cb: Callback) {
    unsafe {
        let r = reg(r);
        let owner = r.owner;
        r.game.huds.push(registry::HudElement { name: name.as_str().to_string(), anchor: anchor.as_str().to_string(), order, owner, cb });
    }
}

unsafe extern "C" fn set_player_menu(r: *mut abi::Registrar, menu: Str) {
    unsafe { reg(r).game.player_menu = Some(menu.as_str().to_string()) }
}

unsafe extern "C" fn reg_attach(r: *mut abi::Registrar, kind: Str, component: Str) {
    unsafe { reg(r).game.attach.push((kind.as_str().to_string(), component.as_str().to_string())) }
}

unsafe extern "C" fn set_block_entity(r: *mut abi::Registrar, block: Str, entity: Str, slots: u32) {
    unsafe { reg(r).game.block_entities.push((block.as_str().to_string(), entity.as_str().to_string(), slots)) }
}
