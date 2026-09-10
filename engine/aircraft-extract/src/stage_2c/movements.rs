//! One flight identity carries all independent ground-operation memberships.
use super::admission::AllocationBudget;
use anyhow::Result;
use std::collections::HashMap;

pub(crate) const NON_GA: u16 = 1 << 0;
pub(crate) const ARRIVAL: u16 = 1 << 1;
pub(crate) const DEPARTURE: u16 = 1 << 2;
pub(crate) const GSE: [u16; crate::arrow_schemas::NUM_GSE_CLASSES as usize] =
    [1 << 3, 1 << 4, 1 << 5];
pub(crate) const GA: u16 = 1 << 6;
pub(crate) const GA_ARRIVAL: u16 = 1 << 7;
pub(crate) const GA_DEPARTURE: u16 = 1 << 8;
pub(crate) const OPS: [u16; crate::arrow_schemas::NUM_OPS_KINDS as usize] =
    [1 << 9, 1 << 10, 1 << 11];
pub(crate) const GA_OPS: [u16; crate::arrow_schemas::NUM_OPS_KINDS as usize] =
    [1 << 12, 1 << 13, 1 << 14];
pub(crate) const MICRO_FLAGS: u16 = (1 << 9) - 1;
pub(crate) const AIRPORT_FLAGS: u16 = ((1 << 15) - 1) & !(NON_GA | GA);

#[derive(Default)]
pub(crate) struct MovementUnion {
    pub(crate) members: HashMap<u64, u16>,
    counts: [u32; 15],
}

impl MovementUnion {
    pub(crate) fn insert(
        &mut self,
        fid: u64,
        flags: u16,
        budget: &mut AllocationBudget,
    ) -> Result<()> {
        if let Some(value) = self.members.get_mut(&fid) {
            let added = flags & !*value;
            *value |= flags;
            self.add_counts(added);
            return Ok(());
        }
        anyhow::ensure!(
            self.members.len() < u32::MAX as usize,
            "ground movement count exceeds UInt32"
        );
        budget.reserve_hash_entry::<u64, u16>(self.members.len())?;
        // Sorted part rows, Arrow builders and their transient growth coexist.
        budget.reserve(4 * std::mem::size_of::<(u64, u16)>() as u64)?;
        self.members.insert(fid, flags);
        self.add_counts(flags);
        Ok(())
    }

    fn add_counts(&mut self, mut flags: u16) {
        while flags != 0 {
            self.counts[flags.trailing_zeros() as usize] += 1;
            flags &= flags - 1;
        }
    }

    pub(crate) fn count(&self, flag: u16) -> u32 {
        debug_assert!(flag.is_power_of_two());
        self.counts[flag.trailing_zeros() as usize]
    }
}

pub(crate) fn hit_flags(
    is_ga: bool,
    veh_kind: u8,
    class_idx: u8,
    ops_kind: u8,
    is_dep: bool,
) -> u16 {
    use noise_compute::emission::aircraft::{
        GROUND_OPS_KIND_APRON_MOVEMENT, GROUND_OPS_KIND_RUNWAY_ROLL, GROUND_OPS_KIND_TAXI,
    };
    let mut flags = if is_ga { GA } else { NON_GA };
    if veh_kind == 1 {
        return flags | GSE[usize::from(class_idx)];
    }
    if ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL {
        flags |= match (is_ga, is_dep) {
            (false, false) => ARRIVAL,
            (false, true) => DEPARTURE,
            (true, false) => GA_ARRIVAL,
            (true, true) => GA_DEPARTURE,
        };
    }
    let kind = match ops_kind {
        GROUND_OPS_KIND_RUNWAY_ROLL => 0,
        GROUND_OPS_KIND_TAXI => 1,
        GROUND_OPS_KIND_APRON_MOVEMENT => 2,
        _ => return flags,
    };
    flags | if is_ga { GA_OPS[kind] } else { OPS[kind] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_flight_keeps_every_independent_class_direction_and_operation_membership() {
        use noise_compute::emission::aircraft::*;
        let mut budget = AllocationBudget::new(1 << 20, 0).unwrap();
        let mut union = MovementUnion::default();
        for (ga, vehicle, class, ops, dep) in [
            (false, 0, 0, GROUND_OPS_KIND_RUNWAY_ROLL, false),
            (false, 0, 0, GROUND_OPS_KIND_RUNWAY_ROLL, true),
            (false, 0, 0, GROUND_OPS_KIND_TAXI, false),
            (false, 0, 0, GROUND_OPS_KIND_APRON_MOVEMENT, false),
            (true, 0, 0, GROUND_OPS_KIND_RUNWAY_ROLL, false),
            (true, 0, 0, GROUND_OPS_KIND_RUNWAY_ROLL, true),
            (true, 0, 0, GROUND_OPS_KIND_TAXI, false),
            (true, 0, 0, GROUND_OPS_KIND_APRON_MOVEMENT, false),
            (false, 1, 0, GROUND_OPS_KIND_TAXI, false),
            (false, 1, 1, GROUND_OPS_KIND_TAXI, false),
            (false, 1, 2, GROUND_OPS_KIND_TAXI, false),
        ] {
            let flags = hit_flags(ga, vehicle, class, ops, dep);
            union.insert(42, flags, &mut budget).unwrap();
            union.insert(42, flags, &mut budget).unwrap();
        }
        assert_eq!(union.members.len(), 1);
        assert_eq!(union.members[&42], (1 << 15) - 1);
        for bit in 0..15 {
            assert_eq!(union.count(1 << bit), 1);
        }
    }

    #[test]
    fn refused_membership_growth_leaves_the_union_unchanged() {
        let mut budget = AllocationBudget::new(0, 0).unwrap();
        let mut union = MovementUnion::default();
        assert!(union.insert(42, ARRIVAL, &mut budget).is_err());
        assert!(union.members.is_empty());
        assert_eq!(union.count(ARRIVAL), 0);
    }
}
