//! Dead-man's switch: ping `HEALTHCHECK_URL` (e.g. healthchecks.io) every few minutes.
//!
//! A machine that loses power can't report anything itself. The external service notices the
//! missing pings instead and alerts once its grace period runs out. Pings start only after a
//! successful startup, so a service stuck in a crash loop is reported as down too.

use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tracing::{debug, info, warn};

use crate::clock::{Clock, SystemClock};
use crate::config::HealthcheckConfig;

const PING_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP GET to the ping URL.
pub struct HttpPinger {
    agent: ureq::Agent,
    url: String,
}

impl HttpPinger {
    pub fn new(config: &HealthcheckConfig) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(PING_TIMEOUT))
            .build()
            .into();
        Self {
            agent,
            url: config.url.expose().to_owned(),
        }
    }

    pub fn ping(&self) -> Result<()> {
        let body = match self.agent.get(&self.url).call() {
            Ok(mut response) => response.body_mut().read_to_string().unwrap_or_default(),
            // This variant quotes the URL, which is secret.
            Err(ureq::Error::BadUri(_)) => {
                return Err(anyhow!("HEALTHCHECK_URL is not a valid URL"));
            }
            Err(e) => return Err(anyhow!("healthcheck ping failed: {e}")),
        };
        check_ping_response(&body)
    }
}

/// healthchecks.io answers an unknown check with `200 OK (not found)`.
fn check_ping_response(body: &str) -> Result<()> {
    if body.contains("not found") {
        return Err(anyhow!(
            "healthcheck ping URL is unknown to the server; check HEALTHCHECK_URL"
        ));
    }
    Ok(())
}

/// Pings in a background thread until shutdown.
pub fn spawn(config: &HealthcheckConfig, clock: SystemClock) -> Result<JoinHandle<()>> {
    let pinger = HttpPinger::new(config);
    let interval = config.interval;
    info!(
        interval_minutes = interval.as_secs() / 60,
        "sending healthcheck pings"
    );
    Ok(std::thread::Builder::new()
        .name("heartbeat".to_owned())
        .spawn(move || heartbeat_loop(&clock, interval, || pinger.ping()))?)
}

/// Pings now and then every `interval` until shutdown. Logs a warning when pings start failing
/// and when they recover, not on every failed attempt.
pub fn heartbeat_loop(clock: &dyn Clock, interval: Duration, mut ping: impl FnMut() -> Result<()>) {
    let mut healthy = true;
    loop {
        match ping() {
            Ok(()) if healthy => debug!("healthcheck ping sent"),
            Ok(()) => {
                info!("healthcheck pings work again");
                healthy = true;
            }
            Err(e) if healthy => {
                warn!(
                    error = format!("{e:#}"),
                    "healthcheck ping failed; will keep trying"
                );
                healthy = false;
            }
            Err(e) => debug!(error = format!("{e:#}"), "healthcheck ping failed"),
        }
        if !clock.sleep(interval) {
            break;
        }
    }
    debug!("heartbeat stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::testing::FakeClock;

    #[test]
    fn pings_every_interval_until_shutdown() {
        let clock = FakeClock::at("2026-10-03T12:00:00Z").interrupt_after(3);
        let mut pings = Vec::new();
        heartbeat_loop(&clock, Duration::from_secs(300), || {
            pings.push(clock.now());
            Ok(())
        });
        assert_eq!(pings.len(), 4);
        assert_eq!((pings[3] - pings[0]).num_minutes(), 15);
    }

    #[test]
    fn keeps_pinging_after_failures() {
        let clock = FakeClock::at("2026-10-03T12:00:00Z").interrupt_after(4);
        let mut attempt = 0;
        heartbeat_loop(&clock, Duration::from_secs(60), || {
            attempt += 1;
            if (2..=3).contains(&attempt) {
                Err(anyhow!("network down"))
            } else {
                Ok(())
            }
        });
        assert_eq!(attempt, 5);
    }

    #[test]
    fn unknown_check_is_an_error() {
        assert!(check_ping_response("OK").is_ok());
        assert!(check_ping_response("").is_ok());
        assert!(check_ping_response("OK (not found)").is_err());
    }

    #[test]
    fn bad_url_error_does_not_leak_it() {
        let config = HealthcheckConfig {
            url: crate::config::Secret::new("https://exa mple.com/secret-uuid"),
            interval: Duration::from_secs(60),
        };
        let err = HttpPinger::new(&config).ping().unwrap_err();
        assert!(!format!("{err:#}").contains("secret-uuid"), "{err:#}");
    }
}
