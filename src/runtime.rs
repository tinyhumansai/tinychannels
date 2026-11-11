//! Runtime mechanics re-exported from the lightweight runtime crate.

pub use tinychannels_runtime::{
    ChannelHealthState, ChannelSession, FAILURE_ESCALATE_THRESHOLD, FailureStreak, MAX_JITTER_MS,
    RuntimeChannelMessage, classify_health_result, compute_max_in_flight_messages, jitter_millis,
    log_worker_join_result, run_dispatch_loop, run_in_session, select_acknowledgment_reaction,
};
