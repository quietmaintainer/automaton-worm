//! Packed integer inference for a cell rule programmed as a sparse MLP.

use crate::world::CellInput;
use crate::{MEMORY_WORDS, PORTS};

pub(crate) const WORD_BITS: usize = u32::BITS as usize;
pub(crate) const INPUT_WORDS: usize = 1 + PORTS + MEMORY_WORDS + 1;
pub(crate) const OUTPUT_WORDS: usize = MEMORY_WORDS + PORTS + 1;
pub(crate) const LANES: usize = u64::BITS as usize;

pub(crate) type InputPlanes = [[u64; WORD_BITS]; INPUT_WORDS];
pub(crate) type OutputPlanes = [[u64; WORD_BITS]; OUTPUT_WORDS];
pub(crate) type Program = fn(&InputPlanes) -> OutputPlanes;

pub(crate) fn run(program: Program, inputs: &[CellInput], next: &mut [crate::CellState]) {
    debug_assert_eq!(inputs.len(), next.len());
    debug_assert!(inputs.len() <= LANES);
    let mut words = [[0; WORD_BITS]; INPUT_WORDS];
    let (low, high) = inputs.split_at(inputs.len().min(WORD_BITS));
    pack_half::<0>(low, &mut words);
    pack_half::<32>(high, &mut words);
    words.iter_mut().for_each(transpose);

    let mut outputs = program(&words);
    outputs.iter_mut().for_each(transpose);
    let (low, high) = next.split_at_mut(next.len().min(WORD_BITS));
    unpack_half::<0>(&outputs, low);
    unpack_half::<32>(&outputs, high);
}

fn pack_half<const SHIFT: u32>(inputs: &[CellInput], words: &mut InputPlanes) {
    for (lane, &(input, inbox, cell)) in inputs.iter().enumerate() {
        words[0][lane] |= u64::from(input) << SHIFT;
        for (word, value) in words[1..]
            .iter_mut()
            .zip(inbox.into_iter().chain(cell.memory).chain([cell.answer]))
        {
            word[lane] |= u64::from(value) << SHIFT;
        }
    }
}

fn unpack_half<const SHIFT: u32>(words: &OutputPlanes, cells: &mut [crate::CellState]) {
    for (lane, cell) in cells.iter_mut().enumerate() {
        let read = |word: usize| (words[word][lane] >> SHIFT) as u32;
        cell.memory = std::array::from_fn(read);
        cell.outbox = std::array::from_fn(|port| read(MEMORY_WORDS + port));
        cell.answer = read(OUTPUT_WORDS - 1);
    }
}

// Exchange the row and bit indexes in both 32-bit halves at once. The same
// transform packs the lanes into bit planes and unpacks the output planes.
fn transpose(words: &mut [u64; WORD_BITS]) {
    fn exchange<const SHIFT: usize, const MASK: u64>(words: &mut [u64; WORD_BITS]) {
        for block in words.chunks_exact_mut(2 * SHIFT) {
            let (lower, upper) = block.split_at_mut(SHIFT);
            for (lo, hi) in lower.iter_mut().zip(upper) {
                let swap = ((*lo >> SHIFT) ^ *hi) & MASK;
                *lo ^= swap << SHIFT;
                *hi ^= swap;
            }
        }
    }

    exchange::<16, 0x0000_ffff_0000_ffff>(words);
    exchange::<8, 0x00ff_00ff_00ff_00ff>(words);
    exchange::<4, 0x0f0f_0f0f_0f0f_0f0f>(words);
    exchange::<2, 0x3333_3333_3333_3333>(words);
    exchange::<1, 0x5555_5555_5555_5555>(words);
}
