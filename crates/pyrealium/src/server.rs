//! Single-owner UDP runtime: every session mutation is serialized through one loop.
use crate::{
    movement::{Motion, Placement},
    script,
    state::{Phase, Session},
    store::Store,
    transport::Transport,
    wire::{self, Header},
    world,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    net::{SocketAddr, UdpSocket},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct Config {
    pub bind: SocketAddr,
    pub account: String,
    /// SQLite database holding accounts, characters and prepared content.
    pub database: crate::db::Database,
    /// Time between periodic character saves (`Store::save_due`).
    pub save_interval: Duration,
    /// Run directory: the `stop.request` signal file and the admin token.
    pub data: PathBuf,
    pub run_seconds: Option<u64>,
    pub visual_did: Option<u32>,
    pub game_data: Option<PathBuf>,
    pub initialize_ui: bool,
    pub training_kit: bool,
    pub lifestones: bool,
    pub portals: bool,
    pub map_travel: bool,
    pub content: Option<PathBuf>,
    pub encounters: Option<PathBuf>,
    /// Operator world-builder placements added on top of `content`.
    pub content_overlay: Option<PathBuf>,
    /// Operator creature stats, behaviour, loot, patrol and spawn-group rules.
    pub world_rules: Option<PathBuf>,
    /// Loopback address for live world editing (needs `content_overlay`).
    pub admin_bind: Option<SocketAddr>,
    /// Prepared content read from the database (`content_prep`); takes the place of `content`.
    pub prepared: Option<crate::content_prep::World>,
}
#[path = "tradeskill_actions.rs"]
mod trade_actions;
mod peer;
mod travel;
mod delivery;
mod examine;
mod combat_fx;
mod quest_use;
mod equipment_ops;
mod item_service;
mod trade_ops;
mod group_ops;
mod npc_use;
mod item_actions;
mod runtime;
mod startup;
mod serve;
use self::{peer::*, travel::*, delivery::*, examine::*, combat_fx::*, quest_use::*, equipment_ops::*, item_service::*, trade_ops::*, group_ops::*, npc_use::*, item_actions::*, runtime::*, startup::*, serve::*};
#[cfg(test)]
mod tests;

#[derive(Default)]
struct Counters {
    received: u64,
    rejected: u64,
    logins: u64,
    connected: u64,
    events: u64,
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}
fn other(e: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{e:?}"))
}

/// Developer login only (loopback): the client's account name is trusted.
pub fn run(config: Config) -> io::Result<()> {
    run_with_auth(config, false)
}
/// With `tickets`, every login spends a one-use ticket from the account service.
pub fn run_with_auth(config: Config, tickets: bool) -> io::Result<()> {
    run_auth_mode(config, tickets, false)
}
/// Loopback password accounts, plus no-ticket login for the configured developer profile.
pub fn run_local_accounts(config: Config) -> io::Result<()> {
    run_auth_mode(config, true, true)
}
