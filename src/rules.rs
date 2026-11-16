use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{CellState, MEMORY_WORDS, PORTS};

pub const RULE_VERSION: u32 = 1;
const MAX_RULE_ROWS: usize = 1024;

pub mod register {
    pub const ZERO: usize = 0;
    pub const INPUT: usize = 1;
    pub const INBOX: usize = 2;
    pub const MEMORY: usize = INBOX + super::PORTS;
    pub const OUTBOX: usize = MEMORY + super::MEMORY_WORDS;
    pub const ANSWER: usize = OUTBOX + super::PORTS;
    pub const SCRATCH: usize = ANSWER + 1;
    pub const COUNT: usize = 32;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Opcode {
    Constant,
    Copy,
    Add,
    Subtract,
    Min,
    And,
    Or,
    Xor,
    ShiftLeft,
    ShiftRight,
    Equal,
    LessThan,
    Select,
    Lookup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub op: Opcode,
    pub dst: u32,
    #[serde(default)]
    pub a: u32,
    #[serde(default)]
    pub b: u32,
    #[serde(default)]
    pub c: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDefinition {
    pub version: u32,
    pub rows: Vec<Row>,
    #[serde(default)]
    pub lookup: Vec<u32>,
}

/// Validated, immutable program shared by every site in a simulation.
#[derive(Clone, Debug)]
pub struct RuleTable(RuleDefinition);

impl RuleTable {
    pub fn new(definition: RuleDefinition) -> Result<Self> {
        ensure!(
            definition.version == RULE_VERSION,
            "unsupported rule version {}",
            definition.version
        );
        ensure!(
            (1..=MAX_RULE_ROWS).contains(&definition.rows.len()),
            "a rule needs 1–{MAX_RULE_ROWS} rows"
        );
        ensure!(
            definition.lookup.len() <= 65_536,
            "lookup table exceeds 65,536 words"
        );
        let register = |index| (index as usize) < register::COUNT;
        for (i, &Row { op, dst, a, b, c }) in definition.rows.iter().enumerate() {
            ensure!(
                (register::MEMORY..register::COUNT).contains(&(dst as usize)),
                "row {i} writes a read-only or invalid register"
            );
            let valid = match op {
                Opcode::Constant => b == 0 && c == 0,
                Opcode::Copy => register(a) && b == 0 && c == 0,
                Opcode::Select => register(a) && register(b) && register(c),
                Opcode::Lookup => {
                    register(a)
                        && c > 0
                        && u64::from(b) + u64::from(c) <= definition.lookup.len() as u64
                }
                _ => register(a) && register(b) && c == 0,
            };
            ensure!(valid, "invalid operands in row {i} ({op:?})");
        }
        Ok(Self(definition))
    }

    pub fn from_json(json: &str) -> Result<Self> {
        Self::new(serde_json::from_str(json).context("invalid rule table JSON")?)
    }

    pub fn definition(&self) -> &RuleDefinition {
        &self.0
    }

    pub(crate) fn update(&self, input: u32, inbox: [u32; PORTS], previous: CellState) -> CellState {
        use register::{ANSWER, COUNT, INBOX, INPUT, MEMORY, OUTBOX};
        let mut regs = [0; COUNT];
        regs[INPUT] = input;
        regs[INBOX..MEMORY].copy_from_slice(&inbox);
        regs[MEMORY..OUTBOX].copy_from_slice(&previous.memory);
        regs[ANSWER] = previous.answer;
        for &Row { op, dst, a, b, c } in &self.0.rows {
            let read = |index| regs[index as usize];
            let value = match op {
                Opcode::Constant => a,
                Opcode::Copy => read(a),
                Opcode::Add => read(a).wrapping_add(read(b)),
                Opcode::Subtract => read(a).wrapping_sub(read(b)),
                Opcode::Min => read(a).min(read(b)),
                Opcode::And => read(a) & read(b),
                Opcode::Or => read(a) | read(b),
                Opcode::Xor => read(a) ^ read(b),
                Opcode::ShiftLeft => read(a).wrapping_shl(read(b)),
                Opcode::ShiftRight => read(a).wrapping_shr(read(b)),
                Opcode::Equal => u32::from(read(a) == read(b)),
                Opcode::LessThan => u32::from(read(a) < read(b)),
                Opcode::Select => {
                    if read(c) != 0 {
                        read(a)
                    } else {
                        read(b)
                    }
                }
                Opcode::Lookup => {
                    let index = read(a);
                    if index < c {
                        self.0.lookup[(b + index) as usize]
                    } else {
                        0
                    }
                }
            };
            regs[dst as usize] = value;
        }
        CellState {
            memory: regs[MEMORY..OUTBOX].try_into().unwrap(),
            outbox: regs[OUTBOX..ANSWER].try_into().unwrap(),
            answer: regs[ANSWER],
        }
    }
}
