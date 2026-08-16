//! The engine's out-of-process helpers, discovered together.
//!
//! Grouped so the user's notify command reaches both of them from ONE place.
//! Wired per-call-site, it is invisible when one of them silently loses the
//! ability to say anything — and for the review dialog that means every take it
//! drops disappears without a word.

use crate::indicator::SubprocessIndicator;
use crate::notify::configured_notify_command;
use crate::review::SubprocessReviewDialog;

/// The review dialog and the recording-indicator overlay, both reporting
/// failures through the user's configured notifier.
pub struct EngineHelpers {
    /// Shows the take and takes the user's correction. Holds their words.
    pub dialog: SubprocessReviewDialog,
    /// The "voice is live" overlay. Cosmetic.
    pub indicator: SubprocessIndicator,
}

impl EngineHelpers {
    /// Discover both beside the running engine, using the notify command from
    /// the user's config.
    #[must_use]
    pub fn discover() -> Self {
        Self::with_notifier(&configured_notify_command())
    }

    /// Discover both with an explicit notifier.
    #[must_use]
    pub fn with_notifier(notify_command: &str) -> Self {
        Self {
            dialog: SubprocessReviewDialog::discover(notify_command),
            indicator: SubprocessIndicator::discover(notify_command),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_helpers_report_through_the_notifier_they_were_given() {
        let helpers = EngineHelpers::with_notifier("/opt/custom-notifier");

        assert_eq!(helpers.dialog.notify_command(), "/opt/custom-notifier");
        assert_eq!(helpers.indicator.notify_command(), "/opt/custom-notifier");
    }

    #[test]
    fn discovery_uses_the_users_configured_notifier() {
        // This is the only line joining the engine's supervision to a real
        // desktop. Replacing these arguments with "" disables every alert the
        // engine would ever raise — and `notify_user` treats an empty command
        // as "notifications off", so the whole thing goes quiet while every
        // other test stays green.
        let expected = configured_notify_command();

        let helpers = EngineHelpers::discover();

        assert_eq!(helpers.dialog.notify_command(), expected);
        assert_eq!(helpers.indicator.notify_command(), expected);
    }
}
