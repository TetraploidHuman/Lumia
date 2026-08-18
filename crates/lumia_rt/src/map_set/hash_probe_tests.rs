//! Unit tests for open-hash probe helpers.

use super::*;

#[test]
fn empty_cap_is_none() {
    let words = [0i64, 0];
    unsafe {
        assert_eq!(open_hash_find_slot(words.as_ptr(), 0, 1, false, 2, 1), None);
        assert_eq!(
            open_hash_claim_slot(words.as_ptr() as *mut i64, 0, 1, false, 2, 1),
            None
        );
    }
}

#[test]
fn finds_full_set_style_cell() {
    use super::super::tid::key_hash;
    let key = 7i64;
    let cap = 4usize;
    let idx = (key_hash(key, false) as usize) % cap;
    // [n][cap] + order[cap] + (elem,state)[cap]
    let mut words = vec![1i64, cap as i64];
    words.extend(std::iter::repeat_n(0i64, cap));
    words.extend(std::iter::repeat_n(0i64, cap * 2));
    let cell = 2 + cap + idx * 2;
    words[cell] = key;
    words[cell + 1] = OPEN_HASH_ST_FULL;
    unsafe {
        assert_eq!(
            open_hash_find_slot(words.as_ptr(), cap, key, false, 2, 1),
            Some(idx)
        );
        assert_eq!(
            open_hash_find_slot(words.as_ptr(), cap, 99, false, 2, 1),
            None
        );
    }
}

#[test]
fn claim_slot_reuses_tomb_and_sets_full() {
    use super::super::tid::key_hash;
    let key = 42i64;
    let cap = 4usize;
    let idx = (key_hash(key, false) as usize) % cap;
    let mut words = vec![0i64, cap as i64];
    words.extend(std::iter::repeat_n(0i64, cap));
    words.extend(std::iter::repeat_n(0i64, cap * 2));
    let cell = 2 + cap + idx * 2;
    words[cell + 1] = OPEN_HASH_ST_TOMB;
    unsafe {
        let (got, cell_ptr) =
            open_hash_claim_slot(words.as_mut_ptr(), cap, key, false, 2, 1).expect("claim");
        assert_eq!(got, idx);
        assert_eq!(*cell_ptr, key);
        assert_eq!(*cell_ptr.add(1), OPEN_HASH_ST_FULL);
    }
}

#[test]
fn remove_slot_tombs_and_compacts_order() {
    let cap = 4usize;
    // [n][cap] + order[cap] + (elem,state)[cap]
    let mut words = vec![3i64, cap as i64];
    words.extend([0i64, 1, 2, -1]);
    words.extend(std::iter::repeat_n(0i64, cap * 2));
    words[2 + cap] = 10;
    words[2 + cap + 1] = OPEN_HASH_ST_FULL;
    words[2 + cap + 2] = 20;
    words[2 + cap + 3] = OPEN_HASH_ST_FULL;
    words[2 + cap + 4] = 30;
    words[2 + cap + 5] = OPEN_HASH_ST_FULL;
    unsafe {
        open_hash_remove_slot(words.as_mut_ptr(), cap, 1, 3, 2, 1);
        assert_eq!(words[0], 2);
        assert_eq!(&words[2..4], &[0, 2]);
        assert_eq!(words[2 + cap + 3], OPEN_HASH_ST_TOMB);
        assert_eq!(words[2 + cap + 2], 20);
    }
}

#[test]
fn from_linear_sizes_cap_and_visits_entries() {
    // Linear set-style: [n][e0][e1]… — alloc records chosen cap; put records indices.
    let mut linear = vec![10i64];
    linear.extend(0..10);
    let mut seen_cap = 0usize;
    let mut seen_idx = Vec::new();
    let dest = unsafe {
        open_hash_from_linear(
            linear.as_mut_ptr() as *mut u8,
            1, // extra slot → n2=11 → cap grows past 16
            |cap| {
                seen_cap = cap;
                // Return a non-null dummy; put_linear_at must not deref it.
                1 as *mut u8
            },
            |_dest, i| seen_idx.push(i),
        )
    };
    assert_eq!(dest, 1 as *mut u8);
    assert_eq!(seen_cap, 32);
    assert_eq!(seen_idx, (0..10).collect::<Vec<_>>());
}

#[test]
fn finish_linear_skips_and_promotes() {
    // skip short-circuits without reading n.
    unsafe {
        assert!(finish_linear_container(
            std::ptr::null_mut(),
            false,
            0,
            false,
            |_, _| panic!("compact"),
            |_| panic!("promote"),
        )
        .is_null());
        let mut words = [3i64, 1, 2, 3];
        let p = words.as_mut_ptr() as *mut u8;
        assert_eq!(
            finish_linear_container(p, true, 0, false, |_, _| panic!("c"), |_| panic!("p")),
            p
        );
        let mut compacted = false;
        let out = finish_linear_container(
            p,
            false,
            10,
            false,
            |_, _| compacted = true,
            |_| panic!("promote"),
        );
        assert!(compacted);
        assert_eq!(out, p);
        let mut promoted = false;
        let dest = 0xdead as *mut u8;
        let out = finish_linear_container(
            p,
            false,
            2,
            false,
            |_, _| {},
            |_| {
                promoted = true;
                dest
            },
        );
        assert!(promoted);
        assert_eq!(out, dest);
    }
}

#[test]
fn compact_linear_last_wins_and_keep_first() {
    unsafe {
        // Map-style: [n][k][v]…
        let mut map = [4i64, 1, 10, 2, 20, 1, 11, 3, 30];
        compact_linear_entries(map.as_mut_ptr() as *mut u8, false, 2, true);
        assert_eq!(&map[..7], &[3, 1, 11, 2, 20, 3, 30]);
        // Set-style: [n][e]…
        let mut set = [5i64, 1, 2, 1, 3, 2];
        compact_linear_entries(set.as_mut_ptr() as *mut u8, false, 1, false);
        assert_eq!(&set[..4], &[3, 1, 2, 3]);
    }
}
