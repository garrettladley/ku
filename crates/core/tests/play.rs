//! Player edits, accepted history replay, and mixed dataset parsing.

use ku_core::{Board, Game, GameError, GameEvent, dataset};

const PUZZLE: &str =
    "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

#[test]
fn imported_solution_can_be_played_and_every_accepted_prefix_replayed() {
    let records = dataset::parse(&format!(
        "# saved game\npuzzle,solution\n{PUZZLE},{SOLUTION}"
    ))
    .unwrap();
    let record = &records[0];
    let solution = record.solution().unwrap();
    let mut game = Game::new(record.puzzle().clone());
    for (cell, &given) in record.puzzle().cells().iter().enumerate() {
        if given != 0 {
            continue;
        }
        let before = game.clone();
        // Replaying stale input must not destroy an already accepted prefix.
        assert!(matches!(
            game.apply(&GameEvent::CellChanged {
                cell,
                before: 9,
                after: 0
            }),
            Err(GameError::ReplayMismatch { .. })
        ));
        assert_eq!(game, before);
        game.edit(cell, solution.cells()[cell]).unwrap().unwrap();
        assert_eq!(
            Game::replay(record.puzzle().clone(), game.events()).unwrap(),
            game
        );
    }
    assert_eq!(game.board(), solution);
    assert!(game.is_complete());
    assert_eq!(game.events().len(), 81 - record.puzzle().clue_count());

    game.edit(78, 0).unwrap(); // r9c7 is editable in the imported puzzle.
    assert!(!game.is_complete());
    let (puzzle, expected) = record.clone().into_parts();
    assert_eq!(Game::replay(puzzle, game.events()).unwrap(), game);
    assert_eq!(expected, Some(SOLUTION.parse::<Board>().unwrap()));
}

#[test]
fn mixed_dataset_formats_preserve_order_and_optional_solution_metadata() {
    let records = dataset::parse(&format!(
        "# sources\r\n\r\nquizzes,solutions\r\n {PUZZLE}, {SOLUTION} \r\n\
         {SOLUTION},\r\n012345ABCDEF {PUZZLE} 1.25\r\n{SOLUTION}\r\n"
    ))
    .unwrap();
    let puzzle: Board = PUZZLE.parse().unwrap();
    let solved: Board = SOLUTION.parse().unwrap();
    assert_eq!(records.len(), 4);
    assert_eq!(records[0].puzzle(), &puzzle);
    assert_eq!(records[0].solution(), Some(&solved));
    for (record, expected) in records[1..].iter().zip([&solved, &puzzle, &solved]) {
        assert_eq!(record.puzzle(), expected);
        assert_eq!(record.solution(), None);
    }
}
