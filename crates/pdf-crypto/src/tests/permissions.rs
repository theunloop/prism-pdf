//! The `/P` permissions flag word (§7.6.3.2, Table 22).

use super::*;

#[test]
fn permissions_bits_match_table_22() {
    assert_eq!(Permissions::ALL.bits(), -1);
    // RESTRICTED has the reserved bits (7,8,13..32) set and no grant bits.
    assert_eq!(Permissions::RESTRICTED.bits(), 0xFFFF_F0C0u32 as i32);
    assert_eq!(
        Permissions::RESTRICTED.allow_print().bits() & (1 << 2),
        1 << 2
    );
    assert_eq!(
        Permissions::RESTRICTED.allow_copy().bits() & (1 << 4),
        1 << 4
    );
    assert_eq!(
        Permissions::RESTRICTED.allow_print_high_res().bits() & (1 << 11),
        1 << 11
    );
    // Grants accumulate.
    let p = Permissions::RESTRICTED.allow_print().allow_copy();
    assert_eq!(p.bits() & (1 << 2 | 1 << 4), 1 << 2 | 1 << 4);
}
