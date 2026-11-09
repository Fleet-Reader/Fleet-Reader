// SPDX-License-Identifier: Apache-2.0
//! Deterministic publication-based polling. Samples survive retention and app restarts.
const MIN: i64 = 30 * 60;
const MAX: i64 = 24 * 60 * 60;
const STEPS: [i64; 8] = [MIN, 3600, 7200, 14400, 28800, 43200, 64800, MAX];

pub fn publication_history(
    previous: Option<&str>,
    dates: impl IntoIterator<Item = i64>,
    now: i64,
) -> String {
    let mut dates: Vec<_> = previous
        .into_iter()
        .flat_map(|s| s.split(','))
        .filter_map(|s| s.parse::<i64>().ok())
        .chain(dates)
        .filter(|date| *date > 0 && *date <= now)
        .collect();
    dates.sort_unstable();
    dates.dedup();
    dates.drain(..dates.len().saturating_sub(32));
    dates
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

pub fn interval(history: Option<&str>, now: i64) -> i64 {
    let dates: Vec<i64> = history
        .into_iter()
        .flat_map(|s| s.split(','))
        .filter_map(|s| s.parse().ok())
        .filter(|date| *date > 0 && *date <= now)
        .collect();
    let Some(latest) = dates.last() else {
        return 2 * 3600;
    };
    let age = now.saturating_sub(*latest);
    let mut gaps: Vec<_> = dates
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|gap| *gap > 0)
        .collect();
    // The last eight gaps reflect changes in cadence without one old archive dominating.
    gaps.drain(..gaps.len().saturating_sub(8));
    gaps.sort_unstable();
    let baseline = if gaps.is_empty() {
        2 * 3600
    } else {
        let cadence = gaps[gaps.len() / 2];
        if cadence <= 12 * 3600 {
            MIN
        } else {
            cadence / 8
        }
    };
    let desired = baseline.max(age / 4).clamp(MIN, MAX);
    STEPS
        .into_iter()
        .find(|step| *step >= desired)
        .unwrap_or(MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn follows_recent_cadence_and_backs_off_stale_feeds() {
        let now = 100 * 86400;
        let history = |gap: i64| publication_history(None, (0_i64..12).map(|i| now - i * gap), now);
        assert_eq!(interval(Some(&history(4 * 3600)), now), MIN);
        assert_eq!(interval(Some(&history(86400)), now), 4 * 3600);
        assert_eq!(interval(Some(&history(7 * 86400)), now), MAX);
        assert_eq!(interval(Some(&history(30 * 86400)), now), MAX);
        assert_eq!(interval(Some(&history(4 * 3600)), now + 7 * 86400), MAX);
        assert_eq!(interval(None, now), 2 * 3600);
    }
    #[test]
    fn ignores_future_undated_duplicate_dates_and_bounds_history() {
        let history = publication_history(Some("0,1,1,bad"), [1, 2, 10000], 100);
        assert_eq!(history, "1,2");
        let history = publication_history(Some(&history), 1..100, 100);
        assert_eq!(history.split(',').count(), 32);
        assert!(history.ends_with(",99"));
    }
}
