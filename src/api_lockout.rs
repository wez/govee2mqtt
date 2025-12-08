//! API Lockout State Management
//!
//! This module provides persistent lockout state tracking for API failures,
//! enabling graceful degradation to LAN-only mode when Govee's cloud APIs
//! are unavailable due to rate limiting, account lockouts, or network issues.
//!
//! ## Important Notes
//!
//! **Initial API Connection Required**: When API credentials are configured,
//! govee2mqtt must successfully connect to the API at least once during startup
//! to retrieve the device list and metadata (friendly names, room assignments,
//! device capabilities). This information is cached and used when operating in
//! LAN-only degraded mode.
//!
//! **LAN-Only Mode Without Credentials**: If no API credentials are provided,
//! govee2mqtt will operate in pure LAN-only mode from the start. Devices will
//! be discovered via the LAN protocol but will have less descriptive entity
//! names (device IDs instead of friendly names) since the metadata from the
//! API is not available.
//!
//! **Automatic Recovery**: When in degraded mode due to a recoverable error,
//! the system will periodically attempt to reconnect to the API. Once the
//! lockout period expires, normal API operations resume automatically.
//!
//! See: https://github.com/wez/govee2mqtt/issues/76

// Note: cache_get, CacheComputeResult, CacheGetOptions are available if needed
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Duration constants for lockout periods
const TWENTY_FOUR_HOURS: Duration = Duration::from_secs(24 * 60 * 60);
const ONE_HOUR: Duration = Duration::from_secs(60 * 60);
const FIVE_MINUTES: Duration = Duration::from_secs(5 * 60);
const FIFTEEN_MINUTES: Duration = Duration::from_secs(15 * 60);

/// Types of API lockouts that can occur
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LockoutType {
    /// "Your account is abnormal" - triggered by rapid login attempts
    AbnormalActivity,
    /// HTTP 429 - Daily rate limit exceeded
    RateLimit,
    /// HTTP 401 - Authentication failure
    Unauthorized,
    /// Network error (DNS, timeout, connection refused)
    NetworkError,
    /// Unknown/other error
    Unknown,
}

impl LockoutType {
    /// Get the recommended lockout duration for this error type
    pub fn lockout_duration(&self) -> Duration {
        match self {
            Self::AbnormalActivity => TWENTY_FOUR_HOURS,
            Self::RateLimit => TWENTY_FOUR_HOURS,
            Self::Unauthorized => TWENTY_FOUR_HOURS,
            Self::NetworkError => FIVE_MINUTES,
            Self::Unknown => FIFTEEN_MINUTES,
        }
    }

    /// Classify an error into a lockout type
    pub fn from_error(err: &anyhow::Error) -> Self {
        let err_str = format!("{err:#}").to_lowercase();

        if err_str.contains("abnormal") || err_str.contains("too many") {
            Self::AbnormalActivity
        } else if err_str.contains("429") || err_str.contains("rate limit") {
            Self::RateLimit
        } else if err_str.contains("401") || err_str.contains("unauthorized") {
            Self::Unauthorized
        } else if err_str.contains("dns")
            || err_str.contains("timeout")
            || err_str.contains("connect")
            || err_str.contains("network")
        {
            Self::NetworkError
        } else {
            Self::Unknown
        }
    }
}

impl std::fmt::Display for LockoutType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AbnormalActivity => write!(f, "abnormal_activity"),
            Self::RateLimit => write!(f, "rate_limit"),
            Self::Unauthorized => write!(f, "unauthorized"),
            Self::NetworkError => write!(f, "network_error"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Persistent API lockout state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiLockout {
    /// Type of lockout
    pub lockout_type: LockoutType,
    /// When the lockout expires (UTC)
    pub lockout_until: DateTime<Utc>,
    /// Number of consecutive failures
    pub retry_count: u32,
    /// Last error message
    pub last_error: String,
    /// When this lockout was created
    pub created_at: DateTime<Utc>,
}

impl ApiLockout {
    /// Create a new lockout from an error
    pub fn from_error(err: &anyhow::Error) -> Self {
        let lockout_type = LockoutType::from_error(err);
        let duration = lockout_type.lockout_duration();
        let now = Utc::now();

        Self {
            lockout_type,
            lockout_until: now + ChronoDuration::from_std(duration).unwrap_or(ChronoDuration::hours(1)),
            retry_count: 1,
            last_error: format!("{err:#}"),
            created_at: now,
        }
    }

    /// Check if the lockout is still active
    pub fn is_active(&self) -> bool {
        Utc::now() < self.lockout_until
    }

    /// Get time remaining until lockout expires
    pub fn time_remaining(&self) -> Option<ChronoDuration> {
        let remaining = self.lockout_until - Utc::now();
        if remaining > ChronoDuration::zero() {
            Some(remaining)
        } else {
            None
        }
    }

    /// Increment the retry count and extend lockout if needed
    #[allow(dead_code)]
    pub fn increment_retry(&mut self, err: &anyhow::Error) {
        self.retry_count += 1;
        self.last_error = format!("{err:#}");

        // For repeated failures, extend the lockout
        if self.retry_count > 3 {
            let extension = self.lockout_type.lockout_duration();
            self.lockout_until = Utc::now()
                + ChronoDuration::from_std(extension).unwrap_or(ChronoDuration::hours(1));
        }
    }
}

const LOCKOUT_CACHE_KEY: &str = "api-lockout-state";
const LOCKOUT_CACHE_TOPIC: &str = "lockout";

/// Get the current API lockout state, if any (async version)
pub async fn get_api_lockout() -> Option<ApiLockout> {
    let cache = crate::cache::CACHE.load();
    let topic = cache.topic(LOCKOUT_CACHE_TOPIC).ok()?;

    let (_, current) = topic.get_for_update(LOCKOUT_CACHE_KEY).await.ok()?;
    let data = current?;

    serde_json::from_slice::<ApiLockout>(&data.data).ok()
}

/// Set the API lockout state (async version)
pub async fn set_api_lockout(lockout: &ApiLockout) -> anyhow::Result<()> {
    let cache = crate::cache::CACHE.load();
    let topic = cache.topic(LOCKOUT_CACHE_TOPIC)?;

    let data = serde_json::to_vec_pretty(lockout)?;

    // Use a long TTL - we want this to persist
    let (updater, _) = topic.get_for_update(LOCKOUT_CACHE_KEY).await?;
    updater.write(&data, Duration::from_secs(7 * 24 * 60 * 60))?; // 7 days

    log::info!(
        "API lockout set: type={}, until={}, retries={}",
        lockout.lockout_type,
        lockout.lockout_until,
        lockout.retry_count
    );

    Ok(())
}

/// Clear the API lockout state (called when API access is restored)
pub fn clear_api_lockout() -> anyhow::Result<()> {
    crate::cache::invalidate_key(LOCKOUT_CACHE_TOPIC, LOCKOUT_CACHE_KEY)?;
    log::info!("API lockout cleared - full API access restored");
    Ok(())
}

/// Check if we should attempt an API call based on lockout state
#[allow(dead_code)]
pub async fn should_attempt_api_call() -> bool {
    match get_api_lockout().await {
        Some(lockout) if lockout.is_active() => {
            if let Some(remaining) = lockout.time_remaining() {
                log::debug!(
                    "API locked out for {} more minutes ({})",
                    remaining.num_minutes(),
                    lockout.lockout_type
                );
            }
            false
        }
        Some(_) => {
            // Lockout expired, we can try again
            log::info!("API lockout expired, will attempt API access");
            true
        }
        None => true,
    }
}

/// Determine if an error is recoverable (should trigger degraded mode)
/// vs fatal (should bail completely)
pub fn is_recoverable_error(err: &anyhow::Error) -> bool {
    let err_str = format!("{err:#}").to_lowercase();

    // These are recoverable - we should continue in LAN mode
    let recoverable_patterns = [
        "abnormal",
        "rate limit",
        "429",
        "401",
        "unauthorized",
        "timeout",
        "dns",
        "connect",
        "network",
        "connection refused",
        "no route",
        "unreachable",
    ];

    recoverable_patterns.iter().any(|p| err_str.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lockout_type_from_error() {
        let abnormal = anyhow::anyhow!("Your account is abnormal, please contact support");
        assert_eq!(LockoutType::from_error(&abnormal), LockoutType::AbnormalActivity);

        let rate_limit = anyhow::anyhow!("HTTP 429: rate limit exceeded");
        assert_eq!(LockoutType::from_error(&rate_limit), LockoutType::RateLimit);

        let network = anyhow::anyhow!("DNS resolution failed");
        assert_eq!(LockoutType::from_error(&network), LockoutType::NetworkError);
    }

    #[test]
    fn test_lockout_duration() {
        assert_eq!(
            LockoutType::AbnormalActivity.lockout_duration(),
            Duration::from_secs(24 * 60 * 60)
        );
        assert_eq!(
            LockoutType::NetworkError.lockout_duration(),
            Duration::from_secs(5 * 60)
        );
    }

    #[test]
    fn test_is_recoverable() {
        assert!(is_recoverable_error(&anyhow::anyhow!("connection timeout")));
        assert!(is_recoverable_error(&anyhow::anyhow!("HTTP 429 rate limit")));
        assert!(is_recoverable_error(&anyhow::anyhow!("account is abnormal")));
        // Generic errors are not recoverable by default
        assert!(!is_recoverable_error(&anyhow::anyhow!("unknown error xyz")));
    }
}
