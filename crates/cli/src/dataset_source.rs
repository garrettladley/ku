//! File and HTTP dataset inputs share the same checked record parser.

use std::{fmt, fs, path::Path, str::FromStr, time::Duration};

use anyhow::{Context, Result, ensure};
use ku_core::dataset::{self, Record};
use ureq::{Agent, http::Uri};

const MAX_DATASET_BYTES: u64 = 64 * 1024 * 1024;

/// An explicit HTTP(S) source, validated before any request or terminal setup.
#[derive(Clone, Debug)]
pub(crate) struct HttpUrl(Uri);

impl FromStr for HttpUrl {
    type Err = &'static str;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        let uri: Uri = input
            .parse()
            .map_err(|_| "expected an absolute HTTP(S) URL")?;
        if !matches!(uri.scheme_str(), Some("http" | "https"))
            || uri.host().is_none_or(str::is_empty)
            || uri
                .authority()
                .is_some_and(|authority| authority.as_str().contains('@'))
        {
            return Err("expected an absolute HTTP(S) URL without embedded credentials");
        }
        Ok(Self(uri))
    }
}

impl fmt::Display for HttpUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub(crate) fn load_file(path: &Path) -> Result<Vec<Record>> {
    let input = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&input, &path.display().to_string())
}

pub(crate) fn load_url(url: &HttpUrl) -> Result<Vec<Record>> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .max_redirects(5)
        .max_redirects_will_error(true)
        .https_only(url.0.scheme_str() == Some("https"))
        .user_agent(concat!("ku/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut response = agent
        .get(url.0.clone())
        .call()
        .with_context(|| format!("fetching {url}"))?;
    let input = read_body(response.body_mut(), MAX_DATASET_BYTES)
        .with_context(|| format!("reading {url} (UTF-8 dataset, maximum 64 MiB)"))?;
    parse(&input, &url.to_string())
}

fn read_body(body: &mut ureq::Body, limit: u64) -> std::result::Result<String, ureq::Error> {
    body.with_config()
        // ureq's limiter errors once exhausted, even when EOF is exactly at
        // the limit. Allow one lookahead byte so our advertised cap is inclusive.
        .limit(limit.saturating_add(1))
        .lossy_utf8(false)
        .read_to_string()
        .map_err(|error| match error {
            ureq::Error::BodyExceedsLimit(_) => ureq::Error::BodyExceedsLimit(limit),
            error => error,
        })
}

fn parse(input: &str, source: &str) -> Result<Vec<Record>> {
    let records = dataset::parse(input).with_context(|| format!("parsing {source}"))?;
    ensure!(!records.is_empty(), "{source} contains no puzzles");
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_limit_rejects_overflow_instead_of_accepting_a_truncated_dataset() {
        let mut exact = ureq::Body::builder().data("12345678");
        assert_eq!(read_body(&mut exact, 8).unwrap(), "12345678");
        let mut oversized = ureq::Body::builder().data("123456789");
        assert!(matches!(
            read_body(&mut oversized, 8),
            Err(ureq::Error::BodyExceedsLimit(8))
        ));
    }

    #[test]
    fn accepts_only_explicit_http_urls_without_credentials() {
        for invalid in [
            "puzzles.txt",
            "file:///tmp/puzzles.txt",
            "ftp://example.com/puzzles",
            "https://",
            "http://user:pass@example.com/puzzles",
        ] {
            assert!(invalid.parse::<HttpUrl>().is_err(), "{invalid}");
        }
        for valid in [
            "https://example.com/puzzles.txt?format=csv",
            "http://127.0.0.1:8080/puzzles",
        ] {
            assert_eq!(valid.parse::<HttpUrl>().unwrap().to_string(), valid);
        }
    }
}
