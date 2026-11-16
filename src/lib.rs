//! Synchronous cell rules for foraging worms.

pub mod autonomous;
// The build-time rule assembler and runtime each use a subset of the protocol.
#[allow(dead_code)]
mod autonomous_protocol;
mod rule_mlp;
mod rules;
pub mod spawn;
mod world;

#[cfg(target_arch = "wasm32")]
mod web;

pub use rules::{Opcode, RULE_VERSION, Row, RuleDefinition, RuleTable, register};
pub use world::{CellState, CpuSwarm, Link, MEMORY_WORDS, PORTS, Site, World};
