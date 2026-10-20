//! Process-level solve, generate, verify, and import behavior.

use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

use ku_core::{Solver, dataset};
use ku_solvers::{DancingLinks, built_in_solvers};

const PUZZLE: &str =
    "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ku"))
        .args(args)
        .output()
        .unwrap()
}

struct TestDataset(PathBuf);

impl TestDataset {
    fn new(name: &str, contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ku-{name}-{}.txt", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        Self(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TestDataset {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn every_registered_solver_is_selectable_from_the_cli() {
    for &solver in built_in_solvers() {
        let output = run(&[
            "solve",
            PUZZLE,
            "--solver",
            solver.name(),
            "--unique",
            "--stats",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{SOLUTION}\n")
        );
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("solutions=1")
        );
    }
}

#[test]
fn board_can_be_read_from_stdin_and_pretty_output_can_be_parsed() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ku"))
        .args(["solve", "--pretty"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(PUZZLE.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let board: ku_core::Board = String::from_utf8(output.stdout).unwrap().parse().unwrap();
    assert_eq!(board.to_string(), SOLUTION);
}

#[test]
fn invalid_input_and_unknown_solver_exit_unsuccessfully() {
    for args in [
        vec!["solve", "123"],
        vec!["solve", PUZZLE, "--solver", "unknown"],
        vec!["generate", "--min-clues", "16"],
        vec!["generate", "--count", "0"],
    ] {
        let output = run(&args);
        assert!(!output.status.success());
        assert!(!output.stderr.is_empty());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn closed_output_pipe_exits_successfully_without_diagnostics() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ku"))
        .args(["generate", "--count", "100000", "--min-clues", "81"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn failed_batches_flush_the_successful_prefix_and_report_the_record() {
    let path = std::env::temp_dir().join(format!("ku-partial-cli-{}.txt", std::process::id()));
    let mut unsatisfiable = [0; 81];
    unsatisfiable[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    unsatisfiable[17] = 9;
    let unsatisfiable = ku_core::Board::try_from(unsatisfiable).unwrap();
    std::fs::write(&path, format!("{PUZZLE}\n{unsatisfiable}\n")).unwrap();
    let output = run(&["solve", "--file", path.to_str().unwrap()]);
    std::fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("puzzle,solution\n{PUZZLE},{SOLUTION}\n")
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("record 2: puzzle has no solution"));
}

#[test]
fn uniqueness_mode_rejects_ambiguous_boards() {
    let output = run(&["solve", &"0".repeat(81), "--unique"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("multiple solutions")
    );
}

#[test]
fn generated_csv_round_trips_and_has_independently_checked_solutions() {
    let output = run(&[
        "generate",
        "--seed",
        "42",
        "--count",
        "4",
        "--min-clues",
        "32",
    ]);
    assert!(output.status.success());
    let records = dataset::parse(&String::from_utf8(output.stdout).unwrap()).unwrap();
    assert_eq!(records.len(), 4);
    for record in records {
        let result = DancingLinks.search(record.puzzle(), std::num::NonZeroUsize::new(2).unwrap());
        assert_eq!(result.solutions_found(), 1);
        assert_eq!(result.solution(), record.solution());
    }
}

#[test]
fn verify_checks_dataset_solutions_with_registered_solvers() {
    let generated = run(&["generate", "--seed", "42", "--count", "32"]);
    assert!(generated.status.success());
    let file = TestDataset::new("verify", std::str::from_utf8(&generated.stdout).unwrap());
    let output = run(&["verify", file.path()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "verified 32 unique puzzles with all registered solvers\n"
    );
}

#[test]
fn importing_hash_puzzle_rating_format_preserves_the_expected_solution() {
    let file = TestDataset::new("import", &format!("000000000000 {PUZZLE} 1.0\n"));
    let output = run(&["import", file.path()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("puzzle,solution\n{PUZZLE},{SOLUTION}\n")
    );
}

#[test]
fn tui_help_describes_sources_and_solver_selection() {
    let output = run(&["tui", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for flag in ["--file", "--record", "--solver", "--watch"] {
        assert!(help.contains(flag));
    }
}

#[test]
fn tui_validates_arguments_before_touching_the_terminal() {
    for (args, message) in [
        (vec!["tui", PUZZLE, "--solver", "unknown"], "unknown solver"),
        (vec!["tui", PUZZLE, "--record", "2"], "out of range"),
        (vec!["tui", "--record", "0"], "invalid value"),
        (vec!["tui", "--record", "33"], "out of range"),
        (vec!["tui", "123"], "expected 81 cells"),
    ] {
        let output = run(&args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
        assert!(!output.stdout.contains(&0x1b));
    }
}

#[test]
fn tui_rejects_piped_launch_after_loading_each_existing_puzzle_source() {
    let file = TestDataset::new("tui-source", &format!("{PUZZLE}\n{PUZZLE}\n"));
    for args in [
        vec!["tui"],
        vec!["tui", PUZZLE],
        vec![
            "tui",
            "--file",
            file.path(),
            "--record",
            "2",
            "--watch",
            "--solver",
            "dlx",
        ],
    ] {
        let output = run(&args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
        assert!(!output.stdout.contains(&0x1b));
    }
}

#[test]
fn tui_reports_empty_dataset_before_terminal_setup() {
    let path = std::env::temp_dir().join(format!("ku-empty-tui-{}.csv", std::process::id()));
    std::fs::write(&path, "# no puzzles\n").unwrap();
    let output = run(&["tui", "--file", path.to_str().unwrap()]);
    std::fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("contains no puzzles"));
}

#[test]
fn batch_statistics_stay_on_stderr_and_puzzle_only_verification_proves_uniqueness() {
    let file = TestDataset::new("batch-stats", &format!("{PUZZLE}\n{SOLUTION}\n"));
    let output = run(&["solve", "--file", file.path(), "--unique", "--stats"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        format!("puzzle,solution\n{PUZZLE},{SOLUTION}\n{SOLUTION},{SOLUTION}\n").as_bytes()
    );
    let diagnostics = std::str::from_utf8(&output.stderr).unwrap();
    assert_eq!(diagnostics.lines().count(), 2);
    assert!(diagnostics.lines().all(|line| line.contains("solutions=1")));
    let verified = run(&["verify", file.path()]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    assert_eq!(
        verified.stdout,
        b"verified 2 unique puzzles with all registered solvers\n"
    );
}

#[test]
fn verification_and_import_reject_ambiguity_without_claiming_success() {
    let file = TestDataset::new("ambiguous-batch", &"0".repeat(81));
    for args in [vec!["verify", file.path()], vec!["import", file.path()]] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(1));
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("record 1"), "{error}");
        assert!(error.contains("multiple solutions"), "{error}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("verified"));
        assert!(
            dataset::parse(std::str::from_utf8(&output.stdout).unwrap())
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn a_valid_expected_solution_is_checked_even_without_uniqueness_mode() {
    let mut cells = *SOLUTION.parse::<ku_core::Board>().unwrap().cells();
    let rectangle = [3, 4, 30, 31];
    for cell in rectangle {
        cells[cell] = 0;
    }
    let puzzle = ku_core::Board::try_from(cells).unwrap();
    let first = ku_solvers::Bitmask.solve(&puzzle).unwrap();
    let mut alternate = *first.cells();
    for cell in rectangle {
        alternate[cell] = match alternate[cell] {
            6 => 7,
            7 => 6,
            _ => unreachable!(),
        };
    }
    let alternate = ku_core::Board::try_from(alternate).unwrap();
    assert!(alternate.is_solution_for(&puzzle));
    let file = TestDataset::new("expected-mismatch", &format!("{puzzle},{alternate}\n"));
    let output = run(&["solve", "--file", file.path(), "--solver", "bitmask"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"puzzle,solution\n");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("record 1: solution differs from expected dataset solution")
    );
}

#[test]
fn malformed_datasets_fail_before_emitting_a_successful_prefix() {
    let file = TestDataset::new("malformed-batch", &format!("# source\n{PUZZLE}\n123\n"));
    for args in [
        vec!["solve", "--file", file.path()],
        vec!["import", file.path()],
        vec!["verify", file.path()],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(file.path()), "{error}");
        assert!(error.contains("line 3"), "{error}");
    }
}

#[test]
fn generation_advances_seeds_and_wraps_at_the_u64_boundary() {
    let seed = u64::MAX.to_string();
    let output = run(&[
        "generate",
        "--seed",
        &seed,
        "--count",
        "2",
        "--min-clues",
        "81",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        run(&[
            "generate",
            "--seed",
            &seed,
            "--count",
            "2",
            "--min-clues",
            "81"
        ])
        .stdout,
        output.stdout
    );
    let records = dataset::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(records.len(), 2);
    for (record, seed) in records.iter().zip([u64::MAX, 0]) {
        let expected = ku_solvers::generate(seed, ku_solvers::MinClues::MAX).unwrap();
        assert_eq!(record.puzzle(), expected.puzzle());
        assert_eq!(record.solution(), Some(expected.solution()));
    }
}
