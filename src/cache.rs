//! An on-disk cache of Reverb responses.
//!
//! Working out one price class costs tens of small requests, and the same counts get
//! asked for again the moment a caller adjusts an unrelated flag. Caching by URL keeps
//! repeat runs almost free and keeps gearprice a light guest on someone else's API.
//!
//! Each entry is one file: a single metadata line, then the response body verbatim. The
//! URL is stored rather than trusted from the file name, so a truncated write or a hash
//! collision reads as a miss instead of as another endpoint's answer.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct Entry {
    url: String,
    fetched_at: u64,
}

pub struct ResponseCache {
    directory: PathBuf,
    ttl: Duration,
}

impl ResponseCache {
    pub fn new(directory: PathBuf, ttl: Duration) -> Self {
        Self { directory, ttl }
    }

    fn path_for(&self, url: &str) -> PathBuf {
        let digest = Sha256::digest(url.as_bytes());
        let name: String = digest
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.directory.join(format!("{name}.json"))
    }

    /// The cached body for `url`, if one is present and still inside the TTL.
    ///
    /// Every failure path — unreadable file, malformed header, wrong URL, clock skew —
    /// is a miss. A cache is an optimisation; it is never allowed to fail a run.
    pub fn get(&self, url: &str) -> Option<String> {
        let contents = fs::read_to_string(self.path_for(url)).ok()?;
        let (header, body) = contents.split_once('\n')?;
        let entry: Entry = serde_json::from_str(header).ok()?;
        if entry.url != url {
            return None;
        }
        let age = now_seconds().checked_sub(entry.fetched_at)?;
        // Strictly less than, so a TTL of zero means "never serve from cache" rather
        // than "serve anything written this second".
        (age < self.ttl.as_secs()).then(|| body.to_string())
    }

    /// Stores a response. Written to a neighbouring temporary file and renamed, so a
    /// concurrent reader sees either the old entry or the new one and never a half file.
    pub fn put(&self, url: &str, body: &str) -> Result<()> {
        fs::create_dir_all(&self.directory).with_context(|| {
            format!("cannot create cache directory {}", self.directory.display())
        })?;
        let entry = Entry {
            url: url.to_string(),
            fetched_at: now_seconds(),
        };
        let path = self.path_for(url);
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let payload = format!("{}\n{body}", serde_json::to_string(&entry)?);
        fs::write(&temporary, payload)
            .with_context(|| format!("cannot write cache entry {}", temporary.display()))?;
        match fs::rename(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(error).with_context(|| format!("cannot publish cache entry {}", path.display()))
            }
        }
    }

    /// Deletes every cached response. Returns how many entries went.
    pub fn clear(&self) -> Result<usize> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("cannot read cache directory {}", self.directory.display())
                });
            }
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            if entry
                .path()
                .extension()
                .is_some_and(|value| value == "json")
                && fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Number of entries and total bytes currently held.
    pub fn usage(&self) -> (usize, u64) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return (0, 0);
        };
        entries
            .flatten()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|value| value == "json")
            })
            .filter_map(|entry| entry.metadata().ok())
            .fold((0, 0), |(count, bytes), metadata| {
                (count + 1, bytes + metadata.len())
            })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_response_reads_back_and_a_stale_one_does_not() {
        let directory = tempfile::tempdir().unwrap();
        let cache = ResponseCache::new(directory.path().to_path_buf(), Duration::from_secs(3600));
        cache
            .put("https://example.invalid/a", "{\"total\":7}")
            .unwrap();
        assert_eq!(
            Some("{\"total\":7}".to_string()),
            cache.get("https://example.invalid/a")
        );
        assert_eq!(None, cache.get("https://example.invalid/b"));

        let expired = ResponseCache::new(directory.path().to_path_buf(), Duration::ZERO);
        // Written a moment ago, but a zero TTL makes everything already stale.
        assert_eq!(None, expired.get("https://example.invalid/a"));
    }

    #[test]
    fn a_body_containing_newlines_survives_the_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let cache = ResponseCache::new(directory.path().to_path_buf(), Duration::from_secs(3600));
        let body = "{\n  \"description\": \"line one\nline two\"\n}";
        cache.put("https://example.invalid/multi", body).unwrap();
        assert_eq!(
            Some(body.to_string()),
            cache.get("https://example.invalid/multi")
        );
    }

    #[test]
    fn an_entry_whose_url_does_not_match_is_a_miss() {
        let directory = tempfile::tempdir().unwrap();
        let cache = ResponseCache::new(directory.path().to_path_buf(), Duration::from_secs(3600));
        cache.put("https://example.invalid/a", "body").unwrap();
        // Rewrite the entry's header to claim a different URL, as a hash collision would.
        let path = cache.path_for("https://example.invalid/a");
        let contents = fs::read_to_string(&path).unwrap();
        let body = contents.split_once('\n').unwrap().1.to_string();
        let header = serde_json::to_string(&Entry {
            url: "https://example.invalid/other".into(),
            fetched_at: now_seconds(),
        })
        .unwrap();
        fs::write(&path, format!("{header}\n{body}")).unwrap();
        assert_eq!(None, cache.get("https://example.invalid/a"));
    }

    #[test]
    fn clearing_removes_entries_and_reports_the_count() {
        let directory = tempfile::tempdir().unwrap();
        let cache = ResponseCache::new(directory.path().to_path_buf(), Duration::from_secs(3600));
        cache.put("https://example.invalid/a", "one").unwrap();
        cache.put("https://example.invalid/b", "two").unwrap();
        assert_eq!(2, cache.usage().0);
        assert_eq!(2, cache.clear().unwrap());
        assert_eq!(0, cache.usage().0);
        // Clearing an absent directory is a no-op, not an error.
        let missing = ResponseCache::new(directory.path().join("gone"), Duration::from_secs(1));
        assert_eq!(0, missing.clear().unwrap());
    }
}
