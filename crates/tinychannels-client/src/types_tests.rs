use super::*;

use tinybus::module::ModuleCallError;

#[test]
fn runtime_errors_keep_typed_module_causes_without_remote_payloads() {
    let error =
        ChannelRuntimeError::module(ChannelRuntimeOperation::Stop, ModuleCallError::ModuleFault);

    assert_eq!(error.operation(), ChannelRuntimeOperation::Stop);
    assert_eq!(
        error.kind(),
        ChannelRuntimeErrorKind::Module(ModuleCallError::ModuleFault)
    );
    assert!(std::error::Error::source(&error).is_some());
    assert!(!format!("{error:?}").contains("private remote payload"));
    assert!(!error.to_string().contains("private remote payload"));
}

#[test]
fn runtime_contract_errors_keep_their_kind_without_an_error_source() {
    for kind in [
        ChannelRuntimeErrorKind::InvalidReservation,
        ChannelRuntimeErrorKind::InvalidCallbackAddress,
    ] {
        let error =
            ChannelRuntimeError::contract(ChannelRuntimeOperation::ValidateReservation, kind);

        assert_eq!(
            error.operation(),
            ChannelRuntimeOperation::ValidateReservation
        );
        assert_eq!(error.kind(), kind);
        assert!(std::error::Error::source(&error).is_none());
        assert!(!format!("{error:?}").contains("private remote payload"));
        assert!(!error.to_string().contains("private remote payload"));
    }
}
