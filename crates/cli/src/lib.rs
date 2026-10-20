//! Sudoku play, solver visualization, generation, and dataset checks.
//!
//! [`run_cli`] is the process-argument entry point. Embedders can use [`tui`]'s
//! independent input, output, solver-source, and lifecycle adapters. Domain types
//! live in [`ku_core`], strategies in [`ku_solvers`], and the deterministic model
//! and portable replay in [`ku_session`].
//!
//! # Running a session without a terminal
//!
//! Scripted input exercises the ordinary driver. A recording captures accepted
//! activations and cleanup, so strict replay reconstructs the final state.
//!
//! ```
//! use ku_cli::tui::{
//!     app, input::Action, session::{Session, SessionExit},
//!     simulation::{RecordingOutput, ScriptedInput, SessionRecording},
//!     source::WorkerSource,
//! };
//! use ku_core::Board;
//! use ku_session::history::ReplayMode;
//!
//! let mut app = app::new(vec![Board::default().into()], 0, "example".into(), "bitmask")?;
//! let mut input = ScriptedInput::new([Action::ToggleSidebar, Action::Quit]);
//! let mut source = WorkerSource::default();
//! let mut output = RecordingOutput::default();
//! let mut recording = SessionRecording::default();
//! {
//!     let mut session = Session::new(&mut app, &mut input, &mut source,
//!         &mut output, &mut recording);
//!     assert_eq!(session.run()?, SessionExit::Quit);
//! }
//! assert!(app.sidebar_open());
//! assert!(app.closed());
//! assert!(!output.frames().is_empty());
//! assert_eq!(recording.replay(ReplayMode::Strict)?.snapshot(), app.snapshot());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::{
    io::{self, BufWriter, Read, Write},
    num::NonZeroUsize,
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use ku_core::{Board, SearchResult, Solver, dataset::Record};
use ku_solvers::{MinClues, built_in_solvers, generate};

const UNIQUE_LIMIT: NonZeroUsize = NonZeroUsize::MIN.saturating_add(1);

mod dataset_source;
use dataset_source::{HttpUrl, load_file, load_url};

pub mod tui;

#[derive(Parser)]
#[command(
    name = "ku",
    version,
    about = "Play, solve, generate, and visualize Sudoku puzzles"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Play a puzzle and watch the existing solvers in an interactive terminal.
    Tui {
        /// A single board; otherwise load a source or generate a fresh puzzle bank.
        #[arg(conflicts_with_all = ["file", "url", "seed"])]
        puzzle: Option<Board>,
        /// Load puzzle lines, CSV, or Sudoku Exchange records.
        #[arg(short, long)]
        file: Option<PathBuf>,
        /// Download a UTF-8 puzzle-line, CSV, or hash/puzzle/rating dataset.
        #[arg(long, conflicts_with = "file")]
        url: Option<HttpUrl>,
        /// Seed for the generated default bank of 32 unique puzzles.
        #[arg(long, default_value_t = 0, conflicts_with_all = ["file", "url"])]
        seed: u64,
        /// One-based starting puzzle in the selected dataset.
        #[arg(long, default_value = "1")]
        record: NonZeroUsize,
        #[arg(short, long, alias = "algorithm", default_value = "bitmask", value_parser = parse_solver_name)]
        solver: &'static dyn Solver,
        /// Start in solver watch mode instead of play mode.
        #[arg(long)]
        watch: bool,
        /// Color palette (press t in the TUI to browse all installed themes).
        #[arg(long, default_value = "ku", value_parser = tui::theme::parse_name)]
        theme: String,
    },
    /// Solve a board argument, a dataset file, or a board read from stdin.
    Solve {
        /// 81 digits; zero or dot denotes an empty cell. Whitespace is accepted.
        #[arg(conflicts_with_all = ["file", "url"])]
        puzzle: Option<Board>,
        /// Solve every record in a puzzle-line, CSV, or Sudoku Exchange file.
        #[arg(short, long, conflicts_with = "pretty")]
        file: Option<PathBuf>,
        /// Download and solve a UTF-8 dataset over HTTP(S).
        #[arg(long, conflicts_with_all = ["file", "pretty"])]
        url: Option<HttpUrl>,
        #[arg(short, long, alias = "algorithm", default_value = "bitmask", value_parser = parse_solver_name)]
        solver: &'static dyn Solver,
        /// Reject puzzles with more than one solution.
        #[arg(short, long)]
        unique: bool,
        /// Print a spaced 9×9 grid.
        #[arg(short, long)]
        pretty: bool,
        /// Print search work to stderr.
        #[arg(long)]
        stats: bool,
    },
    /// Generate puzzle/solution CSV with uniqueness preserved after each removal.
    Generate {
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Lower bound on clues (17..=81); may finish above this bound.
        #[arg(long, default_value_t = MinClues::default())]
        min_clues: MinClues,
        #[arg(short, long, default_value = "1")]
        count: NonZeroUsize,
    },
    /// Check uniqueness and expected solutions using all registered solvers.
    Verify { file: PathBuf },
    /// Convert a dataset to puzzle/solution CSV, rejecting nonunique puzzles.
    Import {
        /// Local input file; alternatively supply --url.
        #[arg(required_unless_present = "url", conflicts_with = "url")]
        file: Option<PathBuf>,
        /// Download a dataset and convert it to uniqueness-checked CSV.
        #[arg(long)]
        url: Option<HttpUrl>,
        #[arg(short, long, alias = "algorithm", default_value = "bitmask", value_parser = parse_solver_name)]
        solver: &'static dyn Solver,
    },
}

/// Parses process arguments and executes the requested command.
///
/// # Errors
/// Returns contextual errors for malformed input, I/O, unsatisfiable puzzles,
/// or nonunique puzzles when uniqueness was requested.
pub fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    if matches!(&cli.command, Command::Tui { .. }) {
        // A worker's panic hook restores the terminal and writes diagnostics.
        // Holding stdio locks on the UI thread would deadlock that cleanup.
        return execute(cli, &mut io::sink(), &mut io::sink());
    }
    let mut output = BufWriter::new(io::stdout().lock());
    let mut diagnostics = io::stderr().lock();
    let result = execute(cli, &mut output, &mut diagnostics);
    // Drop ignores flush errors. Explicitly flush even a successful prefix of a
    // failed batch, while preserving the command's original error.
    let flushed = output.flush().context("writing output");
    result.and(flushed)
}

fn execute(cli: Cli, output: &mut impl Write, diagnostics: &mut impl Write) -> Result<()> {
    match cli.command {
        Command::Tui {
            puzzle,
            file,
            url,
            seed,
            record,
            solver,
            watch,
            theme,
        } => {
            let (records, label) = match (puzzle, file, url) {
                (Some(puzzle), _, _) => (vec![Record::from(puzzle)], "board argument".to_owned()),
                (_, Some(file), _) => (load_file(&file)?, file.display().to_string()),
                (_, _, Some(url)) => (load_url(&url)?, url.to_string()),
                _ => (
                    generated_bank(seed)?,
                    format!("generated bank (seed {seed})"),
                ),
            };
            ensure!(
                record.get() <= records.len(),
                "record {} is out of range; dataset contains {} puzzles",
                record,
                records.len()
            );
            let config = tui::app::AppConfig::new(
                records,
                record.get() - 1,
                label,
                solver.name(),
                tui::app::solver_catalog(),
                ku_core::HistoryLimits::default(),
                if watch {
                    tui::app::Mode::Watch
                } else {
                    tui::app::Mode::Play
                },
                NonZeroUsize::MIN.saturating_add(7),
                50,
            )?;
            let config = tui::theme::configure(config, &theme)?;
            let app = tui::app::App::from_config(config);
            return tui::run(app).context("running Sudoku TUI");
        }
        Command::Solve {
            puzzle,
            file,
            url,
            solver,
            unique,
            pretty,
            stats,
        } => {
            if let Some(records) = file
                .map(|file| load_file(&file))
                .or_else(|| url.map(|url| load_url(&url)))
                .transpose()?
            {
                writeln!(output, "puzzle,solution")?;
                for (index, record) in records.iter().enumerate() {
                    let result = solve_record(record, solver, unique)
                        .with_context(|| format!("record {}", index + 1))?;
                    let solution = result.solution().context("no solution")?;
                    writeln!(output, "{},{}", record.puzzle(), solution)?;
                    if stats {
                        print_stats(diagnostics, solver, &result)?;
                    }
                }
            } else {
                let puzzle = match puzzle {
                    Some(puzzle) => puzzle,
                    None => {
                        let mut input = String::new();
                        io::stdin()
                            .read_to_string(&mut input)
                            .context("reading board from stdin")?;
                        input.parse().context("parsing board from stdin")?
                    }
                };
                let result = solve_record(&Record::from(puzzle), solver, unique)?;
                let solution = result.solution().context("no solution")?;
                if pretty {
                    writeln!(output, "{solution:#}")?;
                } else {
                    writeln!(output, "{solution}")?;
                }
                if stats {
                    print_stats(diagnostics, solver, &result)?;
                }
            }
        }
        Command::Generate {
            seed,
            min_clues,
            count,
        } => {
            writeln!(output, "puzzle,solution")?;
            for offset in 0..count.get() {
                let generated = generate(seed.wrapping_add(offset as u64), min_clues)?;
                writeln!(output, "{},{}", generated.puzzle(), generated.solution())?;
            }
        }
        Command::Verify { file } => {
            let records = load_file(&file)?;
            for (index, record) in records.iter().enumerate() {
                let mut reference = record.solution().cloned();
                for &solver in built_in_solvers() {
                    let result = solve_record(record, solver, true)
                        .with_context(|| format!("record {}, {}", index + 1, solver.name()))?;
                    if let Some(expected) = &reference {
                        ensure!(
                            result.solution() == Some(expected),
                            "record {}: solvers disagree",
                            index + 1
                        );
                    } else {
                        reference = result.solution().cloned();
                    }
                }
            }
            writeln!(
                output,
                "verified {} unique puzzles with all registered solvers",
                records.len()
            )?;
        }
        Command::Import { file, url, solver } => {
            let records = match (file, url) {
                (Some(file), _) => load_file(&file)?,
                (_, Some(url)) => load_url(&url)?,
                _ => anyhow::bail!("import requires a file or --url"),
            };
            writeln!(output, "puzzle,solution")?;
            for (index, record) in records.iter().enumerate() {
                let result = solve_record(record, solver, true)
                    .with_context(|| format!("record {}", index + 1))?;
                let solution = result.solution().context("no solution")?;
                writeln!(output, "{},{}", record.puzzle(), solution)?;
            }
        }
    }
    Ok(())
}

fn generated_bank(seed: u64) -> Result<Vec<Record>> {
    (0..32)
        .map(|offset| {
            let (puzzle, solution) =
                generate(seed.wrapping_add(offset), MinClues::default())?.into_parts();
            Ok(Record::new(puzzle, Some(solution))?)
        })
        .collect()
}

fn solve_record(record: &Record, solver: &dyn Solver, unique: bool) -> Result<SearchResult> {
    let limit = if unique {
        UNIQUE_LIMIT
    } else {
        NonZeroUsize::MIN
    };
    let result = solver.search(record.puzzle(), limit);
    ensure!(result.solutions_found() != 0, "puzzle has no solution");
    ensure!(
        !unique || result.unique_solution().is_some(),
        "puzzle has multiple solutions"
    );
    if let Some(expected) = record.solution() {
        ensure!(
            result.solution() == Some(expected),
            "solution differs from expected dataset solution"
        );
    }
    Ok(result)
}

fn print_stats(output: &mut impl Write, solver: &dyn Solver, result: &SearchResult) -> Result<()> {
    let stats = result.stats();
    writeln!(
        output,
        "{}: solutions={}, nodes={}, guesses={}, backtracks={}, forced={}, depth={}",
        solver.name(),
        result.solutions_found(),
        stats.nodes,
        stats.guesses,
        stats.backtracks,
        stats.forced_moves,
        stats.max_depth
    )
    .context("writing search statistics")
}

fn parse_solver_name(name: &str) -> std::result::Result<&'static dyn Solver, String> {
    if let Some(solver) = built_in_solvers()
        .iter()
        .copied()
        .find(|solver| solver.name() == name)
    {
        Ok(solver)
    } else {
        let names = built_in_solvers()
            .iter()
            .map(|solver| solver.name())
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "unknown solver {name:?}; available solvers: {names}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUZZLE: &str =
        "530070000600195000098000060800060003400803001700020006060000280000419005000080079";

    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn output_errors_propagate_as_io_errors() {
        let cli = Cli::try_parse_from(["ku", "solve", PUZZLE]).unwrap();
        let error = execute(cli, &mut FailingWriter, &mut Vec::new()).unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn statistics_errors_propagate_without_panicking() {
        let cli = Cli::try_parse_from(["ku", "solve", PUZZLE, "--stats"]).unwrap();
        let error = execute(cli, &mut Vec::new(), &mut FailingWriter).unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(error.to_string(), "writing search statistics");
    }

    #[test]
    fn invalid_clue_bounds_are_rejected_during_argument_parsing() {
        for clues in ["0", "16", "82", "256", "many", "17.5"] {
            assert!(Cli::try_parse_from(["ku", "generate", "--min-clues", clues]).is_err());
        }
    }

    #[test]
    fn generated_bank_is_reproducible_and_each_record_is_unique() {
        let records = generated_bank(u64::MAX).unwrap();
        let repeated = generated_bank(u64::MAX).unwrap();
        assert_eq!(records.len(), 32);
        for (record, again) in records.iter().zip(&repeated) {
            assert_eq!(record.puzzle(), again.puzzle());
            assert_eq!(record.solution(), again.solution());
            let result = ku_solvers::DancingLinks.search(record.puzzle(), UNIQUE_LIMIT);
            assert_eq!(result.unique_solution(), record.solution());
        }
    }

    #[test]
    fn tui_theme_argument_is_canonical_and_invalid_palettes_fail_before_startup() {
        let cli = Cli::try_parse_from(["ku", "tui", "--theme", "catppuccin-mocha"]).unwrap();
        assert!(matches!(cli.command, Command::Tui { theme, .. } if theme == "catppuccin-mocha"));
        let error = Cli::try_parse_from(["ku", "tui", "--theme", "missing"])
            .err()
            .expect("unknown themes must fail argument parsing");
        assert!(error.to_string().contains("one-dark-pro"));
    }
}
