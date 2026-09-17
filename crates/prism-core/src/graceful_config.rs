//! Configuration for the graceful hook. Separated from `config.rs` only so the
//! hook's types sit beside nothing else that could imply Prism understands what
//! it is asking a workload to drop.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// What to say, and how long to wait for an answer.
///
/// Keys match `docs/architecture.md` §4.3 exactly, which was written before the
/// code and until now described something that did not exist. `Facet` carries
/// `deny_unknown_fields`, so a configuration copied out of that document failed
/// to parse rather than being ignored.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Graceful {
    pub http_post: HttpPost,
    /// How long to wait, as a duration string. Defaults to ten seconds, the
    /// figure in the document.
    #[serde(default = "default_timeout")]
    pub timeout: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpPost {
    pub url: String,
    /// Sent as written, except for `{placeholder}` spans — see
    /// `crate::graceful::Deficit::render`.
    pub body: String,
    /// Defaults to JSON, which is what every endpoint this addresses speaks.
    #[serde(default)]
    pub content_type: Option<String>,
}

fn default_timeout() -> String {
    "10s".into()
}

impl Graceful {
    /// The configured timeout, or ten seconds if it cannot be read.
    ///
    /// A malformed duration does not fail the load. The alternative is refusing
    /// to start a facet because its shed hook has a typo in a timeout, which
    /// trades a working workload for a cosmetic correctness — and the daemon's
    /// whole argument is that it should degrade rather than stop.
    pub fn timeout(&self) -> Duration {
        parse_duration(&self.timeout).unwrap_or(Duration::from_secs(10))
    }

    pub fn content_type(&self) -> &str {
        self.http_post
            .content_type
            .as_deref()
            .unwrap_or("application/json")
    }
}

/// Parse `"10s"`, `"500ms"`, `"2m"`. Bare numbers are seconds.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    // A bare number has no unit at all, so `find` returning None is the
    // all-digits case rather than a parse failure.
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, unit) = s.split_at(split);
    let n: u64 = digits.parse().ok()?;
    match unit.trim() {
        "ms" => Some(Duration::from_millis(n)),
        "s" | "" => Some(Duration::from_secs(n)),
        "m" => Some(Duration::from_secs(n * 60)),
        "h" => Some(Duration::from_secs(n * 3600)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_block_parses() {
        // Verbatim from docs/architecture.md §4.3, which is the point of this
        // type existing.
        let toml = r#"
http_post = { url = "http://127.0.0.1:8188/free",
              body = '{"unload_models":true,"free_memory":true}' }
timeout   = "10s"
"#;
        let g: Graceful = toml::from_str(toml).unwrap();
        assert_eq!(g.http_post.url, "http://127.0.0.1:8188/free");
        assert_eq!(g.timeout(), Duration::from_secs(10));
        assert_eq!(g.content_type(), "application/json");
    }

    #[test]
    fn a_missing_timeout_is_the_documented_default() {
        let toml = r#"http_post = { url = "http://x/y", body = "{}" }"#;
        let g: Graceful = toml::from_str(toml).unwrap();
        assert_eq!(g.timeout(), Duration::from_secs(10));
    }

    #[test]
    fn a_typo_in_the_timeout_does_not_stop_the_facet() {
        let g = Graceful {
            http_post: HttpPost { url: "http://x/y".into(), body: "{}".into(), content_type: None },
            timeout: "ten seconds".into(),
        };
        assert_eq!(g.timeout(), Duration::from_secs(10));
    }

    #[test]
    fn durations_parse_in_the_units_the_config_uses() {
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("30"), Some(Duration::from_secs(30)));
    }
}
