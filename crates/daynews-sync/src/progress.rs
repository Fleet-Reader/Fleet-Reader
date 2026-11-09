// SPDX-License-Identifier: Apache-2.0
//! Provider-neutral, count-based progress. Unknown work never gets a fictitious ETA.
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Preparing,
    Uploading,
    Subscriptions,
    ReadState,
    Articles,
    MissingArticles,
    Finalizing,
    Complete,
    Resetting,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub phase: Phase,
    /// Completed units in the current phase (operations, pages, or missing articles).
    pub completed: usize,
    pub total: Option<usize>,
    /// Newly inserted articles, not the number of entries received from the provider.
    pub articles: usize,
    pub subscriptions: usize,
    /// A publication represented in the current page, not a claim that its history is complete.
    pub feed: Option<String>,
}
impl Progress {
    pub fn fraction(&self) -> Option<f64> {
        let portion = || {
            self.total
                .filter(|n| *n > 0)
                .map(|n| (self.completed as f64 / n as f64).min(1.0))
        };
        match self.phase {
            Phase::Preparing | Phase::Resetting => None,
            Phase::Uploading => portion().map(|v| v * 0.05),
            Phase::Subscriptions => Some(0.05),
            Phase::ReadState => Some(0.08),
            Phase::Articles => portion().map(|v| 0.1 + 0.8 * v),
            Phase::MissingArticles => portion().map(|v| 0.9 + 0.07 * v),
            Phase::Finalizing => Some(0.97),
            Phase::Complete => Some(1.0),
        }
    }
    /// Estimate the remaining article-page work from observed throughput. Setup time is
    /// excluded; final read-state reconciliation and network variability remain approximate.
    pub fn remaining(&self, phase_elapsed: Duration) -> Option<Duration> {
        if !matches!(
            self.phase,
            Phase::Articles | Phase::MissingArticles | Phase::Uploading
        ) || self.completed == 0
            || phase_elapsed.as_secs_f64() < 1.0
        {
            return None;
        }
        let remaining = self.total?.checked_sub(self.completed)?;
        if remaining == 0 {
            return None;
        }
        Some(Duration::from_secs_f64(
            (phase_elapsed.as_secs_f64() / self.completed as f64 * remaining as f64)
                .min(365.0 * 86400.0),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn estimates_only_measured_work_and_never_claims_completion_early() {
        let mut p = Progress {
            phase: Phase::Articles,
            completed: 2,
            total: Some(10),
            ..Default::default()
        };
        assert_eq!(
            p.remaining(Duration::from_secs(4)),
            Some(Duration::from_secs(16))
        );
        assert!((p.fraction().unwrap() - 0.26).abs() < 0.0001);
        p.total = None;
        assert_eq!(p.remaining(Duration::from_secs(4)), None);
        assert_eq!(p.fraction(), None);
        p.total = Some(2);
        assert_eq!(p.remaining(Duration::from_secs(4)), None);
        assert!(p.fraction().unwrap() < 1.0);
        p.phase = Phase::Complete;
        assert_eq!(p.fraction(), Some(1.0));
    }
}
