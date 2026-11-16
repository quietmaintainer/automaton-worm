use bytemuck::cast_slice;
use wasm_bindgen::prelude::*;

use crate::autonomous::{AutonomousArena, ROUND_TICKS, RULE_JSON, RoomMode};
use crate::spawn::{MAX_GRID_SIZE, MAX_PELLETS, MAX_WORMS, WORM_LENGTH, minimum_size};
use crate::{CellState, MEMORY_WORDS, PORTS};

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[wasm_bindgen(js_name = arenaLimits)]
pub fn arena_limits() -> String {
    serde_json::json!({
        "maxGridSize": MAX_GRID_SIZE,
        "maxWorms": MAX_WORMS,
        "maxPellets": MAX_PELLETS,
        "wormLength": WORM_LENGTH,
        "minimumGridSize": (0..=MAX_WORMS).map(minimum_size).collect::<Vec<_>>(),
    })
    .to_string()
}

enum AutonomousKernel {
    Table,
    Mlp,
}

#[wasm_bindgen]
pub struct AutonomousWorm(AutonomousArena, AutonomousKernel);

#[wasm_bindgen]
impl AutonomousWorm {
    #[wasm_bindgen(constructor)]
    pub fn new(
        size: u32,
        seed: u32,
        count: u32,
        pellets: u32,
        room_mode: &str,
    ) -> Result<Self, JsValue> {
        let mode = match room_mode {
            "off" => RoomMode::Off,
            "direct" => RoomMode::Direct,
            "wave" => RoomMode::Wave,
            _ => return Err(JsValue::from_str("Choose Off, Direct, or Wave")),
        };
        AutonomousArena::generate_with_room_mode(
            size as usize,
            seed,
            count as usize,
            pellets as usize,
            mode,
        )
        .map(|arena| Self(arena, AutonomousKernel::Mlp))
        .map_err(js_error)
    }

    #[wasm_bindgen(js_name = setEngine)]
    pub fn set_engine(&mut self, engine: &str) -> Result<(), JsValue> {
        self.1 = match engine {
            "mlp" => AutonomousKernel::Mlp,
            "table" => AutonomousKernel::Table,
            _ => return Err(JsValue::from_str("Choose Optimized MLP or Compiled table")),
        };
        Ok(())
    }

    pub fn advance(&mut self, ticks: u32) -> Result<(), JsValue> {
        match self.1 {
            AutonomousKernel::Table => self.0.advance(ticks),
            AutonomousKernel::Mlp => self.0.advance_mlp(ticks),
        }
        .map_err(js_error)
    }

    pub fn tick(&self) -> u32 {
        self.0.tick()
    }

    pub fn width(&self) -> u32 {
        self.0.size() as u32
    }

    pub fn height(&self) -> u32 {
        self.width()
    }

    pub fn passages(&self) -> Box<[u8]> {
        self.0.links().collect()
    }

    #[wasm_bindgen(js_name = ruleJson)]
    pub fn rule_json() -> String {
        RULE_JSON.into()
    }

    #[wasm_bindgen(js_name = ticksPerMove)]
    pub fn ticks_per_move() -> u32 {
        ROUND_TICKS
    }

    pub fn bodies(&self) -> Box<[u32]> {
        self.0
            .bodies()
            .iter()
            .flatten()
            .map(|&cell| cell as u32)
            .collect()
    }

    pub fn headings(&self) -> Box<[u8]> {
        self.0.headings().collect()
    }

    pub fn pellets(&self) -> Box<[u32]> {
        self.0.pellets().map(|cell| cell as u32).collect()
    }

    #[wasm_bindgen(js_name = scentField)]
    pub fn scent_field(&self) -> Box<[u8]> {
        self.0.scent().collect()
    }

    #[wasm_bindgen(js_name = cellDetails)]
    pub fn cell_details(&self, index: u32) -> Option<Box<[u32]>> {
        grid_cell_words(self.0.cells(), self.0.size(), index as usize).map(Vec::into_boxed_slice)
    }

    #[wasm_bindgen(js_name = blockedRounds)]
    pub fn blocked_rounds(&self) -> f64 {
        self.0.blocked_rounds as f64
    }

    #[wasm_bindgen(js_name = recoveryJson)]
    pub fn recovery_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.0.recovery_stats()).map_err(js_error)
    }

    #[wasm_bindgen(js_name = totalFood)]
    pub fn total_food(&self) -> f64 {
        self.0.total_food as f64
    }

    #[wasm_bindgen(js_name = moveCounts)]
    pub fn move_counts(&self) -> Box<[u32]> {
        self.0.moves().into()
    }

    #[wasm_bindgen(js_name = foodCounts)]
    pub fn food_counts(&self) -> Box<[u32]> {
        self.0.scores().into()
    }

    pub fn contested(&self) -> f64 {
        self.0.contested as f64
    }

    pub fn reversals(&self) -> f64 {
        self.0.reversals as f64
    }

    #[wasm_bindgen(js_name = cellsFreed)]
    pub fn cells_freed(&self) -> f64 {
        self.0.cells_freed as f64
    }

    #[wasm_bindgen(js_name = roomSignals)]
    pub fn room_signals(&self) -> Box<[u32]> {
        cast_slice(self.0.room_signals()).into()
    }

    #[wasm_bindgen(js_name = forwardedRoomSignals)]
    pub fn forwarded_room_signals(&self) -> Box<[u32]> {
        cast_slice(self.0.forwarded_room_signals()).into()
    }

    #[wasm_bindgen(js_name = yieldedWorms)]
    pub fn yielded_worms(&self) -> Box<[u32]> {
        self.0.yielded_worms().into()
    }
}

fn grid_cell_words(cells: &[CellState], size: usize, index: usize) -> Option<Vec<u32>> {
    let cell = cells.get(index)?;
    let mut words = Vec::with_capacity(MEMORY_WORDS + 2 * PORTS + 1);
    words.extend(cell.memory);
    words.extend(cell.outbox);
    words.extend(
        crate::world::cardinal_neighbors(index, size, size)
            .into_iter()
            .enumerate()
            .map(|(port, link)| link.map_or(0, |neighbor| cells[neighbor].outbox[port ^ 1])),
    );
    words.push(cell.answer);
    Some(words)
}
