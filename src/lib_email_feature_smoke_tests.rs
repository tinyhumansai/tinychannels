use crate::EmailChannel;

#[test]
fn email_channel_is_available_with_email_feature() {
    // Verify EmailChannel is exported whenever the send half is enabled.
    // Gated on `email-send`, not `email`: a send-only consumer keeps the
    // established `tinychannels::EmailChannel` path, and gating the
    // crate-root export on the full feature would silently remove the type
    // from the root for exactly the build this split exists to serve.
    let _ = std::any::type_name::<EmailChannel>();
}
