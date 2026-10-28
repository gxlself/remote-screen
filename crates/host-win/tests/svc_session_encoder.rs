use std::ffi::OsString;

use host_core::config::{HostConfig, VideoEncoder};
use host_win::{
    session_run::parse_args,
    svc_session::{configured_encoder, session_args},
};

fn parse_encoder(args: Vec<String>) -> Option<VideoEncoder> {
    parse_args(args.into_iter().map(OsString::from))
        .expect("generated session arguments must parse")
        .encoder
}

#[test]
fn session_args_cover_all_encoder_preferences() {
    let cases = [
        (VideoEncoder::Auto, vec!["--session", "7"]),
        (
            VideoEncoder::Nvenc,
            vec!["--session", "7", "--encoder", "nvenc"],
        ),
        (
            VideoEncoder::Amf,
            vec!["--session", "7", "--encoder", "amf"],
        ),
        (
            VideoEncoder::Qsv,
            vec!["--session", "7", "--encoder", "qsv"],
        ),
        (VideoEncoder::Mf, vec!["--session", "7", "--encoder", "mf"]),
        (
            VideoEncoder::OpenH264,
            vec!["--session", "7", "--encoder", "openh264"],
        ),
    ];

    for (encoder, expected) in cases {
        let args = session_args(7, encoder.clone());
        assert_eq!(args, expected);
        assert_eq!(
            parse_encoder(args),
            (encoder != VideoEncoder::Auto).then_some(encoder)
        );
    }
}

#[test]
fn configured_encoder_reads_the_current_store_value() {
    let path = std::path::PathBuf::from("target").join(format!(
        "rs-svc-session-encoder-{}.toml",
        std::process::id()
    ));
    let store = host_core::config::ConfigStore::new(&path, HostConfig::default());
    assert_eq!(configured_encoder(&store), VideoEncoder::Auto);

    let updated = store
        .update(&serde_json::json!({"video": {"encoder": "qsv"}}))
        .expect("valid encoder update");
    assert_eq!(updated.video.encoder, VideoEncoder::Qsv);
    assert_eq!(configured_encoder(&store), VideoEncoder::Qsv);

    let _ = std::fs::remove_file(path);
}
