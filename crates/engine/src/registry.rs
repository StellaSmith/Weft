//! Gameplay registries: entity kinds, components, menus, recipes, HUD parts. Frozen together with
//! blocks and items, ids again assigned by sorted name.

use std::sync::atomic::{AtomicU32, Ordering};

use rustc_hash::FxHashMap;
use voxide_abi::{self as abi, Callback, ModId, entity_flags as ef, part_role};

pub const ENGINE: ModId = ModId(u32::MAX);
pub const PLAYER: &str = "core:player";

#[derive(Clone)]
pub struct HudElement {
    pub name: String,
    pub anchor: String,
    pub order: i32,
    pub owner: ModId,
    pub cb: Callback,
}
pub const ITEM: &str = "core:item";

#[derive(Clone)]
pub struct Cuboid {
    pub origin: [f32; 3],
    pub size: [f32; 3],
    pub uv: [f32; 2],
    pub inflate: f32,
}

#[derive(Clone)]
pub struct Part {
    pub pivot: [f32; 3],
    pub role: u32,
    pub cuboids: Vec<Cuboid>,
    pub rest: [f32; 3],
}

pub struct EntityKind {
    pub name: String,
    pub owner: ModId,
    pub width: f32,
    pub height: f32,
    pub eye: f32,
    pub max_health: f32,
    pub speed: f32,
    pub flags: u32,
    pub category: u32,
    pub texture: String,
    pub texture_size: [u32; 2],
    pub parts: Vec<Part>,
    /// (owner, callback) lists: the kind's own callback first, then hooks from other mods.
    pub on_tick: Vec<(ModId, Callback)>,
    pub on_hurt: Vec<(ModId, Callback)>,
    pub on_death: Vec<(ModId, Callback)>,
    pub on_interact: Vec<(ModId, Callback)>,
    /// Components attached at spawn.
    pub components: Vec<u32>,
    /// Size of the inventory created with it (block entities), 0 = none.
    pub slots: u32,
}

impl EntityKind {
    pub fn is(&self, f: u32) -> bool {
        self.flags & f != 0
    }
}

pub struct ComponentDef {
    pub name: String,
    pub size: u32,
    pub flags: u32,
    pub default: Vec<u8>,
}

pub struct MenuDef {
    pub name: String,
    pub owner: ModId,
    pub temp_slots: u32,
    pub data_count: u32,
    pub setup: Option<Callback>,
    pub on_change: Option<Callback>,
    pub on_take: Option<Callback>,
    pub on_tick: Option<Callback>,
    pub screen: Option<Callback>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ingredient {
    Empty,
    Item(u32),
    Tag(String),
}

impl Ingredient {
    pub fn matches(&self, item: u32, tags: &dyn Fn(u32, &str) -> bool) -> bool {
        match self {
            Ingredient::Empty => item == 0,
            Ingredient::Item(i) => *i == item,
            Ingredient::Tag(t) => item != 0 && tags(item, t),
        }
    }
}

pub struct Recipe {
    /// Shaped: width, height, cells (row major).
    pub shaped: Option<(u32, u32, Vec<Ingredient>)>,
    pub inputs: Vec<Ingredient>,
    pub output: u32,
    pub count: u32,
    pub time: u32,
}

#[derive(Default)]
pub struct Game {
    pub entities: Vec<EntityKind>,
    pub entity_names: FxHashMap<String, u32>,
    pub components: Vec<ComponentDef>,
    pub component_names: FxHashMap<String, u32>,
    pub menus: Vec<MenuDef>,
    pub menu_names: FxHashMap<String, u32>,
    pub recipes: FxHashMap<String, Vec<Recipe>>,
    pub huds: Vec<HudElement>,
    pub player_menu: Option<u32>,
    /// Block index -> (entity kind, inventory slots).
    pub block_entities: FxHashMap<u32, u32>,
    pub player_kind: u32,
    pub item_kind: u32,
    pub energy: u32,
}

// ---------------------------------------------------------------- raw registration

pub struct RawEntity {
    pub name: String,
    pub cell: usize,
    pub desc: EntityKind,
    pub replace: bool,
}

pub struct RawComponent {
    pub cell: usize,
    pub def: ComponentDef,
    pub attach_to: Vec<String>,
}

pub struct RawRecipe {
    pub kind: String,
    pub pattern: Vec<String>,
    pub keys: Vec<(char, String)>,
    pub inputs: Vec<String>,
    pub output: String,
    pub count: u32,
    pub time: u32,
}

#[derive(Default)]
pub struct RawGame {
    pub entities: Vec<RawEntity>,
    pub components: Vec<RawComponent>,
    pub hooks: Vec<(ModId, String, u32, Callback)>,
    pub recipes: Vec<RawRecipe>,
    pub menus: Vec<(usize, MenuDef)>,
    pub huds: Vec<HudElement>,
    /// (owner, name, usage, callback).
    pub commands: Vec<(ModId, String, String, Callback)>,
    pub player_menu: Option<String>,
    pub block_entities: Vec<(String, String, u32)>,
    pub attach: Vec<(String, String)>,
    pub ores: Vec<RawOre>,
}

pub struct RawOre {
    pub dimension: String,
    pub block: String,
    pub replace: Vec<String>,
    pub min_y: i32,
    pub max_y: i32,
    pub veins: u32,
    pub size: u32,
}

fn opt(c: Callback) -> Option<Callback> {
    c.is_some().then_some(c)
}

pub(crate) unsafe fn copy_entity(owner: ModId, d: &abi::EntityDesc) -> RawEntity {
    unsafe {
        let parts = d
            .parts
            .as_slice()
            .iter()
            .map(|p| Part {
                pivot: p.pivot,
                role: p.role,
                rest: p.rest,
                cuboids: p
                    .cuboids
                    .as_slice()
                    .iter()
                    .map(|c| Cuboid { origin: c.origin, size: c.size, uv: c.uv, inflate: c.inflate })
                    .collect(),
            })
            .collect();
        let one = |c: Callback| opt(c).map(|c| vec![(owner, c)]).unwrap_or_default();
        RawEntity {
            name: d.name.as_str().to_string(),
            cell: d.cell as usize,
            replace: d.replace != 0,
            desc: EntityKind {
                name: d.name.as_str().to_string(),
                owner,
                width: d.width,
                height: d.height,
                eye: d.eye,
                max_health: d.max_health,
                speed: d.speed,
                flags: d.flags,
                category: d.category,
                texture: d.texture.as_str().to_string(),
                texture_size: d.texture_size,
                parts,
                on_tick: one(d.on_tick),
                on_hurt: one(d.on_hurt),
                on_death: one(d.on_death),
                on_interact: one(d.on_interact),
                components: Vec::new(),
                slots: 0,
            },
        }
    }
}

pub(crate) unsafe fn copy_menu(owner: ModId, d: &abi::MenuDesc) -> (usize, MenuDef) {
    unsafe {
        (
            d.cell as usize,
            MenuDef {
                name: d.name.as_str().to_string(),
                owner,
                temp_slots: d.temp_slots,
                data_count: d.data_count,
                setup: opt(d.setup),
                on_change: opt(d.on_change),
                on_take: opt(d.on_take),
                on_tick: opt(d.on_tick),
                screen: opt(d.screen),
            },
        )
    }
}

pub(crate) unsafe fn copy_recipe(d: &abi::RecipeDesc) -> RawRecipe {
    unsafe {
        RawRecipe {
            kind: d.kind.as_str().to_string(),
            pattern: d.pattern.as_slice().iter().map(|s| s.as_str().to_string()).collect(),
            keys: d
                .keys
                .as_slice()
                .iter()
                .map(|k| (char::from_u32(k.ch).unwrap_or(' '), k.item.as_str().to_string()))
                .collect(),
            inputs: d.inputs.as_slice().iter().map(|s| s.as_str().to_string()).collect(),
            output: d.output.as_str().to_string(),
            count: d.count.max(1),
            time: d.time,
        }
    }
}

fn write_cell(cell: usize, id: u32) {
    if cell != 0 {
        unsafe { (*(cell as *const AtomicU32)).store(id, Ordering::Relaxed) }
    }
}

fn builtin(name: &str, width: f32, height: f32, flags: u32) -> EntityKind {
    EntityKind {
        name: name.into(),
        owner: ENGINE,
        width,
        height,
        eye: height * 0.9,
        max_health: 20.0,
        speed: 4.3,
        flags,
        category: 0,
        texture: String::new(),
        texture_size: [64, 32],
        parts: Vec::new(),
        on_tick: Vec::new(),
        on_hurt: Vec::new(),
        on_death: Vec::new(),
        on_interact: Vec::new(),
        components: Vec::new(),
        slots: 0,
    }
}

/// Classic player model so other players are visible without any mod providing one.
fn player_parts() -> Vec<Part> {
    let c = |o: [f32; 3], s: [f32; 3], uv: [f32; 2]| Cuboid { origin: o, size: s, uv, inflate: 0.0 };
    vec![
        Part { pivot: [0.0, 24.0, 0.0], role: part_role::HEAD, rest: [0.0; 3], cuboids: vec![c([-4.0, 0.0, -4.0], [8.0, 8.0, 8.0], [0.0, 0.0])] },
        Part { pivot: [0.0, 24.0, 0.0], role: part_role::STATIC, rest: [0.0; 3], cuboids: vec![c([-4.0, -12.0, -2.0], [8.0, 12.0, 4.0], [16.0, 16.0])] },
        Part { pivot: [-6.0, 22.0, 0.0], role: part_role::ARM_LEFT, rest: [0.0; 3], cuboids: vec![c([-2.0, -10.0, -2.0], [4.0, 12.0, 4.0], [40.0, 16.0])] },
        Part { pivot: [6.0, 22.0, 0.0], role: part_role::ARM_RIGHT, rest: [0.0; 3], cuboids: vec![c([-2.0, -10.0, -2.0], [4.0, 12.0, 4.0], [40.0, 16.0])] },
        Part { pivot: [-2.0, 12.0, 0.0], role: part_role::LEG_LEFT, rest: [0.0; 3], cuboids: vec![c([-2.0, -12.0, -2.0], [4.0, 12.0, 4.0], [0.0, 16.0])] },
        Part { pivot: [2.0, 12.0, 0.0], role: part_role::LEG_RIGHT, rest: [0.0; 3], cuboids: vec![c([-2.0, -12.0, -2.0], [4.0, 12.0, 4.0], [0.0, 16.0])] },
    ]
}

pub fn freeze(
    raws: Vec<RawGame>,
    block_names: &FxHashMap<String, u32>,
    item_names: &FxHashMap<String, u32>,
    errors: &mut Vec<String>,
) -> Game {
    let mut g = Game::default();
    let mut entities: Vec<RawEntity> = Vec::new();
    let mut components: Vec<RawComponent> = Vec::new();
    let mut hooks = Vec::new();
    let mut recipes = Vec::new();
    let mut menus = Vec::new();
    let mut player_menu = None;
    let mut block_entities = Vec::new();
    let mut replaced_cells: Vec<(usize, String)> = Vec::new();
    let mut attach: Vec<(String, String)> = Vec::new();
    for r in raws {
        entities.extend(r.entities);
        components.extend(r.components);
        hooks.extend(r.hooks);
        recipes.extend(r.recipes);
        menus.extend(r.menus);
        g.huds.extend(r.huds);
        if r.player_menu.is_some() {
            player_menu = r.player_menu;
        }
        block_entities.extend(r.block_entities);
        attach.extend(r.attach);
    }
    let mut player = builtin(PLAYER, 0.3, 1.8, ef::FALL_DAMAGE | ef::PUSHABLE);
    player.eye = 1.62;
    player.texture = "core:entity/player".into();
    player.parts = player_parts();
    entities.push(RawEntity { name: PLAYER.into(), cell: 0, desc: player, replace: false });
    entities.push(RawEntity { name: ITEM.into(), cell: 0, desc: builtin(ITEM, 0.125, 0.25, ef::INVULNERABLE), replace: false });
    entities.sort_by(|a, b| a.name.cmp(&b.name).then(a.replace.cmp(&b.replace)));
    let mut kept: Vec<RawEntity> = Vec::with_capacity(entities.len());
    for e in entities {
        match kept.last() {
            Some(prev) if prev.name == e.name && e.replace => {
                // The replaced kind's handle must resolve to the replacement too.
                if prev.cell != 0 {
                    replaced_cells.push((prev.cell, e.name.clone()));
                }
                kept.pop();
                kept.push(e);
            }
            Some(prev) if prev.name == e.name => {
                errors.push(format!("entity {} registered twice", e.name));
            }
            _ => kept.push(e),
        }
    }
    let entities = kept;
    for (i, e) in entities.into_iter().enumerate() {
        write_cell(e.cell, i as u32);
        g.entity_names.insert(e.name.clone(), i as u32);
        g.entities.push(e.desc);
    }
    for (cell, name) in replaced_cells {
        write_cell(cell, g.entity_names[&name]);
    }
    g.player_kind = g.entity_names[PLAYER];
    g.item_kind = g.entity_names[ITEM];

    components.push(RawComponent {
        cell: 0,
        def: ComponentDef {
            name: "core:energy".into(),
            size: size_of::<voxide_abi::Energy>() as u32,
            flags: voxide_abi::component_flags::SAVE,
            default: vec![0; size_of::<voxide_abi::Energy>()],
        },
        attach_to: Vec::new(),
    });
    components.sort_by(|a, b| a.def.name.cmp(&b.def.name));
    for (i, c) in components.into_iter().enumerate() {
        write_cell(c.cell, i as u32);
        for k in &c.attach_to {
            match g.entity_names.get(k) {
                Some(&e) => g.entities[e as usize].components.push(i as u32),
                None => errors.push(format!("component {} attaches to unknown entity {k}", c.def.name)),
            }
        }
        g.component_names.insert(c.def.name.clone(), i as u32);
        g.components.push(c.def);
    }
    g.energy = g.component_names["core:energy"];
    for (kind, comp) in attach {
        match (g.entity_names.get(&kind), g.component_names.get(&comp)) {
            (Some(&k), Some(&cid)) => g.entities[k as usize].components.push(cid),
            _ => errors.push(format!("cannot attach {comp} to {kind}: unknown entity or component")),
        }
    }

    // Hooks run after the kind's own callback, in mod order.
    hooks.sort_by_key(|h: &(ModId, String, u32, Callback)| h.0.0);
    for (owner, kind, hook, cb) in hooks {
        let Some(&e) = g.entity_names.get(&kind) else {
            errors.push(format!("hook on unknown entity {kind}"));
            continue;
        };
        let k = &mut g.entities[e as usize];
        match hook {
            0 => k.on_tick.push((owner, cb)),
            1 => k.on_hurt.push((owner, cb)),
            2 => k.on_death.push((owner, cb)),
            _ => k.on_interact.push((owner, cb)),
        }
    }

    menus.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    for (i, (cell, m)) in menus.into_iter().enumerate() {
        write_cell(cell, i as u32);
        g.menu_names.insert(m.name.clone(), i as u32);
        g.menus.push(m);
    }
    g.player_menu = player_menu.and_then(|n| {
        let m = g.menu_names.get(&n).copied();
        if m.is_none() {
            errors.push(format!("player menu {n} is not registered"));
        }
        m
    });

    for (block, entity, slots) in block_entities {
        match (block_names.get(&block), g.entity_names.get(&entity)) {
            (Some(&b), Some(&e)) => {
                g.block_entities.insert(b, e);
                g.entities[e as usize].slots = slots;
            }
            _ => errors.push(format!("block entity {block} -> {entity}: unknown block or entity")),
        }
    }

    let ing = |s: &str, errors: &mut Vec<String>| -> Ingredient {
        if let Some(t) = s.strip_prefix('#') {
            Ingredient::Tag(t.to_string())
        } else if s.is_empty() {
            Ingredient::Empty
        } else {
            match item_names.get(s) {
                Some(&i) => Ingredient::Item(i),
                None => {
                    errors.push(format!("recipe uses unknown item {s}"));
                    Ingredient::Empty
                }
            }
        }
    };
    for r in recipes {
        let Some(&output) = item_names.get(&r.output) else {
            errors.push(format!("recipe makes unknown item {}", r.output));
            continue;
        };
        let shaped = if r.pattern.is_empty() {
            None
        } else {
            let h = r.pattern.len() as u32;
            let w = r.pattern.iter().map(|p| p.chars().count()).max().unwrap_or(0) as u32;
            let mut cells = Vec::new();
            for row in &r.pattern {
                let chars: Vec<char> = row.chars().collect();
                for x in 0..w as usize {
                    let ch = chars.get(x).copied().unwrap_or(' ');
                    let cell = if ch == ' ' {
                        Ingredient::Empty
                    } else {
                        match r.keys.iter().find(|k| k.0 == ch) {
                            Some((_, item)) => ing(item, errors),
                            None => {
                                errors.push(format!("recipe for {}: key '{ch}' not defined", r.output));
                                Ingredient::Empty
                            }
                        }
                    };
                    cells.push(cell);
                }
            }
            Some((w, h, cells))
        };
        let inputs = r.inputs.iter().map(|s| ing(s, errors)).collect();
        g.recipes.entry(r.kind).or_default().push(Recipe { shaped, inputs, output, count: r.count, time: r.time });
    }
    g
}

impl Game {
    pub fn component(&self, name: &str) -> Option<u32> {
        self.component_names.get(name).copied()
    }

    /// Finds the recipe whose pattern fits the grid (any position, mirrored allowed) or whose
    /// shapeless inputs match the grid contents exactly.
    pub fn match_grid(&self, kind: &str, grid: &[u32], w: u32, h: u32, tags: &dyn Fn(u32, &str) -> bool) -> Option<&Recipe> {
        let list = self.recipes.get(kind)?;
        // Trim the grid to the bounding box of its items.
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if grid[(y * w + x) as usize] != 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 == 0 {
            return None;
        }
        let (bw, bh) = (x1 - x0, y1 - y0);
        let at = |x: u32, y: u32| grid[((y0 + y) * w + x0 + x) as usize];
        let items: Vec<u32> = grid.iter().copied().filter(|&i| i != 0).collect();
        list.iter().find(|r| match &r.shaped {
            Some((rw, rh, cells)) => {
                if *rw != bw || *rh != bh {
                    return false;
                }
                let fits = |mirror: bool| {
                    (0..bh).all(|y| {
                        (0..bw).all(|x| {
                            let cx = if mirror { bw - 1 - x } else { x };
                            cells[(y * rw + cx) as usize].matches(at(x, y), tags)
                        })
                    })
                };
                fits(false) || fits(true)
            }
            None => {
                if r.inputs.len() != items.len() {
                    return false;
                }
                let mut used = vec![false; items.len()];
                r.inputs.iter().all(|ing| {
                    let found = items.iter().enumerate().find(|(i, it)| !used[*i] && ing.matches(**it, tags));
                    match found {
                        Some((i, _)) => {
                            used[i] = true;
                            true
                        }
                        None => false,
                    }
                })
            }
        })
    }

    pub fn process(&self, kind: &str, input: u32, tags: &dyn Fn(u32, &str) -> bool) -> Option<&Recipe> {
        self.recipes.get(kind)?.iter().find(|r| r.inputs.first().is_some_and(|i| i.matches(input, tags)))
    }
}
