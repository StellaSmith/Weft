//! What mods, files, commands and rich text name blocks, items and biomes with, parsed here and
//! nowhere else:
//! - `base:stone`: that thing (a block with all its states),
//! - `base:furnace[lit=true]`: the block's states with those property values,
//! - `#logs`: everything carrying the tag,
//! - `#ingots/*`: everything carrying any tag the pattern matches (`*` is any text).
//!
//! A name nothing has and a tag pattern no tag matches are errors at load, said with the mod and
//! the call. Recipes have one more form, the bound variable `#ores/{m}` -> `#ingots/{m}`
//! (`registry::expand_variables`).

use rustc_hash::FxHashMap;

use crate::components::{self as comps, Components};
use crate::content::{BlockDef, split_state};

pub enum Ref<'a> {
    Name(&'a str, Vec<(&'a str, &'a str)>),
    Tags(&'a str),
}

impl<'a> Ref<'a> {
    pub fn parse(text: &'a str) -> Result<Ref<'a>, String> {
        let text = text.trim();
        if let Some(pattern) = text.strip_prefix('#') {
            if pattern.is_empty() {
                return Err("`#` without a tag".into());
            }
            return Ok(Ref::Tags(pattern));
        }
        if text.is_empty() {
            return Err("an empty name".into());
        }
        let (name, props) = split_state(text)?;
        Ok(Ref::Name(name, props))
    }
}

/// Ids of the tags `pattern` matches (`*` any text), an error when there is none.
pub fn tags(c: &Components, pattern: &str) -> Result<Vec<u32>, String> {
    let found: Vec<u32> = c.names.iter().enumerate().filter(|(i, n)| c.is_tag(*i as u32) && comps::matches(pattern, n)).map(|(i, _)| i as u32).collect();
    if found.is_empty() { Err(format!("no tag matches #{pattern}")) } else { Ok(found) }
}

/// One tag (no pattern): conditions and filters that test a single tag.
pub fn tag(c: &Components, text: &str) -> Result<u32, String> {
    let name = tag_name(text)?;
    c.id(name).filter(|&t| c.is_tag(t)).ok_or_else(|| format!("no tag #{name}"))
}

/// The name in `#name`, for calls that only take a tag; a bare name is an error since it would
/// read as a block or item anywhere else.
pub fn tag_name(text: &str) -> Result<&str, String> {
    match text.trim().strip_prefix('#') {
        Some(t) if !t.is_empty() => Ok(t),
        _ => Err(format!("`{text}` is not a tag: tags are written #{text}")),
    }
}

/// The block states a reference stands for.
pub fn states(r: &Ref, names: &FxHashMap<String, u32>, blocks: &[BlockDef], c: &Components, state_count: u32) -> Result<Vec<u32>, String> {
    match r {
        Ref::Name(name, props) => blocks[*names.get(*name).ok_or_else(|| format!("unknown block {name}"))? as usize].matching(props),
        Ref::Tags(p) => {
            let t = tags(c, p)?;
            Ok((0..state_count).filter(|&s| t.iter().any(|&t| c.has(comps::BLOCK, s, t))).collect())
        }
    }
}

/// The blocks (definition index) a reference stands for.
pub fn blocks(r: &Ref, names: &FxHashMap<String, u32>, blocks: &[BlockDef], c: &Components, state_count: u32) -> Result<Vec<u32>, String> {
    let mut out: Vec<u32> = match r {
        Ref::Name(name, props) if props.is_empty() => vec![*names.get(*name).ok_or_else(|| format!("unknown block {name}"))?],
        _ => states(r, names, blocks, c, state_count)?.into_iter().map(|s| blocks.iter().position(|b| s >= b.first_state && s < b.first_state + b.states).unwrap_or(0) as u32).collect(),
    };
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// One block state: a name with properties picks that state (the rest at their defaults), a
/// tag its preferred block's first state.
pub fn state(r: &Ref, names: &FxHashMap<String, u32>, blocks: &[BlockDef], c: &Components) -> Result<u32, String> {
    match r {
        Ref::Name(name, props) => blocks[*names.get(*name).ok_or_else(|| format!("unknown block {name}"))? as usize].state_of(props),
        Ref::Tags(p) => {
            let t = tags(c, p)?;
            blocks.iter().find(|b| t.iter().any(|&t| c.has(comps::BLOCK, b.first_state, t))).map(|b| b.first_state).ok_or_else(|| format!("no block has #{p}"))
        }
    }
}

/// The biomes a reference stands for: a biome by name, or every biome with the tags.
pub fn biomes(r: &Ref, names: &FxHashMap<String, u32>, c: &Components, count: u32) -> Result<Vec<u32>, String> {
    match r {
        Ref::Name(name, props) if props.is_empty() => Ok(vec![*names.get(*name).ok_or_else(|| format!("unknown biome {name}"))?]),
        Ref::Name(name, _) => Err(format!("{name}: a biome has no properties")),
        Ref::Tags(p) => {
            let t = tags(c, p)?;
            Ok((0..count).filter(|&b| t.iter().any(|&t| c.has(comps::BIOME, b, t))).collect())
        }
    }
}

/// Items carrying each tag, the preferred first: in mod load order, then registration order.
/// "The item of a tag" (a recipe's output, a loot entry, a tab icon) is the first of its list.
pub fn tag_items(c: &Components, ordered: &[u32]) -> FxHashMap<u32, Vec<u32>> {
    let mut out: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
    for &i in ordered {
        for t in c.of(comps::ITEM, i).filter(|&t| c.is_tag(t)) {
            out.entry(t).or_default().push(i);
        }
    }
    out
}

/// The items a reference stands for (a name with properties is no item).
pub fn items(r: &Ref, names: &FxHashMap<String, u32>, c: &Components, by_tag: &FxHashMap<u32, Vec<u32>>) -> Result<Vec<u32>, String> {
    match r {
        Ref::Name(name, props) if props.is_empty() => Ok(vec![*names.get(*name).ok_or_else(|| format!("unknown item {name}"))?]),
        Ref::Name(name, _) => Err(format!("{name}: an item has no properties")),
        Ref::Tags(p) => {
            let mut out: Vec<u32> = tags(c, p)?.iter().flat_map(|t| by_tag.get(t).cloned().unwrap_or_default()).collect();
            out.dedup();
            if out.is_empty() { Err(format!("no item has #{p}")) } else { Ok(out) }
        }
    }
}

/// One item: the named one, or the preferred item of a tag.
pub fn item(r: &Ref, names: &FxHashMap<String, u32>, c: &Components, by_tag: &FxHashMap<u32, Vec<u32>>) -> Result<u32, String> {
    items(r, names, c, by_tag).map(|v| v[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_syntax() {
        assert!(matches!(Ref::parse("base:stone"), Ok(Ref::Name("base:stone", ref p)) if p.is_empty()));
        assert!(matches!(Ref::parse("base:furnace[lit=true, facing=east]"), Ok(Ref::Name("base:furnace", ref p)) if p == &[("lit", "true"), ("facing", "east")]));
        assert!(matches!(Ref::parse("#ingots/*"), Ok(Ref::Tags("ingots/*"))));
        assert!(Ref::parse("#").is_err());
        assert!(Ref::parse("base:furnace[lit").is_err());
        assert!(Ref::parse("base:furnace[lit]").is_err());
        assert_eq!(tag_name("#plant_soil"), Ok("plant_soil"));
        assert!(tag_name("plant_soil").is_err());
    }
}
