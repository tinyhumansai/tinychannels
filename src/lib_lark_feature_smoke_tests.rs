use crate::LarkChannel;

#[test]
fn lark_channel_is_available_with_lark_feature() {
    // Verify LarkChannel is exported when `lark` feature is enabled.
    // This test ensures the export is reachable at compile time.
    let _ = std::any::type_name::<LarkChannel>();
}
