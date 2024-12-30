use core::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum LockErr {
    Removed,
    Retry,
}

#[derive(Debug, Default)]
pub struct VersionLock {
    version: AtomicUsize,
}

impl VersionLock {
    /// Create a new VersionLock
    pub const fn new() -> Self {
        Self {
            version: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn next_version(&self, version: usize) -> usize {
        // first bit means node is removed
        // second bit means node is locked
        // valid generations are 0, 4, 8, 12...

        // if version & 2 == 0 {
        //     // not locked, use current generation
        //     version
        // } else {
        //     // locked, use next generation to try
        //     version + 2
        // }
        version + (version & 2)
    }

    /// try lock and return current version
    /// if the node is removed, return Err(LockErr::Removed)
    #[inline]
    pub fn try_lock(&self) -> Result<usize, LockErr> {
        let version = self.version.load(Ordering::Relaxed);
        if version & 1 == 1 {
            return Err(LockErr::Removed);
        }

        let mut version = self.next_version(version);
        while let Err(v) = self.version.compare_exchange_weak(
            version,
            version + 2,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            if v & 1 == 1 {
                return Err(LockErr::Removed);
            }
            if v & 2 == 2 {
                // already locked
                return Err(LockErr::Retry);
            }
            version = v;
        }

        Ok(version)
    }

    /// lock and return current version
    /// if the node is removed, return Err(LockErr::Removed)
    /// valid version is returned in 0, 4, 8, 12...
    #[inline]
    pub fn lock(&self) -> Result<usize, LockErr> {
        let backoff = crossbeam_utils::Backoff::new();

        let version = self.version.load(Ordering::Relaxed);
        if version & 1 == 1 {
            return Err(LockErr::Removed);
        }

        let mut version = self.next_version(version);
        while let Err(v) = self.version.compare_exchange_weak(
            version,
            version + 2,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            if v & 1 == 1 {
                return Err(LockErr::Removed);
            }
            version = self.next_version(v);
            backoff.snooze();
        }

        Ok(version)
    }

    #[inline]
    /// unlock to allow other threads to lock
    pub fn unlock(&self) {
        let version = self.version.load(Ordering::Relaxed);
        self.version.store(version + 2, Ordering::Release);
    }

    #[inline]
    /// unlock as removed
    pub fn unlock_remove(&self) {
        let version = self.version.load(Ordering::Relaxed);
        self.version.store(version + 3, Ordering::Release);
    }

    #[inline]
    /// Check if the lock is mark as removed
    pub fn is_removed(&self) -> bool {
        self.version.load(Ordering::Relaxed) & 1 == 1
    }

    // #[inline]
    // /// Check if the lock is unlocked and not removed
    // pub fn is_ready(&self) -> bool {
    //     self.version.load(Ordering::Relaxed) & 3 == 0
    // }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_lock_test() {
        let lock = super::VersionLock::new();
        assert_eq!(lock.try_lock(), Ok(0));
        assert_eq!(lock.try_lock(), Err(super::LockErr::Retry));
        lock.unlock();
        assert_eq!(lock.try_lock(), Ok(4));
        lock.unlock_remove();
        assert_eq!(lock.try_lock(), Err(super::LockErr::Removed));
        assert!(lock.is_removed());
        // assert!(!lock.is_ready());
    }

    #[test]
    fn next_version() {
        let lock = super::VersionLock::new();
        assert_eq!(lock.next_version(0), 0);
        assert_eq!(lock.next_version(1), 1);
        assert_eq!(lock.next_version(2), 4);
        assert_eq!(lock.next_version(3), 5);
        assert_eq!(lock.next_version(4), 4);
        assert_eq!(lock.next_version(5), 5);
        assert_eq!(lock.next_version(6), 8);
        assert_eq!(lock.next_version(7), 9);
        assert_eq!(lock.next_version(8), 8);
        assert_eq!(lock.next_version(9), 9);
        assert_eq!(lock.next_version(10), 12);
        assert_eq!(lock.next_version(11), 13);
        assert_eq!(lock.next_version(12), 12);
    }
}
