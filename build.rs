use std::{env, fmt::Write, fs, path::PathBuf};

// Read the same format and validate the same operands as the runtime loader.
#[allow(dead_code)]
#[path = "src/rules.rs"]
mod rules;
use rules::RuleTable;
#[allow(dead_code)]
#[path = "src/world.rs"]
mod world;
use world::{CellState, MEMORY_WORDS, PORTS};

#[path = "build/rule_mlp.rs"]
mod mlp_compiler;
#[allow(dead_code)]
#[path = "src/rule_mlp.rs"]
mod rule_mlp;

#[path = "src/autonomous_protocol.rs"]
mod autonomous_protocol;
#[path = "rules/worm-autonomous.rs"]
mod autonomous_rule;
#[allow(dead_code)]
#[path = "src/formation_protocol.rs"]
mod formation_protocol;
#[path = "rules/worm-formations.rs"]
mod formation_rule;

fn main() {
    for path in [
        "src/rules.rs",
        "src/world.rs",
        "src/rule_mlp.rs",
        "build/rule_mlp.rs",
        "src/autonomous_protocol.rs",
        "rules/worm-autonomous.rs",
        "src/formation_protocol.rs",
        "rules/worm-formations.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let table = autonomous_rule::table();
    let (model, stats) = mlp_compiler::compile(&table, autonomous_protocol::TICKS_PER_MOVE);
    fs::write(output.join("worm_autonomous_mlp.rs"), model).unwrap();
    fs::write(
        output.join("worm_autonomous_mlp_stats.json"),
        serde_json::to_vec_pretty(&stats).unwrap(),
    )
    .unwrap();

    let mut source = String::new();
    write_update(&mut source, &table);
    fs::write(output.join("worm_autonomous_rule.rs"), &source).unwrap();
    fs::write(
        output.join("worm_autonomous_rule.json"),
        serde_json::to_vec(table.definition()).unwrap(),
    )
    .unwrap();

    source.clear();
    let formations = formation_rule::table();
    write_update(&mut source, &formations);
    fs::write(output.join("worm_formation_rule.rs"), source).unwrap();
    fs::write(
        output.join("worm_formation_rule.json"),
        serde_json::to_vec(formations.definition()).unwrap(),
    )
    .unwrap();
}

fn write_update<W: Copy + Into<u64>>(source: &mut String, table: &RuleTable<W>) {
    let wide = size_of::<W>() == size_of::<u64>();
    let (word, name) = if wide {
        ("u64", "update_at_phase<const PHASE: u32>")
    } else {
        ("u32", "update")
    };
    writeln!(source, "#[inline]\npub(super) fn {name}(input: u32, inbox: [{word}; PORTS], previous: CellState) -> CellState {{").unwrap();
    if wide {
        source.push_str(
            "let input = u64::from(input);\n\
            let mask = super::PHASE_MASK as u32;\n\
            debug_assert_eq!(previous.memory[3] & mask, PHASE);\n\
            let clock = (previous.memory[3] & !mask) | PHASE;\n",
        );
    }
    let lookup = &table.definition().lookup;
    if !lookup.is_empty() {
        writeln!(
            source,
            "const LOOKUP: [{word}; {}] = {lookup:?};",
            lookup.len()
        )
        .unwrap();
    }

    // Give each write a fresh value so LLVM can remove copies, fold constants,
    // and schedule operations without an opcode dispatch or a register array.
    use rules::register::*;
    let mut registers: [String; COUNT] = std::array::from_fn(|_| format!("0{word}"));
    registers[INPUT] = "input".into();
    for port in 0..PORTS {
        registers[INBOX + port] = format!("inbox[{port}]");
    }
    for word in 0..MEMORY_WORDS {
        let value = format!("previous.memory[{word}]");
        registers[MEMORY + word] = if wide {
            format!("u64::from({value})")
        } else {
            value
        };
    }
    if wide {
        registers[MEMORY + MEMORY_WORDS - 1] = "u64::from(clock)".into();
    }
    registers[ANSWER] = "previous.answer".into();
    for (row, instruction) in table.definition().rows.iter().enumerate() {
        let read = |index: u32| &registers[index as usize];
        let literal = instruction.a.into();
        let a = || read(literal as u32);
        let b = || read(instruction.b);
        let c = || read(instruction.c);
        let shift = || {
            if wide {
                format!("{} as u32", b())
            } else {
                b().clone()
            }
        };
        use rules::Opcode::*;
        let expression = match instruction.op {
            Constant => format!("{literal}{word}"),
            Copy => a().clone(),
            Add => format!("{}.wrapping_add({})", a(), b()),
            Subtract => format!("{}.wrapping_sub({})", a(), b()),
            Min => format!("{}.min({})", a(), b()),
            And => format!("{} & {}", a(), b()),
            Or => format!("{} | {}", a(), b()),
            Xor => format!("{} ^ {}", a(), b()),
            ShiftLeft => format!("{}.wrapping_shl({})", a(), shift()),
            ShiftRight => format!("{}.wrapping_shr({})", a(), shift()),
            Equal => format!("{word}::from({} == {})", a(), b()),
            LessThan => format!("{word}::from({} < {})", a(), b()),
            Select => format!("if {} != 0 {{ {} }} else {{ {} }}", c(), a(), b()),
            Lookup => format!(
                "if {} < {} {{ LOOKUP[({} + {}) as usize] }} else {{ 0 }}",
                a(),
                instruction.c,
                instruction.b,
                a(),
            ),
        };
        let name = format!("_v{row}");
        writeln!(source, "let {name} = {expression};").unwrap();
        registers[instruction.dst as usize] = name;
    }

    let memory = registers[MEMORY..OUTBOX]
        .iter()
        .map(|value| {
            if wide {
                format!("{value} as u32")
            } else {
                value.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        source,
        "CellState {{ memory: [{memory}], outbox: [{}], answer: {} }}\n}}",
        registers[OUTBOX..ANSWER].join(", "),
        registers[ANSWER]
    )
    .unwrap();
}
