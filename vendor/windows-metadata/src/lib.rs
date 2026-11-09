#![doc = include_str!("../readme.md")]
#![expect(non_snake_case, non_upper_case_globals)]
#![allow(missing_docs)]

use std::cmp::Ordering;
use std::collections::*;

// A coded index must fit the largest table selected by its tag.
fn coded_index_size(tables: &[usize]) -> usize {
    fn small(row_count: usize, bits: u8) -> bool {
        (row_count as u64) < (1u64 << (16 - bits))
    }

    fn bits_needed(value: usize) -> u8 {
        let mut value = value - 1;
        let mut bits: u8 = 1;
        while {
            value >>= 1;
            value != 0
        } {
            bits += 1;
        }
        bits
    }

    let bits_needed = bits_needed(tables.len());

    if tables.iter().all(|table| small(*table, bits_needed)) {
        2
    } else {
        4
    }
}

mod attributes;
pub mod reader;
mod value;
pub mod writer;

pub use attributes::*;
pub use value::*;
mod bindings;
use bindings::*;

mod clr;
use clr::*;

mod ty;
pub use ty::*;

mod type_name;
pub use type_name::*;

mod signature;
pub use signature::*;

pub use reader::{AsRow, HasAttributes};

/// Metadata merge and namespace remapping support.
pub mod merge;

/// Creates a builder for combining winmd files.
pub fn merge() -> merge::Merger {
    merge::Merger::new()
}

/// Creates a [`merge::Remapper`] that rewrites a flat winmd into a header-based namespace
/// partition for `--package` generation.
pub fn remap() -> merge::Remapper {
    merge::Remapper::new()
}

/// Removes the generic arity suffix beginning with a backtick.
pub fn trim_tick(name: &str) -> &str {
    if let Some(pos) = name.find('`') {
        &name[..pos]
    } else {
        name
    }
}
