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
fn main() {
    for path in [
        "src/rules.rs",
        "src/world.rs",
        "src/rule_mlp.rs",
        "build/rule_mlp.rs",
        "src/autonomous_protocol.rs",
        "rules/worm-autonomous.rs",
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
    fs::write(output.join("worm_autonomous_rule.rs"), source).unwrap();
    fs::write(
        output.join("worm_autonomous_rule.json"),
        serde_json::to_vec(table.definition()).unwrap(),
    )
    .unwrap();
}

fn write_update(source: &mut String, table: &RuleTable) {
    source.push_str(
        "#[inline]\npub(super) fn update(input: u32, inbox: [u32; PORTS], previous: CellState) -> CellState {\n",
    );
    let lookup = &table.definition().lookup;
    if !lookup.is_empty() {
        writeln!(
            source,
            "const LOOKUP: [u32; {}] = {lookup:?};",
            lookup.len()
        )
        .unwrap();
    }

    // Give each write a fresh value so LLVM can remove copies, fold constants,
    // and schedule operations without an opcode dispatch or a register array.
    use rules::register::*;
    let mut registers: [String; COUNT] = std::array::from_fn(|_| "0u32".into());
    registers[INPUT] = "input".into();
    for port in 0..PORTS {
        registers[INBOX + port] = format!("inbox[{port}]");
    }
    for word in 0..MEMORY_WORDS {
        registers[MEMORY + word] = format!("previous.memory[{word}]");
    }
    registers[ANSWER] = "previous.answer".into();
    for (row, instruction) in table.definition().rows.iter().enumerate() {
        let read = |index: u32| &registers[index as usize];
        let a = || read(instruction.a);
        let b = || read(instruction.b);
        let c = || read(instruction.c);
        use rules::Opcode::*;
        let expression = match instruction.op {
            Constant => format!("{}u32", instruction.a),
            Copy => a().clone(),
            Add => format!("{}.wrapping_add({})", a(), b()),
            Subtract => format!("{}.wrapping_sub({})", a(), b()),
            Min => format!("{}.min({})", a(), b()),
            And => format!("{} & {}", a(), b()),
            Or => format!("{} | {}", a(), b()),
            Xor => format!("{} ^ {}", a(), b()),
            ShiftLeft => format!("{}.wrapping_shl({})", a(), b()),
            ShiftRight => format!("{}.wrapping_shr({})", a(), b()),
            Equal => format!("u32::from({} == {})", a(), b()),
            LessThan => format!("u32::from({} < {})", a(), b()),
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

    writeln!(
        source,
        "CellState {{ memory: [{}], outbox: [{}], answer: {} }}\n}}",
        registers[MEMORY..OUTBOX].join(", "),
        registers[OUTBOX..ANSWER].join(", "),
        registers[ANSWER]
    )
    .unwrap();
}
