//! Running blocking lookups at the same time.
//!
//! Deliberately threads rather than a work-stealing pool. Everything parallel in gearprice
//! is a network request that Reverb takes between a third of a second and a second and a
//! half to answer, and a pool sized to the number of cores will happily run three of them
//! one after another on the same thread. Threads are cheap next to a wait like that.

use anyhow::{Result, bail};

/// Runs `work` over every item at once, one thread each, and collects the results in
/// order. A panicking worker fails the batch rather than taking the process down.
pub fn each<Item: Sync, Output: Send>(
    items: &[Item],
    work: impl Fn(&Item) -> Result<Output> + Sync,
) -> Result<Vec<Output>> {
    if items.len() <= 1 {
        return items.iter().map(&work).collect();
    }
    std::thread::scope(|scope| {
        let running: Vec<_> = items
            .iter()
            .map(|item| scope.spawn(|| work(item)))
            .collect();
        running
            .into_iter()
            .map(|handle| match handle.join() {
                Ok(result) => result,
                Err(_) => bail!("a lookup thread panicked"),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn the_work_really_happens_at_the_same_time() {
        // The point of the module: eight tenth-of-a-second waits must not take eight
        // tenths of a second. A thread pool sized to the cores would let them.
        let items: Vec<u32> = (0..8).collect();
        let started = Instant::now();
        let results = each(&items, |item| {
            std::thread::sleep(Duration::from_millis(100));
            Ok(*item * 2)
        })
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "took {:?}, which is not concurrent",
            started.elapsed()
        );
        // Results come back in the order the items were given, not the order they finished.
        assert_eq!(vec![0, 2, 4, 6, 8, 10, 12, 14], results);
    }

    #[test]
    fn a_failure_anywhere_fails_the_batch() {
        let items: Vec<u32> = (0..4).collect();
        let attempts = AtomicU32::new(0);
        let result = each(&items, |item| {
            attempts.fetch_add(1, Ordering::Relaxed);
            if *item == 2 { bail!("no") } else { Ok(*item) }
        });
        assert!(result.is_err());
        // Every item was still attempted; the batch is not short-circuited halfway.
        assert_eq!(4, attempts.load(Ordering::Relaxed));
    }

    #[test]
    fn a_single_item_needs_no_thread_at_all() {
        assert_eq!(vec![7], each(&[7], |item| Ok(*item)).unwrap());
        assert!(each::<u32, u32>(&[], |_| Ok(0)).unwrap().is_empty());
    }
}
