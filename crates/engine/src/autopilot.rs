//! Scripted input for automated checks: `voxide --script file`. One command per line:
//!
//! ```text
//! wait 2.5            # seconds
//! loaded              # wait until the world around the player is meshed
//! press core:inventory
//! hold core:forward 1.5
//! click 0             # mouse button
//! cursor 0.5 0.4      # move the mouse to a fraction of the window
//! look 90 -20         # yaw, pitch in degrees
//! chat /give stone 10
//! shot name.png       # into the screenshot directory given with --shot-dir
//! quit
//! ```

use std::time::Instant;

use crate::input::{Input, Key};

pub enum Step {
    Wait(f32),
    Loaded,
    Press(String),
    Hold(String, f32),
    Click(u8),
    Cursor(f32, f32),
    Look(f32, f32),
    Chat(String),
    Shot(String),
    Host(String),
    Join(String),
    Quit,
}

pub struct Autopilot {
    steps: Vec<Step>,
    next: usize,
    until: Option<Instant>,
    release: Vec<(Key, Instant)>,
}

pub enum Action {
    None,
    Look(f32, f32),
    Chat(String),
    Shot(String),
    /// Start hosting; the ticket is written to this file once online.
    Host(String),
    /// Join using the ticket stored in this file (waits until it exists).
    Join(String),
    Quit,
}

impl Autopilot {
    pub fn load(src: &str) -> Autopilot {
        let mut steps = Vec::new();
        for line in src.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
            let nums: Vec<f32> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            let step = match cmd {
                "wait" => Step::Wait(nums.first().copied().unwrap_or(1.0)),
                "loaded" => Step::Loaded,
                "press" => Step::Press(rest.trim().to_string()),
                "hold" => {
                    let (a, t) = rest.rsplit_once(' ').unwrap_or((rest, "1"));
                    Step::Hold(a.trim().to_string(), t.parse().unwrap_or(1.0))
                }
                "click" => Step::Click(nums.first().copied().unwrap_or(0.0) as u8),
                "cursor" if nums.len() == 2 => Step::Cursor(nums[0], nums[1]),
                "look" if nums.len() == 2 => Step::Look(nums[0], nums[1]),
                "chat" => Step::Chat(rest.to_string()),
                "shot" => Step::Shot(rest.trim().to_string()),
                "host" => Step::Host(rest.trim().to_string()),
                "join" => Step::Join(rest.trim().to_string()),
                "quit" => Step::Quit,
                _ => {
                    log::warn!("autopilot: unknown line '{line}'");
                    continue;
                }
            };
            steps.push(step);
        }
        Autopilot { steps, next: 0, until: None, release: Vec::new() }
    }

    /// Advances the script; `loaded` tells whether the world finished meshing.
    pub fn frame(&mut self, input: &mut Input, size: (u32, u32), loaded: bool) -> Action {
        let now = Instant::now();
        self.release.retain(|(k, at)| {
            if now >= *at {
                input.key_up(*k);
                false
            } else {
                true
            }
        });
        if let Some(t) = self.until {
            if now < t {
                return Action::None;
            }
            self.until = None;
        }
        let Some(step) = self.steps.get(self.next) else { return Action::None };
        let mut advance = true;
        let action = match step {
            Step::Wait(s) => {
                self.until = Some(now + std::time::Duration::from_secs_f32(*s));
                Action::None
            }
            Step::Loaded => {
                advance = loaded;
                Action::None
            }
            Step::Press(a) => {
                if let Some(k) = input.key_of(a) {
                    input.key_down(k);
                    self.release.push((k, now + std::time::Duration::from_millis(50)));
                }
                Action::None
            }
            Step::Hold(a, s) => {
                if let Some(k) = input.key_of(a) {
                    input.key_down(k);
                    self.release.push((k, now + std::time::Duration::from_secs_f32(*s)));
                }
                Action::None
            }
            Step::Click(b) => {
                input.key_down(Key::Mouse(*b));
                self.release.push((Key::Mouse(*b), now + std::time::Duration::from_millis(50)));
                Action::None
            }
            Step::Cursor(x, y) => {
                input.cursor = glam::Vec2::new(x * size.0 as f32, y * size.1 as f32);
                Action::None
            }
            Step::Look(y, p) => Action::Look(*y, *p),
            Step::Chat(t) => Action::Chat(t.clone()),
            Step::Shot(f) => Action::Shot(f.clone()),
            Step::Host(f) => Action::Host(f.clone()),
            Step::Join(f) => match std::fs::read_to_string(f) {
                Ok(t) if !t.trim().is_empty() => Action::Join(t.trim().to_string()),
                _ => {
                    advance = false;
                    Action::None
                }
            },
            Step::Quit => Action::Quit,
        };
        if advance {
            self.next += 1;
        }
        action
    }
}
