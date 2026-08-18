//! A record of what a market looked like, each time it was asked.
//!
//! Everything else gearprice reports is a photograph: this is what the gear costs today.
//! What it cost in March is not recoverable from Reverb afterwards at any price, so the
//! only way to know whether a market is moving is to have written it down at the time.
//!
//! Snapshots are appended to one file, one JSON object per line. A line is small enough
//! to be written in a single call, the format survives being read by anything, and a
//! corrupt line loses one reading rather than the history. It lives with the user's data
//! rather than in the cache, so emptying the cache cannot throw away the one thing that
//! cannot be fetched again.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::market::Market;
use crate::query::Condition;

/// One reading of one market.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub recorded_at: DateTime<Utc>,
    /// What was measured. Snapshots only compare against others with the same subject.
    pub subject: Subject,
    pub listings: u32,
    /// Percentile to price, in major units.
    pub percentiles: Vec<(f64, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median_days_listed: Option<i64>,
    /// The median of what the model actually sold for, where Reverb had a record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median_sold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sales_read: Option<usize>,
}

impl Snapshot {
    pub fn of(subject: Subject, market: &Market, sold: Option<&crate::sold::Sold>) -> Self {
        let currency = market.currency.as_str();
        Self {
            median_sold: sold
                .and_then(crate::sold::Sold::median)
                .map(|price| price.major(currency)),
            sales_read: sold.map(|sold| sold.sales.len()),
            recorded_at: Utc::now(),
            subject,
            listings: market.total,
            percentiles: market
                .percentiles
                .iter()
                .map(|(fraction, price)| (*fraction, price.major(currency)))
                .collect(),
            median_days_listed: market.days_listed_between(None, None),
        }
    }

    pub fn percentile(&self, fraction: f64) -> Option<f64> {
        self.percentiles
            .iter()
            .find(|(wanted, _)| (wanted - fraction).abs() < f64::EPSILON)
            .map(|(_, price)| *price)
    }

    pub fn median(&self) -> Option<f64> {
        self.percentile(0.50)
    }
}

/// What a snapshot is a reading of.
///
/// Two snapshots are only comparable when all of this matches. A median in kroner against
/// one in dollars, or used gear against new, is not a price movement — it is a different
/// question asked twice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    /// The catalogue model, where the query resolved to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<u64>,
    pub title: String,
    pub currency: String,
    pub condition: Condition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ships_to: Option<String>,
}

impl Subject {
    /// Whether two readings are of the same thing, and so worth comparing.
    pub fn matches(&self, other: &Self) -> bool {
        self.currency == other.currency
            && self.condition == other.condition
            && self.ships_to == other.ships_to
            && match (self.model_id, other.model_id) {
                // A model id is the reliable identity; a title is what is left without one.
                (Some(left), Some(right)) => left == right,
                _ => self.title.eq_ignore_ascii_case(&other.title),
            }
    }
}

/// Whether the file ends mid-line, and so needs a break before anything is appended.
fn needs_newline(file: &mut fs::File) -> Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let length = file.metadata()?.len();
    if length == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(length - 1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

pub struct History {
    path: PathBuf,
}

impl History {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one reading. Written in a single call so a concurrent run cannot interleave
    /// half a line into it.
    pub fn record(&self, snapshot: &Snapshot) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create history directory {}", parent.display()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("cannot open history {}", self.path.display()))?;

        // A run killed mid-write leaves a line with no newline on the end. Appending
        // straight onto it would glue this reading to that fragment and lose both, so the
        // break is put back first. One truncated line should cost one reading.
        let mut line = String::new();
        if needs_newline(&mut file)? {
            line.push('\n');
        }
        line.push_str(&serde_json::to_string(snapshot)?);
        line.push('\n');

        file.write_all(line.as_bytes())
            .with_context(|| format!("cannot write history {}", self.path.display()))
    }

    /// Every readable snapshot, oldest first.
    ///
    /// A line that will not parse is skipped rather than fatal: a truncated write from a
    /// killed run should cost one reading, not the whole record.
    pub fn read(&self) -> Result<Vec<Snapshot>> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot read history {}", self.path.display()));
            }
        };
        let mut snapshots: Vec<Snapshot> = contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        snapshots.sort_by_key(|snapshot| snapshot.recorded_at);
        Ok(snapshots)
    }

    /// Past readings of the same thing, oldest first.
    pub fn readings_of(&self, subject: &Subject) -> Result<Vec<Snapshot>> {
        Ok(self
            .read()?
            .into_iter()
            .filter(|snapshot| snapshot.subject.matches(subject))
            .collect())
    }

    /// Every distinct thing the history has readings of, most recently seen first.
    pub fn subjects(&self) -> Result<Vec<(Subject, usize, DateTime<Utc>)>> {
        let mut tracked: Vec<(Subject, usize, DateTime<Utc>)> = Vec::new();
        for snapshot in self.read()? {
            match tracked
                .iter_mut()
                .find(|(subject, _, _)| subject.matches(&snapshot.subject))
            {
                Some((_, count, last)) => {
                    *count += 1;
                    *last = (*last).max(snapshot.recorded_at);
                }
                None => tracked.push((snapshot.subject, 1, snapshot.recorded_at)),
            }
        }
        tracked.sort_by_key(|(_, _, last)| std::cmp::Reverse(*last));
        Ok(tracked)
    }
}

/// The change between the first and last reading of a market.
#[derive(Clone, Debug, Serialize)]
pub struct Movement {
    pub since: DateTime<Utc>,
    pub readings: usize,
    pub median_then: Option<f64>,
    pub median_now: Option<f64>,
    pub sold_then: Option<f64>,
    pub sold_now: Option<f64>,
    pub listings_then: u32,
    pub listings_now: u32,
}

impl Movement {
    pub fn between(readings: &[Snapshot]) -> Option<Self> {
        let (first, last) = (readings.first()?, readings.last()?);
        // One reading is a starting point, not a movement.
        (readings.len() >= 2).then(|| Self {
            since: first.recorded_at,
            readings: readings.len(),
            median_then: first.median(),
            median_now: last.median(),
            sold_then: first.median_sold,
            sold_now: last.median_sold,
            listings_then: first.listings,
            listings_now: last.listings,
        })
    }

    pub fn median_change(&self) -> Option<f64> {
        Some(self.median_now? - self.median_then?)
    }

    pub fn median_change_share(&self) -> Option<f64> {
        let then = self.median_then?;
        if then.abs() <= f64::EPSILON {
            return None;
        }
        Some(self.median_change()? / then)
    }
}

/// Where the record of past readings lives.
///
/// Under the user's data directory rather than the cache: a cached response can be
/// fetched again, and a market as it stood in March cannot.
pub fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os("GEARPRICE_HISTORY") {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("gearprice/history.jsonl");
    }
    crate::paths::home_dir_in(&crate::paths::ProcessEnvironment)
        .join(".local/share/gearprice/history.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(currency: &str, condition: Condition) -> Subject {
        Subject {
            model_id: Some(104_713),
            title: "Gibson Les Paul Standard '60s".into(),
            currency: currency.into(),
            condition,
            ships_to: None,
        }
    }

    fn snapshot(at: &str, median: f64, listings: u32, subject: Subject) -> Snapshot {
        Snapshot {
            recorded_at: DateTime::parse_from_rfc3339(at)
                .unwrap()
                .with_timezone(&Utc),
            subject,
            listings,
            percentiles: vec![
                (0.25, median - 100.0),
                (0.50, median),
                (0.75, median + 100.0),
            ],
            median_days_listed: Some(40),
            median_sold: Some(median * 0.85),
            sales_read: Some(20),
        }
    }

    #[test]
    fn readings_come_back_in_order_whatever_order_they_were_written() {
        let directory = tempfile::tempdir().unwrap();
        let history = History::new(directory.path().join("history.jsonl"));
        assert!(
            history.read().unwrap().is_empty(),
            "a missing file is an empty history"
        );

        let used = subject("USD", Condition::Used);
        history
            .record(&snapshot(
                "2026-08-01T00:00:00Z",
                2_275.0,
                174,
                used.clone(),
            ))
            .unwrap();
        history
            .record(&snapshot(
                "2026-03-12T00:00:00Z",
                2_300.0,
                198,
                used.clone(),
            ))
            .unwrap();

        let readings = history.readings_of(&used).unwrap();
        assert_eq!(2, readings.len());
        assert_eq!(Some(2_300.0), readings[0].median());
        assert_eq!(Some(2_275.0), readings[1].median());
    }

    #[test]
    fn readings_of_different_questions_are_never_compared() {
        let directory = tempfile::tempdir().unwrap();
        let history = History::new(directory.path().join("history.jsonl"));
        let usd = subject("USD", Condition::Used);
        history
            .record(&snapshot("2026-03-12T00:00:00Z", 2_300.0, 198, usd.clone()))
            .unwrap();
        history
            .record(&snapshot(
                "2026-08-01T00:00:00Z",
                24_000.0,
                174,
                subject("NOK", Condition::Used),
            ))
            .unwrap();
        history
            .record(&snapshot(
                "2026-08-01T00:00:00Z",
                2_899.0,
                241,
                subject("USD", Condition::New),
            ))
            .unwrap();

        // A median in kroner is not this market moving, and neither is one for new stock.
        assert_eq!(1, history.readings_of(&usd).unwrap().len());
        assert_eq!(3, history.subjects().unwrap().len());
    }

    #[test]
    fn a_corrupt_line_costs_one_reading_and_not_the_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.jsonl");
        let history = History::new(path.clone());
        let used = subject("USD", Condition::Used);
        history
            .record(&snapshot(
                "2026-03-12T00:00:00Z",
                2_300.0,
                198,
                used.clone(),
            ))
            .unwrap();
        // A run killed mid-write leaves a half line behind.
        fs::write(
            &path,
            format!(
                "{}{{\"recorded_at\":\"2026-08",
                fs::read_to_string(&path).unwrap()
            ),
        )
        .unwrap();
        history
            .record(&snapshot(
                "2026-08-01T00:00:00Z",
                2_275.0,
                174,
                used.clone(),
            ))
            .unwrap();

        assert_eq!(2, history.readings_of(&used).unwrap().len());
    }

    #[test]
    fn a_single_reading_is_a_starting_point_rather_than_a_movement() {
        let used = subject("USD", Condition::Used);
        assert!(Movement::between(&[]).is_none());
        assert!(
            Movement::between(&[snapshot("2026-03-12T00:00:00Z", 2_300.0, 198, used.clone())])
                .is_none()
        );

        let readings = vec![
            snapshot("2026-03-12T00:00:00Z", 2_300.0, 198, used.clone()),
            snapshot("2026-05-01T00:00:00Z", 2_280.0, 186, used.clone()),
            snapshot("2026-08-01T00:00:00Z", 2_223.0, 174, used),
        ];
        let movement = Movement::between(&readings).unwrap();
        assert_eq!(3, movement.readings);
        assert_eq!(Some(-77.0), movement.median_change());
        // What it was trading at is recorded beside what it was listed at.
        assert_eq!(Some(2_300.0 * 0.85), movement.sold_then);
        assert_eq!(Some(2_223.0 * 0.85), movement.sold_now);
        let share = movement.median_change_share().unwrap();
        assert!((share + 0.0334782).abs() < 1e-6, "{share}");
        assert_eq!(198, movement.listings_then);
        assert_eq!(174, movement.listings_now);
    }
}
