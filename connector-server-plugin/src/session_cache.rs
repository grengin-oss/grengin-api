// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::Hash,
    ops::{Deref, DerefMut},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionCachePolicy {
    pub max_entries: usize,
    pub idle_ttl_seconds: u64,
    pub max_in_flight: usize,
    pub wait_timeout_seconds: u64,
}

impl Default for SessionCachePolicy {
    fn default() -> Self {
        Self {
            max_entries: 256,
            idle_ttl_seconds: 300,
            max_in_flight: 64,
            wait_timeout_seconds: 30,
        }
    }
}

impl SessionCachePolicy {
    pub fn validate(self) -> Result<Self, SessionCacheError> {
        if self.max_entries > 16384
            || !(1..=86400).contains(&self.idle_ttl_seconds)
            || !(1..=1024).contains(&self.max_in_flight)
            || !(1..=120).contains(&self.wait_timeout_seconds)
        {
            return Err(SessionCacheError::InvalidPolicy);
        }
        Ok(self)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SessionCacheError {
    #[error("invalid MCP session cache policy")]
    InvalidPolicy,
    #[error("MCP sessions are busy; retry later")]
    Busy,
}

struct Slot<V> {
    value: Arc<Mutex<Option<V>>>,
    touched: AtomicU64,
}

struct Entries<K, V> {
    cached: HashMap<K, Arc<Slot<V>>>,
    live: HashMap<K, Weak<Slot<V>>>,
    swept: u64,
}

pub struct SessionCache<K, V> {
    policy: SessionCachePolicy,
    origin: Instant,
    entries: Mutex<Entries<K, V>>,
    permits: Arc<Semaphore>,
}

pub struct SessionLease<V> {
    value: OwnedMutexGuard<Option<V>>,
    slot: Arc<Slot<V>>,
    origin: Instant,
    _permit: OwnedSemaphorePermit,
}

fn tick(origin: Instant) -> u64 {
    origin.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

impl<V> Deref for SessionLease<V> {
    type Target = Option<V>;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<V> DerefMut for SessionLease<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

impl<V> Drop for SessionLease<V> {
    fn drop(&mut self) {
        self.slot
            .touched
            .store(tick(self.origin), Ordering::Relaxed);
    }
}

impl<K: Clone + Eq + Hash, V> SessionCache<K, V> {
    pub fn new(policy: SessionCachePolicy) -> Result<Self, SessionCacheError> {
        let policy = policy.validate()?;
        Ok(Self {
            policy,
            origin: Instant::now(),
            entries: Mutex::new(Entries {
                cached: HashMap::new(),
                live: HashMap::new(),
                swept: 0,
            }),
            permits: Arc::new(Semaphore::new(policy.max_in_flight)),
        })
    }

    async fn slot(&self, key: K) -> Arc<Slot<V>> {
        let now = tick(self.origin);
        let mut entries = self.entries.lock().await;
        let ttl = Duration::from_secs(self.policy.idle_ttl_seconds).as_nanos() as u64;
        let sweep_interval = ttl.min(Duration::from_secs(10).as_nanos() as u64);
        if now.saturating_sub(entries.swept) >= sweep_interval {
            entries.cached.retain(|_, slot| {
                Arc::strong_count(slot) > 1
                    || now.saturating_sub(slot.touched.load(Ordering::Relaxed)) < ttl
            });
            entries.live.retain(|_, slot| slot.strong_count() > 0);
            entries.swept = now;
        }
        let slot = entries
            .cached
            .get(&key)
            .cloned()
            .or_else(|| entries.live.get(&key).and_then(Weak::upgrade))
            .unwrap_or_else(|| {
                Arc::new(Slot {
                    value: Arc::new(Mutex::new(None)),
                    touched: AtomicU64::new(now),
                })
            });
        slot.touched.store(now, Ordering::Relaxed);
        entries.live.insert(key.clone(), Arc::downgrade(&slot));
        if self.policy.max_entries > 0 && !entries.cached.contains_key(&key) {
            if entries.cached.len() >= self.policy.max_entries {
                let oldest = entries
                    .cached
                    .iter()
                    .min_by_key(|(_, value)| value.touched.load(Ordering::Relaxed))
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    if entries
                        .cached
                        .get(&oldest)
                        .is_some_and(|value| Arc::strong_count(value) == 1)
                    {
                        entries.live.remove(&oldest);
                    }
                    // Leases and weak lookup keep an evicted active session alive and single-flight.
                    entries.cached.remove(&oldest);
                }
            }
            entries.cached.insert(key, slot.clone());
        }
        slot
    }

    pub async fn lease(&self, key: K) -> Result<SessionLease<V>, SessionCacheError> {
        let slot = self.slot(key).await;
        tokio::time::timeout(
            Duration::from_secs(self.policy.wait_timeout_seconds),
            async {
                let value = slot.value.clone().lock_owned().await;
                let permit = self
                    .permits
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| SessionCacheError::Busy)?;
                Ok(SessionLease {
                    value,
                    slot,
                    origin: self.origin,
                    _permit: permit,
                })
            },
        )
        .await
        .map_err(|_| SessionCacheError::Busy)?
    }

    pub async fn invalidate(&self, key: &K) {
        let mut entries = self.entries.lock().await;
        entries.cached.remove(key);
        entries.live.remove(key);
    }

    pub async fn cached_len(&self) -> usize {
        self.entries.lock().await.cached.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn more_than_256_users_do_not_exhaust_a_small_cache() {
        let cache = SessionCache::<usize, usize>::new(SessionCachePolicy {
            max_entries: 4,
            ..Default::default()
        })
        .unwrap();
        for user in 0..1024 {
            let mut lease = cache.lease(user).await.unwrap();
            assert!(lease.is_none());
            *lease = Some(user);
        }
        assert_eq!(cache.cached_len().await, 4);
    }

    #[tokio::test]
    async fn active_eviction_preserves_identity_and_does_not_duplicate_slots() {
        let cache = Arc::new(
            SessionCache::<usize, usize>::new(SessionCachePolicy {
                max_entries: 1,
                ..Default::default()
            })
            .unwrap(),
        );
        let mut first = cache.lease(1).await.unwrap();
        *first = Some(42);
        drop(cache.lease(2).await.unwrap());
        let waiting = cache.clone();
        let waiter = tokio::spawn(async move { waiting.lease(1).await.unwrap() });
        tokio::time::timeout(Duration::from_secs(1), async {
            while Arc::strong_count(&first.slot) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!waiter.is_finished());
        drop(first);
        assert_eq!(*waiter.await.unwrap(), Some(42));
    }

    #[tokio::test]
    async fn cancellation_releases_capacity_and_other_users_are_independent() {
        let cache = Arc::new(
            SessionCache::<usize, usize>::new(SessionCachePolicy {
                max_in_flight: 1,
                ..Default::default()
            })
            .unwrap(),
        );
        let first = cache.lease(1).await.unwrap();
        let next = cache.clone();
        let waiter = tokio::spawn(async move { next.lease(2).await });
        tokio::task::yield_now().await;
        waiter.abort();
        let _ = waiter.await;
        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), cache.lease(3))
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn invalidation_and_disabled_idle_cache_do_not_reuse_values() {
        let cache = SessionCache::<usize, usize>::new(SessionCachePolicy::default()).unwrap();
        *cache.lease(1).await.unwrap() = Some(7);
        cache.invalidate(&1).await;
        assert!(cache.lease(1).await.unwrap().is_none());
        let cache = SessionCache::<usize, usize>::new(SessionCachePolicy {
            max_entries: 0,
            ..Default::default()
        })
        .unwrap();
        *cache.lease(1).await.unwrap() = Some(7);
        assert!(cache.lease(1).await.unwrap().is_none());
        assert_eq!(cache.cached_len().await, 0);
    }

    #[tokio::test]
    async fn idle_ttl_reclaims_values_without_expiring_an_active_lease() {
        let mut cache = SessionCache::<usize, usize>::new(SessionCachePolicy {
            idle_ttl_seconds: 1,
            ..Default::default()
        })
        .unwrap();
        cache.origin = Instant::now() - Duration::from_secs(2);
        *cache.lease(1).await.unwrap() = Some(7);
        {
            let mut entries = cache.entries.lock().await;
            entries
                .cached
                .get(&1)
                .unwrap()
                .touched
                .store(0, Ordering::Relaxed);
            entries.swept = 0;
        }
        drop(cache.lease(2).await.unwrap());
        assert!(cache.lease(1).await.unwrap().is_none());
        let mut active = cache.lease(1).await.unwrap();
        *active = Some(42);
        active.slot.touched.store(0, Ordering::Relaxed);
        cache.entries.lock().await.swept = 0;
        drop(cache.lease(3).await.unwrap());
        assert_eq!(*active, Some(42));
        drop(active);
        assert_eq!(*cache.lease(1).await.unwrap(), Some(42));
    }

    #[tokio::test]
    async fn admission_timeout_is_retryable_and_does_not_poison_the_cache() {
        let cache = SessionCache::<usize, usize>::new(SessionCachePolicy {
            max_in_flight: 1,
            wait_timeout_seconds: 1,
            ..Default::default()
        })
        .unwrap();
        let first = cache.lease(1).await.unwrap();
        assert!(matches!(cache.lease(2).await, Err(SessionCacheError::Busy)));
        drop(first);
        assert!(cache.lease(2).await.is_ok());
    }

    #[test]
    fn policy_is_typed_and_bounded() {
        for policy in [
            SessionCachePolicy {
                max_entries: 16385,
                ..Default::default()
            },
            SessionCachePolicy {
                max_in_flight: 0,
                ..Default::default()
            },
            SessionCachePolicy {
                wait_timeout_seconds: 0,
                ..Default::default()
            },
            SessionCachePolicy {
                idle_ttl_seconds: 0,
                ..Default::default()
            },
        ] {
            assert_eq!(
                policy.validate().unwrap_err(),
                SessionCacheError::InvalidPolicy
            );
        }
        assert!(
            serde_json::from_str::<SessionCachePolicy>(r#"{"max_entries":"unlimited"}"#).is_err()
        );
        assert!(serde_json::from_str::<SessionCachePolicy>(r#"{"typo":1}"#).is_err());
    }
}
