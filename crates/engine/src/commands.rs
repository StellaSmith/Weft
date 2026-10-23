//! Command syntax shared by the server (parsing, dispatch) and the client (completion). A command
//! describes its arguments with one usage string: literal words, `<name:type>` required and
//! `[name:type]` optional parameters, alternatives separated by `|`, for example
//! `<pos:pos> | <target:player>` or `<item:item> [count:int]`.
//!
//! Types: int, float, word, text (rest of the line), item, block, entity, dim, player, pos (three
//! coordinates, `~` relative to the player).

use glam::DVec3;
use voxide_abi::{Callback, ModId};

use crate::content::Content;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Literal(String),
    Int,
    Float,
    Word,
    Text,
    Item,
    Block,
    Entity,
    Dim,
    Player,
    Pos,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub kind: Kind,
    pub optional: bool,
}

pub type Alt = Vec<Param>;

pub fn parse_usage(usage: &str) -> Result<Vec<Alt>, String> {
    let mut alts = Vec::new();
    for alt in usage.split('|') {
        let mut params = Vec::new();
        for tok in alt.split_whitespace() {
            let (open, close, optional) = match tok.chars().next() {
                Some('<') => ('<', '>', false),
                Some('[') => ('[', ']', true),
                _ => {
                    params.push(Param { name: tok.to_string(), kind: Kind::Literal(tok.to_string()), optional: false });
                    continue;
                }
            };
            let inner = tok.strip_prefix(open).and_then(|t| t.strip_suffix(close)).ok_or_else(|| format!("bad parameter {tok}"))?;
            let (name, ty) = inner.split_once(':').ok_or_else(|| format!("parameter {tok} needs a type"))?;
            let kind = match ty {
                "int" => Kind::Int,
                "float" => Kind::Float,
                "word" => Kind::Word,
                "text" => Kind::Text,
                "item" => Kind::Item,
                "block" => Kind::Block,
                "entity" => Kind::Entity,
                "dim" => Kind::Dim,
                "player" => Kind::Player,
                "pos" => Kind::Pos,
                _ => return Err(format!("unknown parameter type {ty}")),
            };
            params.push(Param { name: name.to_string(), kind, optional });
        }
        alts.push(params);
    }
    Ok(alts)
}

/// Parameter names (and literal words) in order of first appearance across alternatives: the
/// slots of `Args` and of the values handed to mods.
pub fn slots(alts: &[Alt]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for p in alts.iter().flatten() {
        if !names.contains(&p.name) {
            names.push(p.name.clone());
        }
    }
    names
}

#[derive(Clone, Debug)]
pub enum Value {
    Int(i64),
    Float(f64),
    Text(String),
    /// Registry id and full name (items, block states, entity kinds, dimensions).
    Id(u32, String),
    Pos(DVec3),
}

pub struct Args {
    pub names: Vec<String>,
    pub values: Vec<Option<Value>>,
}

impl Args {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.names.iter().position(|n| n == name).and_then(|i| self.values[i].as_ref())
    }
    pub fn int(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            Value::Text(t) | Value::Id(_, t) => Some(t),
            _ => None,
        }
    }
    pub fn id(&self, name: &str) -> Option<u32> {
        match self.get(name)? {
            Value::Id(i, _) => Some(*i),
            _ => None,
        }
    }
    pub fn pos(&self, name: &str) -> Option<DVec3> {
        match self.get(name)? {
            Value::Pos(p) => Some(*p),
            _ => None,
        }
    }
}

/// A full registry name from what the player typed: exact `ns:name`, or a bare name matching
/// exactly one entry's path (the base game's wins ties).
pub fn resolve<'a>(names: impl Iterator<Item = &'a String> + Clone, typed: &str) -> Option<String> {
    if typed.contains(':') {
        return names.into_iter().find(|n| n.as_str() == typed).cloned();
    }
    let matches: Vec<&String> = names.filter(|n| n.split_once(':').is_some_and(|(_, p)| p == typed)).collect();
    match matches.as_slice() {
        [one] => Some((*one).clone()),
        [] => None,
        many => many.iter().find(|n| n.starts_with("base:")).map(|n| (*n).clone()),
    }
}

fn coordinate(tok: &str, base: f64) -> Option<f64> {
    match tok.strip_prefix('~') {
        Some("") => Some(base),
        Some(rest) => rest.parse::<f64>().ok().map(|v| base + v),
        None => tok.parse().ok(),
    }
}

fn parse_alt(c: &Content, alt: &Alt, tokens: &[&str], here: DVec3, players: &[String], slots: &[String]) -> Result<Args, String> {
    let mut values: Vec<Option<Value>> = vec![None; slots.len()];
    let mut i = 0;
    for p in alt {
        if i >= tokens.len() {
            if p.optional {
                continue;
            }
            return Err(format!("missing {}", p.name));
        }
        let slot = slots.iter().position(|n| n == &p.name);
        let tok = tokens[i];
        let value = match &p.kind {
            // A matched literal shows up as a value under its own name, so alternatives such
            // as `add <x> | remove <x>` can be told apart.
            Kind::Literal(l) => {
                if tok != l {
                    return Err(format!("expected {l}"));
                }
                Value::Text(l.clone())
            }
            Kind::Int => Value::Int(tok.parse().map_err(|_| format!("{} must be a whole number", p.name))?),
            Kind::Float => Value::Float(tok.parse().map_err(|_| format!("{} must be a number", p.name))?),
            Kind::Word => Value::Text(tok.to_string()),
            Kind::Text => {
                let v = Value::Text(tokens[i..].join(" "));
                i = tokens.len();
                if let Some(s) = slot {
                    values[s] = Some(v);
                }
                continue;
            }
            Kind::Item => {
                let n = resolve(c.item_names.keys(), tok).ok_or_else(|| format!("unknown item {tok}"))?;
                Value::Id(c.item_names[&n], n)
            }
            Kind::Block => {
                let n = resolve(c.block_names.keys(), tok).ok_or_else(|| format!("unknown block {tok}"))?;
                Value::Id(c.state_by_name(&n).unwrap_or(0), n)
            }
            Kind::Entity => {
                let n = resolve(c.game.entity_names.keys(), tok).ok_or_else(|| format!("unknown entity {tok}"))?;
                Value::Id(c.game.entity_names[&n], n)
            }
            Kind::Dim => {
                let n = resolve(c.dim_names.keys(), tok).ok_or_else(|| format!("unknown dimension {tok}"))?;
                Value::Id(c.dim_names[&n], n)
            }
            Kind::Player => {
                if !players.iter().any(|n| n == tok) {
                    return Err(format!("no player {tok}"));
                }
                Value::Text(tok.to_string())
            }
            Kind::Pos => {
                if i + 3 > tokens.len() {
                    return Err(format!("{} needs x y z", p.name));
                }
                let axis = |k: usize, base: f64| coordinate(tokens[i + k], base).ok_or_else(|| format!("bad coordinate {}", tokens[i + k]));
                let v = Value::Pos(DVec3::new(axis(0, here.x)?, axis(1, here.y)?, axis(2, here.z)?));
                i += 2;
                v
            }
        };
        if let Some(s) = slot {
            values[s] = Some(value);
        }
        i += 1;
    }
    if i < tokens.len() {
        return Err(format!("unexpected {}", tokens[i]));
    }
    Ok(Args { names: slots.to_vec(), values })
}

/// Parses `tokens` against the first alternative that accepts them.
pub fn parse_args(c: &Content, alts: &[Alt], tokens: &[&str], here: DVec3, players: &[String]) -> Result<Args, String> {
    let slots = slots(alts);
    let mut first_err = None;
    for alt in alts {
        match parse_alt(c, alt, tokens, here, players, &slots) {
            Ok(a) => return Ok(a),
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.unwrap_or_else(|| "bad arguments".into()))
}

#[derive(Clone)]
pub struct CommandDef {
    pub name: String,
    pub usage: String,
    pub alts: Vec<Alt>,
    /// `None`: an engine command.
    pub handler: Option<(ModId, Callback)>,
}

/// Engine commands, handled by the server itself.
pub const BUILTIN: &[(&str, &str)] = &[
    ("help", "[command:word]"),
    ("tp", "<pos:pos> | <target:player>"),
    ("dim", "<dim:dim>"),
    ("give", "<item:item> [count:int]"),
    ("time", "set <ticks:int>"),
    ("summon", "<entity:entity> [count:int]"),
    ("setblock", "<pos:pos> <block:block>"),
    ("insert", "<pos:pos> <item:item> [count:int]"),
    ("block", "<pos:pos>"),
    ("heal", ""),
    ("gamemode", "survival | creative | spectator"),
    ("regions", ""),
];

/// What the chat input can complete to: suggestions for the word being typed, the byte offset
/// where that word starts, and the usage of what is left to type.
pub struct Completion {
    pub suggestions: Vec<String>,
    pub from: usize,
    pub hint: String,
}

pub fn complete(c: &Content, line: &str, players: &[String]) -> Completion {
    let from = line.rfind(' ').map_or(0, |i| i + 1);
    let partial = &line[from..];
    let done: Vec<&str> = line[..from].split_whitespace().collect();
    let mut out = Completion { suggestions: Vec::new(), from, hint: String::new() };
    if done.is_empty() {
        let mut names: Vec<String> = c.commands.iter().map(|d| d.name.clone()).filter(|n| n.starts_with(partial)).collect();
        names.sort();
        out.suggestions = names;
        return out;
    }
    let Some(cmd) = c.commands.iter().find(|d| d.name == done[0]) else { return out };
    let typed = &done[1..];
    let mut seen = Vec::new();
    for alt in &cmd.alts {
        // Find the parameter the current word belongs to.
        let mut at = 0;
        let mut found = None;
        for (pi, p) in alt.iter().enumerate() {
            let width = if p.kind == Kind::Pos { 3 } else if p.kind == Kind::Text { usize::MAX / 2 } else { 1 };
            if typed.len() < at + width {
                found = Some((pi, typed.len() - at));
                break;
            }
            if let Kind::Literal(l) = &p.kind {
                if typed[at] != l {
                    found = None;
                    break;
                }
            }
            at += width;
        }
        let Some((pi, within)) = found else { continue };
        let p = &alt[pi];
        let names_from = |keys: Vec<&String>| -> Vec<String> {
            let mut v: Vec<String> = keys
                .into_iter()
                .filter(|n| n.starts_with(partial) || n.split_once(':').is_some_and(|(_, path)| path.starts_with(partial)))
                .cloned()
                .collect();
            v.sort();
            v
        };
        let list = match &p.kind {
            Kind::Literal(l) if l.starts_with(partial) => vec![l.clone()],
            Kind::Item => names_from(c.item_names.keys().collect()),
            Kind::Block => names_from(c.block_names.keys().collect()),
            Kind::Entity => names_from(c.game.entity_names.keys().collect()),
            Kind::Dim => names_from(c.dim_names.keys().collect()),
            Kind::Player => players.iter().filter(|n| n.starts_with(partial)).cloned().collect(),
            Kind::Pos if partial.is_empty() || partial == "~" => vec![if within == 0 { "~ ~ ~".to_string() } else { "~".to_string() }],
            _ => Vec::new(),
        };
        for s in list {
            if !seen.contains(&s) {
                seen.push(s);
            }
        }
        if out.hint.is_empty() {
            out.hint = alt[pi..]
                .iter()
                .map(|p| match &p.kind {
                    Kind::Literal(l) => l.clone(),
                    k if p.optional => format!("[{}]", hint_name(&p.name, k)),
                    k => format!("<{}>", hint_name(&p.name, k)),
                })
                .collect::<Vec<_>>()
                .join(" ");
        }
    }
    out.suggestions = seen;
    out
}

fn hint_name(name: &str, k: &Kind) -> String {
    if *k == Kind::Pos { "x y z".into() } else { name.to_string() }
}
