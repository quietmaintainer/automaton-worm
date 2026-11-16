use std::io::{self, BufWriter, Write};

use automaton_worm::autonomous::{AutonomousArena, RoomMode};

fn compare(left: &AutonomousArena, right: &AutonomousArena, case: &str) {
    assert_eq!(left.tick(), right.tick(), "{case}: tick");
    let tick = left.tick();
    assert!(
        left.cells() == right.cells(),
        "{case}: cells at tick {tick}"
    );
    assert_eq!(
        left.bodies(),
        right.bodies(),
        "{case}: bodies at tick {tick}"
    );
    assert_eq!(left.moves(), right.moves(), "{case}: moves at tick {tick}");
    assert_eq!(
        left.scores(),
        right.scores(),
        "{case}: scores at tick {tick}"
    );
    assert_eq!(
        left.total_food, right.total_food,
        "{case}: pickups at tick {tick}"
    );
    assert_eq!(
        left.blocked_rounds, right.blocked_rounds,
        "{case}: blocked at tick {tick}"
    );
    assert_eq!(
        left.contested, right.contested,
        "{case}: contested at tick {tick}"
    );
    assert_eq!(
        left.reversals, right.reversals,
        "{case}: reversals at tick {tick}"
    );
    assert_eq!(
        left.cells_freed, right.cells_freed,
        "{case}: cells freed at tick {tick}"
    );
    assert_eq!(
        left.recovery_stats(),
        right.recovery_stats(),
        "{case}: recovery at tick {tick}"
    );
    assert_eq!(
        left.room_signals(),
        right.room_signals(),
        "{case}: requests at tick {tick}"
    );
    assert_eq!(
        left.forwarded_room_signals(),
        right.forwarded_room_signals(),
        "{case}: relays at tick {tick}"
    );
    assert_eq!(
        left.yielded_worms(),
        right.yielded_worms(),
        "{case}: yields at tick {tick}"
    );
}

fn main() -> anyhow::Result<()> {
    let mut out = BufWriter::new(io::stdout().lock());
    let mut frame = Vec::with_capacity(100 * 100 * 36);
    for seed in 412001..=412064 {
        for mode in [RoomMode::Off, RoomMode::Direct, RoomMode::Wave] {
            let mut table = AutonomousArena::generate_with_room_mode(100, seed, 1000, 32, mode)?;
            if seed % 2 == 0 {
                table.advance(288)?;
            }
            let mut mlp = table.clone();
            let mut switched = table.clone();
            let case = format!("{seed} {mode:?}");
            for tick in 1..=100 {
                table.advance(1)?;
                mlp.advance_mlp(1)?;
                if (18..=59).contains(&tick) {
                    switched.advance(1)?;
                } else {
                    switched.advance_mlp(1)?;
                }
                compare(&table, &mlp, &case);
                compare(&table, &switched, &case);

                // Record the actual source-compatible cell words in row-major order.
                frame.clear();
                for cell in table.cells() {
                    for value in cell
                        .memory
                        .into_iter()
                        .chain(cell.outbox)
                        .chain([cell.answer])
                    {
                        frame.extend_from_slice(&value.to_le_bytes());
                    }
                }
                out.write_all(&frame)?;
            }
        }
    }
    out.flush()?;
    Ok(())
}
