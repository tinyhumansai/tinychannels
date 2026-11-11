use super::*;

#[test]
fn acknowledgement_selection_is_stable_and_contextual() {
    assert!(matches!(
        select_acknowledgment_reaction("thanks"),
        "❤️" | "🙏"
    ));
    assert_eq!(
        select_acknowledgment_reaction("thanks"),
        select_acknowledgment_reaction("thanks")
    );
    assert_eq!(compute_max_in_flight_messages(100), 64);
}

#[test]
fn acknowledgement_categories_keep_their_context_and_priority() {
    for (content, choices) in [
        ("THANKS for the code", ["❤️", "🙏"]),
        ("awesome!!", ["🔥", "🎉"]),
        ("btc price?", ["💯", "⚡"]),
        ("rust function?", ["👨‍💻", "🤓"]),
        ("hello there", ["🤗", "😁"]),
        ("yo", ["🤗", "😁"]),
        ("yo friend", ["🤗", "😁"]),
        ("what happened", ["🤔", "✍️"]),
        ("really?", ["🤔", "✍️"]),
        ("ordinary message", ["👀", "✍️"]),
        ("", ["👀", "✍️"]),
    ] {
        assert!(
            choices.contains(&select_acknowledgment_reaction(content)),
            "{content}"
        );
    }
    assert_eq!(compute_max_in_flight_messages(0), 8);
    assert_eq!(compute_max_in_flight_messages(3), 12);
    assert_eq!(compute_max_in_flight_messages(usize::MAX), 64);
}

#[test]
fn jitter_millis_is_bounded_for_short_and_capped_backoffs() {
    for _ in 0..256 {
        assert!(jitter_millis(1) < 1_000);
        assert!(jitter_millis(60) < MAX_JITTER_MS);
    }
    assert_eq!(jitter_millis(0), 0);
}

#[tokio::test]
async fn worker_join_logging_accepts_success_and_panic() {
    log_worker_join_result(Ok(()));
    let task = tokio::spawn(async { panic!("fixture panic") });
    let result = task.await;
    assert!(result.is_err());
    log_worker_join_result(result.map(|_| ()));
}

#[test]
fn a_failure_streak_escalates_once_and_a_healthy_session_resets_it() {
    let mut streak = FailureStreak::default();
    // Transient failures below the threshold stay warnings.
    for _ in 1..FAILURE_ESCALATE_THRESHOLD {
        assert!(!streak.record_failure());
    }
    // The failure that completes the streak escalates, exactly once.
    assert!(streak.record_failure());
    assert_eq!(streak.failures(), FAILURE_ESCALATE_THRESHOLD);
    assert!(
        !streak.record_failure(),
        "a long outage escalates once, not per retry"
    );

    // A healthy session starts the count over.
    streak.record_success();
    assert_eq!(streak.failures(), 0);
    assert!(!streak.record_failure());
}
