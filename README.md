# ku

A Rust Sudoku toolkit for terminal play, solver visualization, unique puzzle
generation, and comparing six search strategies. Puzzles are generated locally;
no puzzle bank or network connection is needed to play.

## Get started

Install [Rust](https://rustup.rs/), then run from this checkout:

```sh
cargo install --path crates/cli --locked
ku tui --seed 42
```

The repository pins Rust 1.95.0. The TUI needs interactive stdin and stdout and a
terminal of at least 37 columns × 22 rows. It starts in Play with a locally
generated bank of 32 puzzles. Press **Tab** to watch a solver, **Space** to run
or pause, and **?** for help.

To run without installing:

```sh
cargo run -p ku-cli --release --locked -- tui --seed 42 --watch
```

## Play and watch

```sh
ku tui --seed 42 --solver bitset-cover --watch
ku tui --theme catppuccin-mocha
ku tui --file puzzles.csv --record 2
```

Generate `puzzles.csv` with the command in the next section, or supply your own
dataset. `--record` is one-based. You can also pass an 81-cell board directly to
`ku tui`.

| Action | Keys |
| --- | --- |
| Move | Arrows or `hjkl` |
| Enter / clear a digit | `1`–`9` / `0`, `.`, Backspace or Delete |
| Switch Play / Watch | Tab |
| Run / pause | Space |
| Step backward / forward | `[` / `]`; Left / Right also step in Watch |
| First / latest history | Home or `r` / End |
| Faster / slower playback | `+` / `-` |
| Choose solver / theme | `s` / `t`; arrows select, Enter applies, Esc cancels |
| Toggle details / help | `i` / `?` or F1 |
| Next puzzle / quit | `n` / `q` or Ctrl+C |

Given clues are locked and conflicting edits are rejected. Play history records
accepted moves; return to the latest state before editing. Watch runs from the
original puzzle while preserving your play session. It shows actual solver
events—assignments, guesses, contradictions, and backtracking—and lets you rewind
the recorded history. Interactive histories stay in memory.

## Solve and generate

```sh
# Solve a board and require a unique solution.
ku solve 530070000600195000098000060800060003400803001700020006060000280000419005000080079 --unique --pretty

# Read a board from stdin instead.
printf '%s\n' '530070000600195000098000060800060003400803001700020006060000280000419005000080079' | ku solve

# Generate puzzle,solution CSV; verify uniqueness with all six solvers.
ku generate --seed 42 --count 10 --min-clues 32 > puzzles.csv
ku verify puzzles.csv

# Solve a dataset with a selected strategy; search counters go to stderr.
ku solve --file puzzles.csv --solver dlx --unique --stats
```

Boards contain 81 row-major digits; `0` and `.` denote empty cells, and whitespace
is accepted. `solve` returns the first solution unless `--unique` is supplied.
Invalid givens, unsatisfiable boards, and nonunique boards in uniqueness mode
exit unsuccessfully.

Generation removes clues from a randomized complete grid, keeping each removal
only when the puzzle remains unique. `--min-clues` accepts 17–81 and is a lower
bound: a puzzle may retain more clues. **Clue count is not a difficulty rating.**
There is no easy/medium/hard grader. A seed reproduces output with the locked
dependencies on the same target; generated grids are not uniformly sampled.

### Bring your own puzzles

`solve --file`, `tui --file`, `verify`, and `import` accept:

- One 81-cell puzzle per line.
- Unquoted `puzzle,solution` CSV, including the `quizzes,solutions` header variant.
- Sudoku Exchange `hash puzzle rating` records.

For a file you supply, `ku import my-puzzles.txt > imported.csv` writes
uniqueness-checked `puzzle,solution` CSV. Provided solutions are validated against
the puzzle. `verify` requires every registered solver to find the same unique
solution. Batch commands can write a successful prefix before failing; check the
exit status before using their output.

`solve`, `tui`, and `import` also accept `--url URL` to load an HTTP(S) dataset.
Use `ku <command> --help` for the complete options.

## Solvers and libraries

All six strategies support solution counting and tracing. The default is
`bitmask`; select another with `--solver`.

| CLI name | Strategy |
| --- | --- |
| `backtracking` | Peer scanning with minimum-remaining-values cell selection |
| `bitmask` | Bitmask search with naked and hidden singles |
| `dlx` | Algorithm X with dancing links in an index arena |
| `forward-checking` | Incremental peer elimination and queued naked singles |
| `constraint-propagation` | Singles, naked pairs, and locked candidates before branching |
| `bitset-cover` | Algorithm X with nine-bit supports for 324 constraints |

The four crates separate Sudoku rules, search, application state, and terminal
I/O. Arrows point to dependencies:

```text
ku-cli ──▶ ku-solvers ──▶ ku-core
   │
   ├─────▶ ku-session ──▶ ku-core
   │
   └───────────────────▶ ku-core
```

[`ku-core`](crates/core/src/lib.rs) validates boards through `FromStr` and
`TryFrom` and defines the `Solver` and `TraceSolver` interfaces. Search accepts a
nonzero solution limit; a limit of two distinguishes unsatisfiable, unique, and
nonunique puzzles. Add strategies to the shared
[`registry`](crates/solvers/src/registry.rs) to expose them to the CLI, tests, and
benchmarks. [`ku-solvers`](crates/solvers/src/lib.rs) returns generated puzzles as
`Result<GeneratedPuzzle, GenerateError>`. Both generation functions take a
`MinClues` newtype, validated in 17–81 through `TryFrom<usize>` or `FromStr`;
`generate_with_solver` accepts a custom `Solver` for uniqueness checks.

`Bitmask`, `Backtracking`, and `ForwardChecking` also implement `GridGenerator`
for randomized complete grids. Use `Generator::new(&ForwardChecking, &DancingLinks)`
to choose grid construction and uniqueness checking independently, then call
`.generate(seed, min_clues)`. The convenience functions use `Bitmask` for grid
construction. See the [generator API](crates/solvers/src/generator.rs) for a
runnable Rust example.

[`ku-session`](crates/session/src/lib.rs) supports deterministic simulation and
JSON replay without terminal I/O or solver implementations. Executable examples
show [replay](crates/session/examples/replay.rs),
[session simulation](crates/cli/examples/simulate.rs), and
[terminal rendering](crates/cli/examples/preview.rs). Build API documentation with
`cargo doc --workspace --no-deps --locked --open`.

Run the examples from the repository root; generated files stay under `target/`:

```sh
cargo run -p ku-cli --example simulate --locked -- target/session-history.json
cargo run -p ku-session --example replay --locked -- target/session-history.json
cargo run -p ku-cli --example preview --locked -- 37 22
```

The simulation saves a session that the replay example validates in a separate
process. The preview renders text through an in-memory terminal backend. Neither
workflow needs an interactive terminal or a network connection.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo test --workspace --all-features --release --locked
```

The optional [`justfile`](justfile) provides `just check` for formatting, Clippy,
debug/release tests, rustdoc, and dependency/license checks. It requires
[Just](https://just.systems/) and `cargo-deny`
(`cargo install cargo-deny --version 0.19.0 --locked`, matching CI).
`just msrv` checks and tests with
Rust 1.94.0, which must be installed separately.

`cargo test` is self-contained: tests generate their own puzzles, create temporary
datasets, and serve HTTP fixtures on loopback. No downloaded puzzle bank, external
service, or preparatory script is required. The HTTP tests need permission to bind
a local port.

Tests and benchmarks generate their workloads in Rust from fixed seeds. Solver
and trace tests check 128 generated puzzles against every built-in strategy,
alongside invalid, unsatisfiable, ambiguous, and complete-board cases.

Coverage uses [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) with the
pinned Rust toolchain. Install the optional tooling once, then run:

```sh
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --version 0.9.1 --locked
just coverage                 # workspace tests and a 94% line coverage floor
just coverage --html          # annotated report in target/llvm-cov/html
```

CI enforces the same coverage floor. Integration tests in each crate's `tests/`
exercise public contracts and cross-crate workflows; in-module tests cover
deduction preconditions, history validation, and wire-format boundaries.
Cancellation tests interrupt every event in a branching search, and portable
session tests replay real traces from every registered solver. Coverage helps
locate gaps; assertions about results, rejected inputs, and unchanged state
provide the refactoring guarantees. Interactive terminal setup still requires
a manual smoke test.

```sh
just bench                    # first-solution and uniqueness searches
just bench-batch              # serial vs. reused Rayon worker pools
just bench-edges              # complete, sparse, ambiguous, unsatisfiable
just asm                      # optimized solver assembly in target/release/deps
just compare puzzles.csv 100  # per-puzzle latency, search work, allocations
```

Criterion generates inputs before timing. Times are per batch, with throughput
reported in puzzles; results are written under `target/criterion`. Use your
own datasets with `just compare` to measure behavior on your workload.
The solve benchmark defaults to 32 puzzles per clue floor (48, 36, 28, 22).
Use `KU_BENCH_SEED_OFFSET` and `KU_BENCH_PUZZLES` to check independent generated
samples, for example:

```sh
KU_BENCH_SEED_OFFSET=100000 KU_BENCH_PUZZLES=64 just bench constraint-propagation --save-baseline holdout
```

Clue floors are generation controls, not human difficulty ratings.

## License

[MIT](LICENSE).
