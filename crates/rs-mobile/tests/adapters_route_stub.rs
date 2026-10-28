mod support;

use std::ffi::c_void;

use rs_mobile::rscore::{
    adapters::api::{
        adapters_cancel_credentials, adapters_close, adapters_resize, adapters_send_ctrl_alt_del,
        adapters_set_monitors, adapters_trust_cert, adapters_type_password,
    },
    adapters::ffi_raw::{rs_raw_attach, RsRawReadyCb},
    adapters::ADAPTER_ID_BASE,
    api_sessions::{
        sessions_action, sessions_background, sessions_clipboard_send, sessions_close,
        sessions_combo, sessions_key, sessions_text,
    },
    error::CoreError,
    ffi_session::{
        rs_input_button, rs_input_pointer, rs_input_release_all, rs_input_scroll, rs_input_touch,
        RsEventCb,
    },
    runtime::install_core,
};

fn assert_unknown_session(result: Result<(), CoreError>) {
    match result {
        Err(CoreError::Internal { message }) => {
            assert!(message.contains("unknown adapter session"), "{message}")
        }
        other => panic!("expected an unknown-session error, got {other:?}"),
    }
}

#[test]
fn adapter_id_input_routes_without_panicking() {
    let core = support::test_core();
    install_core(core.clone());
    let id = ADAPTER_ID_BASE + 1;

    assert_unknown_session(sessions_key(id, 4, true, 0));
    assert_unknown_session(sessions_text(id, "a".to_owned()));
    assert_unknown_session(sessions_combo(id, "altTab".to_owned()));
    assert_unknown_session(sessions_clipboard_send(id, "hi".to_owned()));
    assert_unknown_session(adapters_send_ctrl_alt_del(id));
    assert_unknown_session(sessions_action(id, "ctrlAltDel".to_owned(), None));
    assert_unknown_session(adapters_type_password(id));

    rs_input_pointer(id, 0.5, 0.5);
    rs_input_button(id, 0, true, 0.5, 0.5, 1);
    rs_input_scroll(id, 0.0, 10.0, 1, 0);
    rs_input_touch(id, 0, 0, 0.5, 0.5, 1.0);
    rs_input_release_all(id);

    // Closing or answering a session that is gone is not an error.
    assert_eq!(sessions_close(id), Ok(()));
    for paused in [true, false] {
        assert_eq!(
            sessions_background(id, paused),
            Err(CoreError::internal(format!("unknown session {id}")))
        );
    }
    assert_eq!(adapters_close(id), Ok(()));
    assert_eq!(adapters_cancel_credentials(id), Ok(()));
    assert_eq!(adapters_trust_cert(id, true), Ok(()));
    assert_eq!(adapters_resize(id, 390, 844, 3.0), Ok(()));
    assert_eq!(adapters_set_monitors(id, 2), Ok(()));

    extern "C" fn ready(_: *mut c_void, _: u64, _: u32, _: u32) {}
    extern "C" fn event(_: *mut c_void, _: *const rs_mobile::rscore::ffi_session::RsEvent) {}
    let attached = unsafe {
        rs_raw_attach(
            id,
            std::ptr::null_mut(),
            Some(ready as RsRawReadyCb),
            Some(event as RsEventCb),
        )
    };
    assert!(!attached);
}
