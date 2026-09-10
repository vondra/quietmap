//! Charge actual ground identity growth and its writer working set before allocation.
use anyhow::{Context, Result};
use std::mem::size_of;

pub(crate) struct AllocationBudget {
    limit: u64,
    reserved: u64,
}

impl AllocationBudget {
    pub(crate) fn new(limit: u64, base: u64) -> Result<Self> {
        let mut budget = Self { limit, reserved: 0 };
        budget.reserve(base)?;
        Ok(budget)
    }

    pub(crate) fn reserve(&mut self, bytes: u64) -> Result<()> {
        let total = self
            .reserved
            .checked_add(bytes)
            .context("ground allocation overflow")?;
        anyhow::ensure!(
            total <= self.limit,
            "ground allocation admission: {total} B reserved, {} B limit",
            self.limit
        );
        self.reserved = total;
        Ok(())
    }

    pub(crate) fn reserve_hash_entry<K, V>(&mut self, len: usize) -> Result<()> {
        let entry = size_of::<(K, V)>();
        self.reserve(
            hash_allowance(
                len.checked_add(1).context("ground map length overflow")?,
                entry,
            )? - hash_allowance(len, entry)?,
        )
    }

    pub(crate) fn reserve_vec_entry<T>(&mut self, len: usize) -> Result<()> {
        let allocation = |n: usize| -> Result<u64> {
            if n == 0 {
                return Ok(0);
            }
            let capacity = n
                .max(4)
                .checked_next_power_of_two()
                .context("ground vector capacity overflow")?;
            u64::try_from(6 * capacity as u128 * size_of::<T>() as u128)
                .context("ground vector allowance overflow")
        };
        self.reserve(
            allocation(
                len.checked_add(1)
                    .context("ground vector length overflow")?,
            )? - allocation(len)?,
        )
    }

    pub(crate) fn reserved(&self) -> u64 {
        self.reserved
    }
}

fn hash_allowance(len: usize, entry_bytes: usize) -> Result<u64> {
    if len == 0 {
        return Ok(0);
    }
    // The current std HashMap uses power-of-two hashbrown buckets at 7/8 load.
    // Charge both tables during growth, control bytes, alignment, and a separate
    // allocator allowance; this is not a claim about measured process RSS.
    let buckets = (len as u128 * 8)
        .div_ceil(7)
        .max(4)
        .checked_next_power_of_two()
        .context("ground hash capacity overflow")?;
    u64::try_from(2 * (2 * buckets * (entry_bytes as u128 + 1) + 64))
        .context("ground hash allowance overflow")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn hash_growth_is_charged_before_the_allocator_changes_capacity() {
        let limit = hash_allowance(1024, size_of::<(u64, u16)>()).unwrap() - 1;
        let mut budget = AllocationBudget::new(limit, 0).unwrap();
        let mut map = HashMap::new();
        let mut refused = false;
        for id in 0..1024u64 {
            let before = (map.len(), map.capacity(), budget.reserved());
            if budget.reserve_hash_entry::<u64, u16>(map.len()).is_err() {
                assert_eq!((map.len(), map.capacity(), budget.reserved()), before);
                assert!(!map.contains_key(&id));
                refused = true;
                break;
            }
            map.insert(id, 1u16);
            assert!(
                budget.reserved() >= ((map.capacity() + 1) * (size_of::<(u64, u16)>() + 1)) as u64
            );
        }
        assert!(refused);
        assert!(map.len() > 1);
    }
}
