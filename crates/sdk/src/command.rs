//! Chat commands. The usage string declares the arguments and the engine parses, checks and
//! completes them; the callback reads them by name:
//!
//! ```ignore
//! r.command("feed", "[amount:int]", |cmd| {
//!     let n = cmd.int("amount").unwrap_or(20);
//!     cmd.reply(&format!("fed {n}"));
//! });
//! ```

use crate::abi::{self, Str, command_value as cv};
use crate::{World, callback, host};

pub struct Command<'a> {
    args: &'a mut abi::CommandArgs,
    names: &'a [String],
}

impl Command<'_> {
    pub fn world(&self) -> World {
        World::from_raw(self.args.world)
    }

    /// Entity id of the player who ran the command.
    pub fn player(&self) -> u64 {
        self.args.player
    }

    fn value(&self, name: &str) -> Option<&abi::CommandValue> {
        let i = self.names.iter().position(|n| n == name)?;
        let all = unsafe { std::slice::from_raw_parts(self.args.values, self.args.count as usize) };
        all.get(i).filter(|v| v.kind != cv::NONE)
    }

    pub fn int(&self, name: &str) -> Option<i64> {
        self.value(name).filter(|v| v.kind == cv::INT).map(|v| v.int)
    }

    pub fn float(&self, name: &str) -> Option<f64> {
        self.value(name).filter(|v| v.kind == cv::INT || v.kind == cv::FLOAT).map(|v| v.float)
    }

    /// Words, text, player names, and the full names of items, blocks, entities, dimensions.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.value(name).filter(|v| v.kind == cv::TEXT || v.kind == cv::ID).map(|v| unsafe { v.text.as_str() })
    }

    /// Registry id of an item, block state, entity kind or dimension parameter.
    pub fn id(&self, name: &str) -> Option<u32> {
        self.value(name).filter(|v| v.kind == cv::ID).map(|v| v.int as u32)
    }

    /// Entities a `targets` parameter selected (`@e[type=zombie]`, a player name, ...).
    pub fn targets(&self, name: &str) -> Vec<u64> {
        self.value(name).filter(|v| v.kind == cv::TARGETS).map_or(Vec::new(), |v| unsafe { v.text.as_str() }.split_whitespace().filter_map(|t| t.parse().ok()).collect())
    }

    pub fn pos(&self, name: &str) -> Option<[f64; 3]> {
        self.value(name).filter(|v| v.kind == cv::POS).map(|v| v.pos)
    }

    /// Whether a literal word of the usage (or an optional parameter) was given.
    pub fn has(&self, name: &str) -> bool {
        self.value(name).is_some()
    }

    pub fn reply(&mut self, text: &str) {
        unsafe { (host().command_reply)(self.args, Str::new(text)) }
    }
}

/// Parameter names in the engine's order: first appearance across `|` alternatives.
fn names(usage: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in usage.split_whitespace() {
        if tok == "|" {
            continue;
        }
        let inner = tok.strip_prefix('<').or_else(|| tok.strip_prefix('[')).and_then(|t| t.strip_suffix('>').or_else(|| t.strip_suffix(']')));
        // Literal words are slots too: present when that word was typed.
        let name = inner.and_then(|t| t.split_once(':')).map_or(tok, |(n, _)| n);
        if !out.iter().any(|o| o == name) {
            out.push(name.to_string());
        }
    }
    out
}

pub(crate) fn command_cb(usage: &str, f: impl Fn(&mut Command) + Send + Sync + 'static) -> abi::Callback {
    let names = names(usage);
    callback(move |args: &mut abi::CommandArgs| f(&mut Command { args, names: &names }))
}
