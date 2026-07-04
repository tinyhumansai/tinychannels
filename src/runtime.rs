//! Runtime helper functions that are independent of OpenHuman application state.

use crate::context::{
    CHANNEL_MAX_IN_FLIGHT_MESSAGES, CHANNEL_MIN_IN_FLIGHT_MESSAGES, CHANNEL_PARALLELISM_PER_CHANNEL,
};

pub fn compute_max_in_flight_messages(channel_count: usize) -> usize {
    channel_count
        .saturating_mul(CHANNEL_PARALLELISM_PER_CHANNEL)
        .clamp(
            CHANNEL_MIN_IN_FLIGHT_MESSAGES,
            CHANNEL_MAX_IN_FLIGHT_MESSAGES,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_max_in_flight_messages_zero_channels() {
        assert_eq!(
            compute_max_in_flight_messages(0),
            CHANNEL_MIN_IN_FLIGHT_MESSAGES
        );
    }

    #[test]
    fn compute_max_in_flight_messages_one_channel() {
        let result = compute_max_in_flight_messages(1);
        assert!(result >= CHANNEL_MIN_IN_FLIGHT_MESSAGES);
        assert!(result <= CHANNEL_MAX_IN_FLIGHT_MESSAGES);
    }

    #[test]
    fn compute_max_in_flight_messages_many_channels() {
        assert_eq!(
            compute_max_in_flight_messages(100),
            CHANNEL_MAX_IN_FLIGHT_MESSAGES
        );
    }

    #[test]
    fn compute_max_in_flight_messages_clamps_to_max() {
        assert!(compute_max_in_flight_messages(usize::MAX) <= CHANNEL_MAX_IN_FLIGHT_MESSAGES);
    }
}
