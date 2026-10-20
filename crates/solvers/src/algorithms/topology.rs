// Fixed Sudoku topology used privately by search implementations.
pub(super) const fn box_index(cell: usize) -> usize {
    (cell / 27) * 3 + (cell % 9) / 3
}

pub(super) const ALL_DIGITS: u16 = 0b11_1111_1110;
pub(super) const UNITS: [[usize; 9]; 27] = make_units();

const fn make_units() -> [[usize; 9]; 27] {
    let mut units = [[0; 9]; 27];
    let mut unit = 0;
    while unit < 9 {
        let mut offset = 0;
        while offset < 9 {
            units[unit][offset] = unit * 9 + offset;
            units[9 + unit][offset] = offset * 9 + unit;
            units[18 + unit][offset] = (unit / 3 * 3 + offset / 3) * 9 + unit % 3 * 3 + offset % 3;
            offset += 1;
        }
        unit += 1;
    }
    units
}

// Compile Sudoku's fixed graph once; search need not rediscover peers or divide.
pub(super) const PEERS: [[usize; 20]; 81] = make_peers();
pub(super) const MEMBERSHIPS: [[(usize, usize); 3]; 81] = make_memberships();
pub(super) const UNIT_BITS: [u32; 81] = make_unit_bits();

const fn make_unit_bits() -> [u32; 81] {
    let mut bits = [0; 81];
    let mut cell = 0;
    while cell < 81 {
        let mut member = 0;
        while member < 3 {
            bits[cell] |= 1 << MEMBERSHIPS[cell][member].0;
            member += 1;
        }
        cell += 1;
    }
    bits
}

const fn make_peers() -> [[usize; 20]; 81] {
    let mut peers = [[0; 20]; 81];
    let mut cell = 0;
    while cell < 81 {
        let mut other = 0;
        let mut count = 0;
        while other < 81 {
            if cell != other
                && (cell / 9 == other / 9
                    || cell % 9 == other % 9
                    || box_index(cell) == box_index(other))
            {
                peers[cell][count] = other;
                count += 1;
            }
            other += 1;
        }
        cell += 1;
    }
    peers
}

const fn make_memberships() -> [[(usize, usize); 3]; 81] {
    let mut memberships = [[(0, 0); 3]; 81];
    let mut cell = 0;
    while cell < 81 {
        memberships[cell] = [
            (cell / 9, cell % 9),
            (9 + cell % 9, cell / 9),
            (18 + box_index(cell), cell / 9 % 3 * 3 + cell % 3),
        ];
        cell += 1;
    }
    memberships
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peers_are_complete_unique_symmetric_and_exclude_self() {
        for (cell, peers) in PEERS.iter().enumerate() {
            let expected: Vec<_> = (0..81)
                .filter(|&other| {
                    other != cell
                        && (cell / 9 == other / 9
                            || cell % 9 == other % 9
                            || box_index(cell) == box_index(other))
                })
                .collect();
            assert_eq!(peers.as_slice(), expected);
            for &peer in peers {
                assert!(PEERS[peer].contains(&cell));
            }
        }
    }

    #[test]
    fn memberships_round_trip_through_each_unit() {
        for (cell, memberships) in MEMBERSHIPS.iter().enumerate() {
            let mut bits = 0;
            for &(unit, position) in memberships {
                assert_eq!(UNITS[unit][position], cell);
                bits |= 1 << unit;
            }
            assert_eq!(UNIT_BITS[cell], bits);
        }
    }
}
