//! The visitor path serves what it can and says once per process what it could not.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// Warnings carry file paths, so a world of stale squares would grow the set with every
/// click; past this many the set starts over and a warning may print again.
const REMEMBERED_WARNINGS_CAP: usize = 1024;

/// Print `warning` to stderr unless this process already printed the same text; `first_seen`
/// (the click, say) is printed with it without making the warning a new one.
pub fn warn_once(warning: &str, first_seen: &str) {
    static PRINTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut printed = PRINTED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if is_new_warning(&mut printed, warning) {
        let separator = if first_seen.is_empty() { "" } else { " " };
        eprintln!("quietmap-served-with-fault: {warning}{separator}{first_seen}");
    }
}

fn is_new_warning(printed: &mut HashSet<String>, warning: &str) -> bool {
    if printed.contains(warning) {
        return false;
    }
    if printed.len() >= REMEMBERED_WARNINGS_CAP {
        printed.clear();
    }
    printed.insert(warning.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_distinct_fault_logs_and_a_repeated_one_does_not() {
        let mut printed = HashSet::new();
        assert!(is_new_warning(&mut printed, "leisure.arrow is stale"));
        assert!(!is_new_warning(&mut printed, "leisure.arrow is stale"));
        assert!(is_new_warning(&mut printed, "ships.arrow is stale"));
        for square in 0..REMEMBERED_WARNINGS_CAP {
            is_new_warning(&mut printed, &format!("square {square}"));
        }
        assert!(printed.len() <= REMEMBERED_WARNINGS_CAP);
    }
}
