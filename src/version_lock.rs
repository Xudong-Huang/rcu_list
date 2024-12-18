use core::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TryLockErr {
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
    fn version_generation(&self, version: usize) -> usize {
        // first bit means node is removed
        // second bit means node is locked
        // valid generations are 0, 4, 8, 12...
        if version & 2 == 0 {
            // not locked, use current generation
            version
        } else {
            // locked, use next generation to try
            version + 2
        }
    }

    /// try lock and return current version
    /// if the node is removed, return Err(NodeTryLockErr::Removed)
    #[inline]
    pub fn try_lock(&self) -> Result<usize, TryLockErr> {
        let version = self.version.load(Ordering::Relaxed);
        if version & 1 == 1 {
            return Err(TryLockErr::Removed);
        }

        let version = self.version_generation(version);
        match self.version.compare_exchange(
            version,
            version + 2,
            Ordering::Release,
            Ordering::Relaxed,
        ) {
            Ok(v) => {
                core::sync::atomic::fence(Ordering::Acquire);
                Ok(v)
            }

            Err(v) => {
                if v & 1 == 1 {
                    Err(TryLockErr::Removed)
                } else {
                    Err(TryLockErr::Retry)
                }
            }
        }
    }

    /// lock and return current version
    /// if the node is removed, return Err(NodeTryLockErr::Removed)
    /// valid version is returned in 0, 4, 8, 12...
    #[inline]
    pub fn lock(&self) -> Result<usize, TryLockErr> {
        let backoff = crossbeam_utils::Backoff::new();

        let version = self.version.load(Ordering::Relaxed);
        if version & 1 == 1 {
            return Err(TryLockErr::Removed);
        }

        let mut version = self.version_generation(version);
        while let Err(v) = self.version.compare_exchange_weak(
            version,
            version + 2,
            Ordering::Release,
            Ordering::Relaxed,
        ) {
            if v & 1 == 1 {
                return Err(TryLockErr::Removed);
            }
            version = self.version_generation(v);
            backoff.snooze();
        }

        core::sync::atomic::fence(Ordering::Acquire);
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
        assert_eq!(lock.try_lock(), Err(super::TryLockErr::Retry));
        lock.unlock();
        assert_eq!(lock.try_lock(), Ok(4));
        lock.unlock_remove();
        assert_eq!(lock.try_lock(), Err(super::TryLockErr::Removed));
        assert!(lock.is_removed());
        // assert!(!lock.is_ready());
    }
}
