//! One flight identity carries all independent ground-operation memberships, each held once per provider provenance.
//!
//! A category's primary bit sits at `1 << k`, its secondary bit at
//! `1 << (k + SECONDARY_SHIFT)`. A movement counts as primary in a category
//! when any primary row put it there, as secondary-only when only secondary
//! rows did: the difference estimator's `N_union − N_primary`.
use super::admission::AllocationBudget;
use anyhow::Result;
use std::collections::HashMap;

/// Any movement on a microsegment (aircraft or ground vehicle).
pub(crate) const ANY: u32 = 1 << 0;
pub(crate) const ARRIVAL: u32 = 1 << 1;
pub(crate) const DEPARTURE: u32 = 1 << 2;
pub(crate) const GSE: [u32; crate::arrow_schemas::NUM_GSE_CLASSES as usize] =
    [1 << 3, 1 << 4, 1 << 5];
pub(crate) const OPS: [u32; crate::arrow_schemas::NUM_OPS_KINDS as usize] =
    [1 << 6, 1 << 7, 1 << 8];
const CATEGORIES: u32 = 9;
pub(crate) const SECONDARY_SHIFT: u32 = CATEGORIES;
const CATEGORY_MASK: u32 = (1 << CATEGORIES) - 1;
/// Microsegment unions: movement, direction and vehicle class.
pub(crate) const MICRO_CATEGORIES: u32 = ANY | ARRIVAL | DEPARTURE | GSE[0] | GSE[1] | GSE[2];
/// Airport unions: direction, vehicle class and operation kind.
pub(crate) const AIRPORT_CATEGORIES: u32 = CATEGORY_MASK & !ANY;

/// Both provenances of a category mask.
pub(crate) const fn with_secondary(categories: u32) -> u32 {
    categories | (categories << SECONDARY_SHIFT)
}

#[derive(Default)]
pub(crate) struct MovementUnion {
    pub(crate) members: HashMap<u64, u32>,
}

/// Distinct movements per category: with a primary row, and only with secondary rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MovementCounts {
    primary: [u32; CATEGORIES as usize],
    secondary_only: [u32; CATEGORIES as usize],
}

impl MovementCounts {
    pub(crate) fn primary(&self, category: u32) -> u32 {
        debug_assert!(category.is_power_of_two() && category <= CATEGORY_MASK);
        self.primary[category.trailing_zeros() as usize]
    }
    pub(crate) fn secondary_only(&self, category: u32) -> u32 {
        debug_assert!(category.is_power_of_two() && category <= CATEGORY_MASK);
        self.secondary_only[category.trailing_zeros() as usize]
    }
}

impl MovementUnion {
    pub(crate) fn insert(
        &mut self,
        fid: u64,
        flags: u32,
        budget: &mut AllocationBudget,
    ) -> Result<()> {
        if let Some(value) = self.members.get_mut(&fid) {
            *value |= flags;
            return Ok(());
        }
        anyhow::ensure!(
            self.members.len() < u32::MAX as usize,
            "ground movement count exceeds UInt32"
        );
        budget.reserve_hash_entry::<u64, u32>(self.members.len())?;
        // Sorted part rows, Arrow builders and their transient growth coexist.
        budget.reserve(4 * std::mem::size_of::<(u64, u32)>() as u64)?;
        self.members.insert(fid, flags);
        Ok(())
    }

    pub(crate) fn counts(&self) -> MovementCounts {
        let mut counts = MovementCounts::default();
        for &flags in self.members.values() {
            let primary = flags & CATEGORY_MASK;
            let secondary_only = (flags >> SECONDARY_SHIFT) & CATEGORY_MASK & !primary;
            for bit in 0..CATEGORIES {
                counts.primary[bit as usize] += (primary >> bit) & 1;
                counts.secondary_only[bit as usize] += (secondary_only >> bit) & 1;
            }
        }
        counts
    }
}

/// The category memberships of one ground hit, at the row's provenance.
pub(crate) fn hit_flags(
    secondary_only: bool,
    veh_kind: u8,
    class_idx: u8,
    ops_kind: u8,
    is_dep: bool,
) -> u32 {
    use noise_compute::emission::aircraft::{
        GROUND_OPS_KIND_APRON_MOVEMENT, GROUND_OPS_KIND_RUNWAY_ROLL, GROUND_OPS_KIND_TAXI,
    };
    let mut categories = ANY;
    if veh_kind == 1 {
        categories |= GSE[usize::from(class_idx)];
    } else {
        if ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL {
            categories |= if is_dep { DEPARTURE } else { ARRIVAL };
        }
        categories |= match ops_kind {
            GROUND_OPS_KIND_RUNWAY_ROLL => OPS[0],
            GROUND_OPS_KIND_TAXI => OPS[1],
            GROUND_OPS_KIND_APRON_MOVEMENT => OPS[2],
            _ => 0,
        };
    }
    if secondary_only {
        categories << SECONDARY_SHIFT
    } else {
        categories
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flight seen on the same microsegment by both providers is one
    /// primary movement; only a flight without any primary row in the
    /// category is secondary-only.
    #[test]
    fn secondary_rows_count_only_movements_the_primary_provider_missed() {
        use noise_compute::emission::aircraft::*;
        let mut budget = AllocationBudget::new(1 << 20, 0).unwrap();
        let mut union = MovementUnion::default();
        let arrival = |secondary| hit_flags(secondary, 0, 0, GROUND_OPS_KIND_RUNWAY_ROLL, false);
        let taxi = |secondary| hit_flags(secondary, 0, 0, GROUND_OPS_KIND_TAXI, false);
        // 1: both providers see the arrival; 2: only the secondary does;
        // 3: primary taxi, secondary-only arrival.
        for (fid, flags) in [
            (1, arrival(false)),
            (1, arrival(true)),
            (2, arrival(true)),
            (3, taxi(false)),
            (3, arrival(true)),
            (4, hit_flags(true, 1, 2, GROUND_OPS_KIND_TAXI, false)),
        ] {
            union.insert(fid, flags, &mut budget).unwrap();
            union.insert(fid, flags, &mut budget).unwrap();
        }
        let counts = union.counts();
        assert_eq!((counts.primary(ARRIVAL), counts.secondary_only(ARRIVAL)), (1, 2));
        assert_eq!((counts.primary(ANY), counts.secondary_only(ANY)), (2, 2));
        assert_eq!((counts.primary(OPS[1]), counts.secondary_only(OPS[1])), (1, 0));
        assert_eq!((counts.primary(GSE[2]), counts.secondary_only(GSE[2])), (0, 1));
    }

    #[test]
    fn refused_membership_growth_leaves_the_union_unchanged() {
        let mut budget = AllocationBudget::new(0, 0).unwrap();
        let mut union = MovementUnion::default();
        assert!(union.insert(42, ARRIVAL, &mut budget).is_err());
        assert!(union.members.is_empty());
        assert_eq!(union.counts().primary(ARRIVAL), 0);
    }
}
