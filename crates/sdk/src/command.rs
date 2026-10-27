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
}

impl Command<'_> {
    /// The world it runs in; client commands (`Registry::client_command`) have none, use `client`.
    pub fn world(&self) -> World {
        assert!(!self.args.world.is_null(), "Command::world() in a client command: client commands have no world, use client()");
        World::from_raw(self.args.world)
    }

    /// The client that typed a client command.
    pub fn client(&self) -> Option<crate::Client> {
        (!self.args.client.is_null()).then(|| crate::Client::from_raw(self.args.client))
    }

    /// Entity id of the player who ran the command.
    pub fn player(&self) -> u64 {
        self.args.player
    }

    fn value(&self, name: &str) -> Option<&abi::CommandValue> {
        let all = unsafe { std::slice::from_raw_parts(self.args.values, self.args.count as usize) };
        all.iter().find(|v| unsafe { v.name.as_str() } == name).filter(|v| v.kind != cv::NONE)
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

    /// A line for whoever ran the command: the translation `key` with `args`, formatted by
    /// their client in its own language. Call again for more lines.
    pub fn reply(&mut self, key: &str, args: &[crate::Arg]) {
        let raw = crate::ui::raw_args(args);
        unsafe { (host().command_reply)(self.args, Str::new(key), crate::abi::Slice::new(&raw)) }
    }
}

pub(crate) fn command_cb(f: impl Fn(&mut Command) + Send + Sync + 'static) -> abi::Callback {
    callback(move |args: &mut abi::CommandArgs| f(&mut Command { args }))
}
