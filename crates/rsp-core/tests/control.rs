use prost::Message;
use rsp_core::control::{encode, ControlError, EnvelopeDecoder, MAX_CONTROL_FRAME};
use rsp_core::pb;

fn round_trip(env: pb::Envelope) {
    let bytes = encode(&env);
    let mut decoder = EnvelopeDecoder::new();
    for byte in bytes {
        decoder.push(&[byte]);
    }
    assert_eq!(decoder.next().unwrap(), Some(env));
    assert_eq!(decoder.next().unwrap(), None);
}

#[test]
fn every_control_oneof_member_round_trips() {
    round_trip(pb::ClientHello::default().into());
    round_trip(pb::ServerHello::default().into());
    round_trip(pb::AuthResult::default().into());
    round_trip(pb::SessionState::default().into());
    round_trip(pb::StartStream::default().into());
    round_trip(pb::StreamConfig::default().into());
    round_trip(pb::KeyframeRequest::default().into());
    round_trip(pb::Feedback::default().into());
    round_trip(pb::RateUpdate::default().into());
    round_trip(pb::Ping::default().into());
    round_trip(pb::Pong::default().into());
    round_trip(pb::DisplayList::default().into());
    round_trip(pb::SelectDisplay::default().into());
    round_trip(pb::RequestControl::default().into());
    round_trip(pb::ControlRequest::default().into());
    round_trip(pb::ControlResponse::default().into());
    round_trip(pb::LocalActivity::default().into());
    round_trip(pb::IdleWarning::default().into());
    round_trip(pb::PauseStream::default().into());
    round_trip(pb::ResumeStream::default().into());
    round_trip(pb::HostStats::default().into());
    round_trip(pb::SessionAction::default().into());
    round_trip(pb::SetQuality::default().into());
    round_trip(pb::AttachInput::default().into());
    round_trip(pb::PairStart::default().into());
    round_trip(pb::PairSpake::default().into());
    round_trip(pb::PairConfirm::default().into());
    round_trip(pb::PairResult::default().into());
    round_trip(pb::ErrorInfo::default().into());
    round_trip(pb::Goodbye::default().into());
}

#[test]
fn unknown_fields_are_ignored_and_unknown_oneof_is_empty() {
    let payload = pb::ClientHello {
        protocol: 1,
        device_name: "test".into(),
        ..Default::default()
    };
    let mut body = pb::Envelope::from(payload).encode_to_vec();
    body.extend_from_slice(&[0xa2, 0x06, 0x00]); // field 100, length-delimited, empty
    let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(&body);
    let mut decoder = EnvelopeDecoder::new();
    decoder.push(&bytes);
    let decoded = decoder.next().unwrap().unwrap();
    assert_eq!(
        decoded,
        pb::ClientHello {
            protocol: 1,
            device_name: "test".into(),
            ..Default::default()
        }
        .into()
    );

    let unknown_payload = [0xa2, 0x06, 0x00];
    let mut framed = (unknown_payload.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(&unknown_payload);
    decoder.push(&framed);
    // The first envelope was consumed; field 100 alone leaves msg unset.
    assert_eq!(decoder.next().unwrap().unwrap().msg, None);
}

#[test]
fn truncated_and_oversized_frames_are_distinct_and_errors_stick() {
    let mut decoder = EnvelopeDecoder::new();
    decoder.push(&[0, 0, 0]);
    assert_eq!(decoder.next().unwrap(), None);
    let mut decoder = EnvelopeDecoder::new();
    let header = (MAX_CONTROL_FRAME as u32 + 1).to_be_bytes();
    decoder.push(&[header[0]]);
    // The fourth byte completes the header and reports immediately.
    decoder.push(&header[1..]);
    let first = decoder.next().unwrap_err();
    assert!(matches!(first, ControlError::FrameTooLarge(_)));
    assert_eq!(decoder.next(), Err(first));
}

#[test]
fn malformed_protobuf_is_sticky() {
    let mut decoder = EnvelopeDecoder::new();
    decoder.push(&[0, 0, 0, 1, 0xff]);
    let first = decoder.next().unwrap_err();
    assert!(matches!(first, ControlError::Protobuf(_)));
    assert_eq!(decoder.next(), Err(first));
}
