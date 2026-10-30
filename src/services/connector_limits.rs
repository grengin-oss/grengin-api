// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Window {
    started: Instant,
    touched: Instant,
    count: u32,
}

#[derive(Default)]
pub struct ConnectorStartLimiter {
    windows: Mutex<HashMap<Uuid, Window>>,
}

impl ConnectorStartLimiter {
    pub fn admit(&self, user: Uuid, now: Instant) -> Result<(), u64> {
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        windows.retain(|_, window| {
            now.saturating_duration_since(window.started) < Duration::from_secs(60)
        });
        if windows.len() >= 4096
            && !windows.contains_key(&user)
            && let Some(oldest) = windows
                .iter()
                .min_by_key(|(_, window)| window.touched)
                .map(|(key, _)| *key)
        {
            windows.remove(&oldest);
        }
        let window = windows.entry(user).or_insert(Window {
            started: now,
            touched: now,
            count: 0,
        });
        window.touched = now;
        if window.count >= 20 {
            return Err(60u64
                .saturating_sub(now.saturating_duration_since(window.started).as_secs())
                .max(1));
        }
        window.count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn starts_are_isolated_bounded_and_reset_after_the_fixed_window() {
        let limiter = ConnectorStartLimiter::default();
        let now = Instant::now();
        let user = Uuid::new_v4();
        for _ in 0..20 {
            assert!(limiter.admit(user, now).is_ok());
        }
        assert_eq!(limiter.admit(user, now + Duration::from_secs(5)), Err(55));
        assert!(limiter.admit(Uuid::new_v4(), now).is_ok());
        assert!(limiter.admit(user, now + Duration::from_secs(60)).is_ok());
    }
    #[test]
    fn a_full_cache_does_not_lock_out_every_new_authenticated_user() {
        let limiter = ConnectorStartLimiter::default();
        let now = Instant::now();
        for _ in 0..4100 {
            assert!(limiter.admit(Uuid::new_v4(), now).is_ok());
        }
        assert!(limiter.windows.lock().unwrap().len() <= 4096);
    }
}
