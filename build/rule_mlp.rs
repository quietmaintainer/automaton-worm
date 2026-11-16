//! Program a sparse feed-forward network from the cell rule, then lower its weights.

use std::collections::HashMap;
use std::fmt::Write;

use crate::rule_mlp::{INPUT_WORDS, OUTPUT_WORDS, WORD_BITS};
use crate::rules::{Opcode, RuleTable, register::*};

type Wire = usize;
type Word = [Wire; WORD_BITS];
const ZERO_BIT: Wire = 0;
const ONE_BIT: Wire = 1;
const ZERO_WORD: Word = [ZERO_BIT; WORD_BITS];
const FIRST_GATE: Wire = 2 + INPUT_WORDS * WORD_BITS;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum BitOp {
    Not,
    And,
    Or,
    Xor,
    AndNot,
    Select,
}

fn constant(value: u32) -> Word {
    std::array::from_fn(|bit| ((value >> bit) & 1) as Wire)
}

fn input_word(word: usize) -> Word {
    std::array::from_fn(|bit| 2 + word * WORD_BITS + bit)
}

#[derive(Clone, Copy)]
struct Neuron {
    inputs: [Wire; 3],
    weights: [i8; 3],
    bias: i8,
}

impl Neuron {
    fn truth_table(self) -> u8 {
        let mut table = 0;
        for pattern in 0..8 {
            let sum = self.bias
                + self.weights[0] * (pattern & 1)
                + self.weights[1] * ((pattern >> 1) & 1)
                + self.weights[2] * ((pattern >> 2) & 1);
            // Clipped ReLU, with an integral preactivation and binary output.
            table |= (sum.clamp(0, 1) as u8) << pattern;
        }
        table
    }
}

#[derive(Default)]
struct Mlp {
    neurons: Vec<Neuron>,
}

impl Mlp {
    fn unit(&mut self, inputs: [Wire; 3], weights: [i8; 3], bias: i8) -> Wire {
        let wire = FIRST_GATE + self.neurons.len();
        self.neurons.push(Neuron {
            inputs,
            weights,
            bias,
        });
        wire
    }

    fn not(&mut self, a: Wire) -> Wire {
        self.unit([a, 0, 0], [-1, 0, 0], 1)
    }

    fn and(&mut self, a: Wire, b: Wire) -> Wire {
        self.unit([a, b, 0], [1, 1, 0], -1)
    }

    fn or(&mut self, a: Wire, b: Wire) -> Wire {
        self.unit([a, b, 0], [1, 1, 0], 0)
    }

    fn xor(&mut self, a: Wire, b: Wire) -> Wire {
        let both = self.and(a, b);
        self.unit([a, b, both], [1, 1, -2], 0)
    }

    fn select_bit(&mut self, a: Wire, b: Wire, yes: Wire, no: Wire) -> Wire {
        let when_yes = self.and(yes, a);
        let when_no = self.and(no, b);
        self.or(when_yes, when_no)
    }

    fn select(&mut self, a: Word, b: Word, yes: Wire) -> Word {
        let no = self.not(yes);
        std::array::from_fn(|bit| self.select_bit(a[bit], b[bit], yes, no))
    }

    fn nonzero(&mut self, word: Word) -> Wire {
        word.into_iter()
            .fold(ZERO_BIT, |any, bit| self.or(any, bit))
    }

    fn equal(&mut self, a: Word, b: Word) -> Wire {
        let differences = std::array::from_fn(|bit| self.xor(a[bit], b[bit]));
        let any = self.nonzero(differences);
        self.not(any)
    }

    fn less(&mut self, a: Word, b: Word) -> Wire {
        // Subtraction borrow, from least to most significant bit.
        (0..WORD_BITS).fold(ZERO_BIT, |borrow, bit| {
            self.unit([a[bit], b[bit], borrow], [-1, 1, 1], 0)
        })
    }

    fn add(&mut self, a: Word, b: Word, mut carry: Wire) -> Word {
        std::array::from_fn(|bit| {
            let pair = self.xor(a[bit], b[bit]);
            let sum = self.xor(pair, carry);
            carry = self.unit([a[bit], b[bit], carry], [1, 1, 1], -1);
            sum
        })
    }

    fn shift(&mut self, mut value: Word, amount: Word, left: bool) -> Word {
        // A five-stage barrel shifter implements Rust's wrapping shift count.
        for (bit, select) in amount.into_iter().take(5).enumerate() {
            let distance = 1 << bit;
            let shifted = std::array::from_fn(|i| {
                let source = if left {
                    i.wrapping_sub(distance)
                } else {
                    i + distance
                };
                value.get(source).copied().unwrap_or(ZERO_BIT)
            });
            value = self.select(shifted, value, select);
        }
        value
    }

    fn lookup(&mut self, index: Word, values: &[u32]) -> Word {
        let mut output = ZERO_WORD;
        for (key, &value) in values.iter().enumerate().filter(|&(_, value)| *value != 0) {
            let matched = self.equal(index, constant(key as u32));
            for (bit, out) in output.iter_mut().enumerate() {
                if value & (1 << bit) != 0 {
                    *out = self.or(*out, matched);
                }
            }
        }
        output
    }

    fn program(table: &RuleTable) -> (Self, [Word; OUTPUT_WORDS]) {
        use Opcode::*;
        let mut model = Self::default();
        let mut registers = [ZERO_WORD; COUNT];
        for (word, register) in (INPUT..OUTBOX).chain([ANSWER]).enumerate() {
            registers[register] = input_word(word);
        }
        for &row in &table.definition().rows {
            let read = |register: u32| registers[register as usize];
            let boolean = |bit| {
                let mut word = ZERO_WORD;
                word[0] = bit;
                word
            };
            // Each row is a sparse subnetwork. Its outputs become the source
            // wires of later rows, even when those rows overwrite a register.
            let word = match row.op {
                Constant => constant(row.a),
                Copy => read(row.a),
                Add => model.add(read(row.a), read(row.b), ZERO_BIT),
                Subtract => {
                    let negated = read(row.b).map(|bit| model.not(bit));
                    model.add(read(row.a), negated, ONE_BIT)
                }
                Min => {
                    let less = model.less(read(row.a), read(row.b));
                    model.select(read(row.a), read(row.b), less)
                }
                And => std::array::from_fn(|bit| model.and(read(row.a)[bit], read(row.b)[bit])),
                Or => std::array::from_fn(|bit| model.or(read(row.a)[bit], read(row.b)[bit])),
                Xor => std::array::from_fn(|bit| model.xor(read(row.a)[bit], read(row.b)[bit])),
                ShiftLeft | ShiftRight => {
                    model.shift(read(row.a), read(row.b), row.op == ShiftLeft)
                }
                Equal => boolean(model.equal(read(row.a), read(row.b))),
                LessThan => boolean(model.less(read(row.a), read(row.b))),
                Select => {
                    let yes = model.nonzero(read(row.c));
                    model.select(read(row.a), read(row.b), yes)
                }
                Lookup => model.lookup(
                    read(row.a),
                    &table.definition().lookup[row.b as usize..(row.b + row.c) as usize],
                ),
            };
            registers[row.dst as usize] = word;
        }
        let output = registers[MEMORY..=ANSWER].try_into().unwrap();
        (model, output)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Gate {
    op: BitOp,
    inputs: [Wire; 3],
}

#[derive(Default)]
struct Lowering {
    gates: Vec<Gate>,
    common: HashMap<Gate, Wire>,
}

impl Lowering {
    fn emit(&mut self, op: BitOp, a: Wire, b: Wire, c: Wire) -> Wire {
        let gate = Gate {
            op,
            inputs: [a, b, c],
        };
        *self.common.entry(gate).or_insert_with(|| {
            let wire = FIRST_GATE + self.gates.len();
            self.gates.push(gate);
            wire
        })
    }

    fn binary(&mut self, table: u8, a: Wire, b: Wire) -> Wire {
        use BitOp::*;
        let (low, high) = if a <= b { (a, b) } else { (b, a) };
        if a < 2 && b < 2 {
            return ((table >> (a + 2 * b)) & 1) as Wire;
        }
        // Reduce the two constant and equal-input cases before emitting gates.
        if a < 2 {
            let mask = ((table >> a) & 1) | (((table >> (a + 2)) & 1) << 1);
            return self.unary(mask, b);
        }
        if b < 2 {
            return self.unary((table >> (2 * b)) & 3, a);
        }
        if a == b {
            return self.unary((table & 1) | ((table >> 2) & 2), a);
        }
        match table {
            0 => ZERO_BIT,
            15 => ONE_BIT,
            10 => a,
            12 => b,
            5 => self.emit(Not, a, 0, 0),
            3 => self.emit(Not, b, 0, 0),
            8 => self.emit(And, low, high, 0),
            14 => self.emit(Or, low, high, 0),
            6 => self.emit(Xor, low, high, 0),
            2 => self.emit(AndNot, a, b, 0),
            4 => self.emit(AndNot, b, a, 0),
            _ => {
                let inverse = self.binary(table ^ 15, a, b);
                self.emit(Not, inverse, 0, 0)
            }
        }
    }

    fn unary(&mut self, table: u8, input: Wire) -> Wire {
        match table {
            0 => ZERO_BIT,
            1 => self.emit(BitOp::Not, input, 0, 0),
            2 => input,
            3 => ONE_BIT,
            _ => unreachable!(),
        }
    }

    fn unit(&mut self, table: u8, [a, b, c]: [Wire; 3]) -> Wire {
        use BitOp::*;
        let lo = self.binary(table & 15, a, b);
        let hi = self.binary(table >> 4, a, b);
        match (c, lo, hi) {
            (ZERO_BIT, _, _) => lo,
            (ONE_BIT, _, _) => hi,
            (_, x, y) if x == y => x,
            (_, ZERO_BIT, _) => self.binary(8, c, hi),
            (_, _, ZERO_BIT) => self.binary(2, lo, c),
            (_, ONE_BIT, _) => self.binary(13, c, hi),
            (_, _, ONE_BIT) => self.binary(14, c, lo),
            _ => self.emit(Select, c, hi, lo),
        }
    }

    fn write(self, output: [Word; OUTPUT_WORDS], phase: u32, source: &mut String) -> usize {
        let mut live = vec![false; FIRST_GATE + self.gates.len()];
        for wire in output.into_iter().flatten() {
            live[wire] = true;
        }
        for (index, gate) in self.gates.iter().enumerate().rev() {
            if live[FIRST_GATE + index] {
                for wire in gate.inputs {
                    live[wire] = true;
                }
            }
        }
        let name = |wire| match wire {
            ZERO_BIT => "0u64".to_owned(),
            ONE_BIT => "u64::MAX".to_owned(),
            2..FIRST_GATE => format!(
                "input[{}][{}]",
                (wire - 2) / WORD_BITS,
                (wire - 2) % WORD_BITS
            ),
            _ => format!("gate{wire}"),
        };

        writeln!(
            source,
            "fn phase_{phase}(input: &InputPlanes) -> OutputPlanes {{"
        )
        .unwrap();
        let mut count = 0;
        for (index, gate) in self.gates.iter().enumerate() {
            let wire = FIRST_GATE + index;
            if !live[wire] {
                continue;
            }
            let [a, b, c] = gate.inputs.map(name);
            let expression = match gate.op {
                BitOp::Not => format!("!{a}"),
                BitOp::And => format!("{a} & {b}"),
                BitOp::Or => format!("{a} | {b}"),
                BitOp::Xor => format!("{a} ^ {b}"),
                BitOp::AndNot => format!("{a} & !{b}"),
                BitOp::Select => format!("({a} & {b}) | (!{a} & {c})"),
            };
            writeln!(source, "let gate{wire} = {expression};").unwrap();
            count += 1;
        }
        source.push_str("[\n");
        for word in output {
            writeln!(source, "[{}],", word.map(name).join(", ")).unwrap();
        }
        source.push_str("]\n}\n");
        count
    }
}

pub(super) fn compile(table: &RuleTable, phases: u32) -> (String, serde_json::Value) {
    let (model, output) = Mlp::program(table);
    let mut source = String::new();
    let truth: Vec<_> = model
        .neurons
        .iter()
        .map(|unit| unit.truth_table())
        .collect();
    let mut layers = vec![0; FIRST_GATE];
    let mut edge_count = 0;
    for unit in &model.neurons {
        let mut layer = 0;
        for (wire, weight) in unit.inputs.into_iter().zip(unit.weights) {
            if weight != 0 {
                layer = layer.max(layers[wire]);
                edge_count += 1;
            }
        }
        layers.push(layer + 1);
    }
    let mut gates_per_phase = Vec::new();
    for phase in 0..phases {
        let mut map: Vec<_> = (0..FIRST_GATE).collect();
        let clock = input_word(MEMORY + 3 - INPUT);
        for (bit, wire) in clock.into_iter().take(5).enumerate() {
            map[wire] = ((phase >> bit) & 1) as Wire;
        }
        let mut lowering = Lowering::default();
        for (unit, &table) in model.neurons.iter().zip(&truth) {
            let wire = lowering.unit(table, unit.inputs.map(|source| map[source]));
            map.push(wire);
        }
        let output = output.map(|word| word.map(|wire| map[wire]));
        gates_per_phase.push(lowering.write(output, phase, &mut source));
    }
    writeln!(source, "pub(super) const PROGRAMS: [Program; {phases}] = [").unwrap();
    for phase in 0..phases {
        writeln!(source, "phase_{phase},").unwrap();
    }
    source.push_str("];\n");
    let stats = serde_json::json!({
        "ruleRows": table.definition().rows.len(),
        "inputBits": INPUT_WORDS * WORD_BITS,
        "outputBits": OUTPUT_WORDS * WORD_BITS,
        "clippedReluNeurons": model.neurons.len(),
        "nonzeroWeights": edge_count,
        "depth": output.into_iter().flatten().map(|wire| layers[wire]).max().unwrap(),
        "packedIntegerGatesByPhase": gates_per_phase,
    });
    (source, stats)
}
