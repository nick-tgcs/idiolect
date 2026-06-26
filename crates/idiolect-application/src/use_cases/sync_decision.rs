/// Returns `true` when an auto-train run should be triggered: auto-train is
/// enabled and the number of pending, trainable corrections has reached or
/// exceeded the configured threshold.
///
/// Pure function — no I/O, fully deterministic, easy to unit-test.
#[must_use]
pub fn should_auto_train(auto_train: bool, threshold: u32, trainable_count: u64) -> bool {
    auto_train && trainable_count >= u64::from(threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_false_when_auto_train_is_disabled_regardless_of_count() {
        assert!(!should_auto_train(false, 0, 0));
        assert!(!should_auto_train(false, 0, 100));
        assert!(!should_auto_train(false, 25, 100));
    }

    #[test]
    fn returns_false_when_count_is_below_threshold() {
        assert!(!should_auto_train(true, 25, 0));
        assert!(!should_auto_train(true, 25, 24));
    }

    #[test]
    fn returns_true_when_count_equals_threshold() {
        assert!(should_auto_train(true, 25, 25));
    }

    #[test]
    fn returns_true_when_count_exceeds_threshold() {
        assert!(should_auto_train(true, 25, 100));
    }

    #[test]
    fn threshold_zero_fires_as_soon_as_there_is_any_correction() {
        // An explicit threshold of 0 with auto_train on fires even with a single
        // correction — matches the "always train" mental model.
        assert!(should_auto_train(true, 0, 0));
        assert!(should_auto_train(true, 0, 1));
    }
}
