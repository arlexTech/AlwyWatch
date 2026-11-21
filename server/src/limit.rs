// SPDX-License-Identifier: AGPL-3.0-or-later
//! Attempt limits: so a wrong token, a room code or a transfer start can't be tried endlessly.
//! Counted per key (client address, user) in a fixed window, in memory.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

pub struct Limiter {
    max: u32,
    window: Duration,
    hits: Mutex<HashMap<String, (u32, Instant)>>,
}

/// Keys tracked at most (oldest windows are dropped first when full).
const MAX_KEYS: usize = 10_000;

impl Limiter {
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            max,
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u32, Instant)>> {
        self.hits.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether `key` already used up its attempts in the current window.
    pub fn blocked(&self, key: &str) -> bool {
        self.lock()
            .get(key)
            .is_some_and(|(n, at)| at.elapsed() < self.window && *n >= self.max)
    }

    /// Counts one attempt for `key`.
    pub fn hit(&self, key: &str) {
        let mut m = self.lock();
        if m.len() >= MAX_KEYS {
            let window = self.window;
            m.retain(|_, (_, at)| at.elapsed() < window);
        }
        let e = m.entry(key.to_owned()).or_insert((0, Instant::now()));
        if e.1.elapsed() >= self.window {
            *e = (0, Instant::now());
        }
        e.0 = e.0.saturating_add(1);
    }

    /// Counts one attempt and says whether it is still allowed.
    pub fn allow(&self, key: &str) -> bool {
        if self.blocked(key) {
            return false;
        }
        self.hit(key);
        true
    }
}

/// The client's address as Caddy reports it. Caddy replaces any `X-Forwarded-For` a client sends
/// (no trusted proxies before it), so its last entry is the real peer; the server publishes no
/// port, Caddy is the only way in.
pub fn client(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "direct".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_the_limit_until_the_window_ends() {
        let l = Limiter::new(2, Duration::from_millis(50));
        assert!(l.allow("a"));
        assert!(l.allow("a"));
        assert!(!l.allow("a"));
        assert!(l.allow("b"), "per key");
        std::thread::sleep(Duration::from_millis(60));
        assert!(l.allow("a"), "new window");
    }

    #[test]
    fn client_is_the_last_forwarded_address() {
        let mut h = HeaderMap::new();
        assert_eq!(client(&h), "direct");
        h.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        assert_eq!(client(&h), "5.6.7.8");
    }
}
