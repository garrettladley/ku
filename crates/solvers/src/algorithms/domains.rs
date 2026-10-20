use super::{
    Collector, Observer, cell_index, search_digit,
    topology::{MEMBERSHIPS, PEERS, UNIT_BITS},
};
use crate::{Board, TraceKind, algorithms::topology::ALL_DIGITS};

#[derive(Clone)]
pub(super) struct CandidateState<const TRACK_UNITS: bool> {
    pub cells: [u8; 81],
    pub masks: [u16; 81],
    pending: u128,
    dirty: u32,
}

pub(super) type Domains = CandidateState<false>;
pub(super) type LogicalDomains = CandidateState<true>;

impl<const TRACK_UNITS: bool> CandidateState<TRACK_UNITS> {
    pub fn new(board: &Board) -> Option<Self> {
        let mut used = [0; 27];
        for (&digit, memberships) in board.cells().iter().zip(&MEMBERSHIPS) {
            if digit != 0 {
                for &(unit, _) in memberships {
                    used[unit] |= 1 << digit;
                }
            }
        }
        let mut state = Self {
            cells: *board.cells(),
            masks: [0; 81],
            pending: 0,
            dirty: if TRACK_UNITS { (1 << 27) - 1 } else { 0 },
        };
        for (cell, memberships) in MEMBERSHIPS.iter().enumerate() {
            if board.cells()[cell] != 0 {
                continue;
            }
            let occupied = memberships
                .iter()
                .fold(0, |bits, &(unit, _)| bits | used[unit]);
            let mask = ALL_DIGITS & !occupied;
            if mask == 0 {
                return None;
            }
            state.masks[cell] = mask;
            if mask.is_power_of_two() {
                state.pending |= 1 << cell;
            }
        }
        Some(state)
    }

    #[cfg(test)]
    pub fn eliminate(&mut self, cell: usize, bits: u16) -> Option<bool> {
        let mut output = None;
        let _ = super::collect(std::num::NonZeroUsize::MIN, |collector| {
            output = Some(self.eliminate_observed(cell, bits, collector, 0));
        });
        output.unwrap()
    }

    pub fn eliminate_observed<O: Observer>(
        &mut self,
        cell: usize,
        bits: u16,
        collector: &mut Collector<O>,
        depth: usize,
    ) -> Option<bool> {
        let old = self.masks[cell];
        let mask = old & !bits;
        if mask == old {
            return Some(false);
        }
        if mask == 0 {
            collector.observe(
                || TraceKind::Contradiction,
                depth,
                || (self.cells, Some(self.masks)),
            );
            return None;
        }
        self.masks[cell] = mask;
        collector.observe(
            || TraceKind::Candidates,
            depth,
            || (self.cells, Some(self.masks)),
        );
        if collector.stopped() {
            return None;
        }
        self.mark_dirty(cell);
        if mask.is_power_of_two() {
            self.pending |= 1 << cell;
        }
        Some(true)
    }

    #[cfg(test)]
    pub fn assign(&mut self, cell: usize, bit: u16) -> Option<()> {
        let mut output = None;
        let _ = super::collect(std::num::NonZeroUsize::MIN, |collector| {
            output = Some(self.assign_observed(cell, bit, false, collector, 0));
        });
        output.unwrap()
    }

    pub fn assign_observed<O: Observer>(
        &mut self,
        cell: usize,
        bit: u16,
        forced: bool,
        collector: &mut Collector<O>,
        depth: usize,
    ) -> Option<()> {
        debug_assert!(bit.is_power_of_two());
        if self.masks[cell] & bit == 0 {
            return None;
        }
        self.masks[cell] = 0;
        self.mark_dirty(cell);
        self.cells[cell] = bit.trailing_zeros() as u8;
        collector.observe(
            || TraceKind::Assignment {
                cell: cell_index(cell),
                digit: search_digit(self.cells[cell]),
                forced,
            },
            depth,
            || (self.cells, Some(self.masks)),
        );
        if collector.stopped() {
            return None;
        }
        self.pending &= !(1 << cell);
        for &peer in &PEERS[cell] {
            self.eliminate_observed(peer, bit, collector, depth)?;
        }
        Some(())
    }

    #[cfg(test)]
    pub fn singles(&mut self, forced: &mut u64) -> Option<()> {
        let mut output = None;
        let result = super::collect(std::num::NonZeroUsize::MIN, |collector| {
            output = Some(self.singles_observed(collector, 0));
        });
        *forced += result.stats.forced_moves;
        output.unwrap()
    }

    pub fn singles_observed<O: Observer>(
        &mut self,
        collector: &mut Collector<O>,
        depth: usize,
    ) -> Option<()> {
        while self.pending != 0 {
            let cell = self.pending.trailing_zeros() as usize;
            self.assign_observed(cell, self.masks[cell], true, collector, depth)?;
            collector.result.stats.forced_moves += 1;
        }
        Some(())
    }

    fn mark_dirty(&mut self, cell: usize) {
        // Const specialization keeps dependency tracking out of forward checking.
        if TRACK_UNITS && self.dirty != (1 << 27) - 1 {
            self.dirty |= UNIT_BITS[cell];
        }
    }

    pub fn take_dirty(&mut self) -> u32 {
        std::mem::take(&mut self.dirty)
    }

    pub fn select(&self) -> Option<(usize, u16)> {
        // Singles can finish the board; a byte scan avoids MRV popcounts.
        if !TRACK_UNITS && !self.cells.contains(&0) {
            return None;
        }
        let mut best = None;
        let mut size = 10;
        for (cell, &mask) in self.masks.iter().enumerate() {
            if mask == 0 {
                continue;
            }
            let count = mask.count_ones();
            if count < size {
                best = Some((cell, mask));
                size = count;
                // Singles have been exhausted: two is the best possible choice.
                if count == 2 {
                    break;
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_removes_only_the_digit_from_twenty_peers() {
        let mut state = Domains::new(&Board::default()).unwrap();
        state.assign(40, 1 << 5).unwrap();
        for (cell, &mask) in state.masks.iter().enumerate() {
            assert_eq!(
                mask,
                if cell == 40 {
                    0
                } else if PEERS[40].contains(&cell) {
                    ALL_DIGITS & !(1 << 5)
                } else {
                    ALL_DIGITS
                }
            );
        }
        assert_eq!(state.cells[40], 5);
    }

    #[test]
    fn elimination_reports_no_change_and_detects_an_empty_domain() {
        let mut state = Domains::new(&Board::default()).unwrap();
        assert_eq!(state.eliminate(0, 1 << 5), Some(true));
        assert_eq!(state.eliminate(0, 1 << 5), Some(false));
        assert_eq!(state.eliminate(0, ALL_DIGITS), None);
    }

    #[test]
    fn pending_singles_cascade_across_both_halves_of_the_bitset() {
        let mut state = Domains::new(&Board::default()).unwrap();
        state.masks[80] = (1 << 1) | (1 << 2);
        state.masks[72] = (1 << 2) | (1 << 3);
        state.eliminate(8, ALL_DIGITS & !(1 << 1)).unwrap();
        let mut forced = 0;
        state.singles(&mut forced).unwrap();
        assert_eq!(forced, 3);
        assert_eq!(state.cells[8], 1);
        assert_eq!(state.cells[80], 2);
        assert_eq!(state.cells[72], 3);
        assert_eq!(state.pending, 0);
    }

    #[test]
    fn conflicting_pending_singles_are_a_contradiction() {
        let mut state = Domains::new(&Board::default()).unwrap();
        for cell in [0, 1] {
            state.eliminate(cell, ALL_DIGITS & !(1 << 1)).unwrap();
        }
        assert_eq!(state.singles(&mut 0), None);
    }

    #[test]
    fn mrv_ignores_assigned_cells_and_stops_at_a_pair() {
        let mut state = Domains::new(&Board::default()).unwrap();
        state.assign(0, 1 << 1).unwrap();
        state.masks[80] = (1 << 2) | (1 << 3);
        assert_eq!(state.select(), Some((80, (1 << 2) | (1 << 3))));
    }

    #[test]
    fn dirty_tracking_marks_only_units_whose_candidates_changed() {
        let mut state = LogicalDomains::new(&Board::default()).unwrap();
        assert_eq!(state.take_dirty(), (1 << 27) - 1);
        state.eliminate(40, 1 << 5).unwrap();
        let expected = (1 << 4) | (1 << 13) | (1 << 22);
        assert_eq!(state.take_dirty(), expected);
        state.eliminate(40, 1 << 5).unwrap();
        assert_eq!(state.take_dirty(), 0);
        let mut untracked = Domains::new(&Board::default()).unwrap();
        untracked.eliminate(40, 1 << 5).unwrap();
        assert_eq!(untracked.take_dirty(), 0);
    }

    #[test]
    fn singles_complete_both_the_digit_board_and_candidate_domains() {
        let solution: Board =
            "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
                .parse()
                .unwrap();
        let mut cells = *solution.cells();
        for cell in [0, 40, 80] {
            cells[cell] = 0;
        }
        let mut state = Domains::new(&Board::try_from(cells).unwrap()).unwrap();
        let mut forced = 0;
        state.singles(&mut forced).unwrap();
        assert_eq!(forced, 3);
        assert_eq!(&state.cells, solution.cells());
        assert_eq!(state.masks, [0; 81]);
        assert_eq!(state.select(), None);
    }
}
