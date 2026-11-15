//! Module constants on Champ (#709): `const` values (scalars folded into literals; arrays, maps, nested and
//! empty typed collections laid out in the SPB 17 globals table) compiled through OIR-2, run in Champ and
//! compared with their case.json or with Tomato's output.
//!
//! Only pure language cases: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{check, run_source};

/// Scalar, rat, big int, string, char, enum, array, map, range-expanded and nested constants
/// (L06.const.values), and an `@@int` constant of an empty inner literal (L05.literal.empty-collections).
#[test]
fn champ_cases_module_constants() {
    check(&["L06.const.values", "L05.literal.empty-collections"]);
}

/// Constants read from several functions and recursively: scalar and big int, arrays of strings and bytes,
/// empty typed array and map constants, a map of enum cases, a nested array with an empty row, rats; the
/// expected output is Tomato's.
#[test]
fn champ_module_constants_of_every_shape() {
    let source = r#"module app v1.0.0;

import "io/stdout" take Stdout;

enum Size = (small, large);

const LIMIT = 3;
const BIG = 2 * 9223372036854775807;
const WORDS is @string = ["zero", "one", "two"];
const EMPTY is @int = [];
const EMPTY_MAP is %string:int = [:];
const SIZES is %string:Size = ["s": Size.small, "l": Size.large];
const GRID is @@int = [[1, 2], [], [3]];
const BYTES is @byte = [byte(7), byte(255)];
const HALVES = [1 / 2, 3 / 4];

object Table {
    Table();
    word(n is int) is string { WORDS[n]; }
    words(n is int) is string { if n >= LIMIT { ""; } else { self.word(n) + "," + self.words(n + 1); } }
    total(row is int) is int {
        if row >= GRID.size() { 0; } else { GRID[row].map() -> cell { cell * 10; }.size() + self.total(row + 1); }
    }
}

main() is int {
    let out = Stdout();
    let table = Table();
    out.writeln(table.words(0) + " " + BIG.string());
    out.writeln(EMPTY.size().string() + " " + EMPTY_MAP.size().string() + " " + GRID[1].size().string()
        + " " + table.total(0).string() + " " + GRID[2][0].string());
    let large = match SIZES["l"] else Size.small { case Size.small { "small"; } case Size.large { "large"; } };
    out.writeln(large + " " + BYTES[1].int().string() + " " + HALVES[1].string() + " "
        + (WORDS == ["zero", "one", "two"]).string());
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    assert_eq!(run.stdout, "zero,one,two, 18446744073709551614\n0 0 0 3 3\nlarge 255 3/4 true\n");
}
