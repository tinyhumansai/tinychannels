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
    assert!(jitter_millis(1) < MAX_JITTER_MS);
    assert_eq!(jitter_millis(0), 0);
    assert_eq!(compute_max_in_flight_messages(100), 64);
}
