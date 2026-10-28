#![no_main]

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use rsp_core::pairing::{AppPairing, HostPairing};
use rsp_core::pb::{self, pair_result::Status};

const DEVICE_FP: [u8; 32] = [0x11; 32];
const HOST_FP: [u8; 32] = [0x22; 32];

fn random_spake(input: &mut Unstructured<'_>) -> pb::PairSpake {
    let len = input.arbitrary::<u16>().unwrap_or(0).min(256) as usize;
    let msg = input.bytes(len).map(<[u8]>::to_vec).unwrap_or_default();
    pb::PairSpake { msg }
}

fn random_confirm(input: &mut Unstructured<'_>) -> pb::PairConfirm {
    let len = input.arbitrary::<u16>().unwrap_or(0).min(64) as usize;
    let mac = input.bytes(len).map(<[u8]>::to_vec).unwrap_or_default();
    pb::PairConfirm { mac }
}

fuzz_target!(|data: &[u8]| {
    let mut input = Unstructured::new(data);
    let app_code = if input.arbitrary::<bool>().unwrap_or(false) {
        "012345"
    } else {
        "654321"
    };
    let host_code = if input.arbitrary::<bool>().unwrap_or(false) {
        "012345"
    } else {
        "654321"
    };
    let codes_match = app_code == host_code;

    let mut app = match AppPairing::new(
        app_code,
        DEVICE_FP,
        HOST_FP,
        "fuzz phone",
        pb::Platform::Ios,
    ) {
        Ok(app) => app,
        Err(_) => return,
    };
    let mut host = match HostPairing::new(host_code, HOST_FP, DEVICE_FP) {
        Ok(host) => host,
        Err(_) => return,
    };

    let mut pair_start = None;
    let mut host_spake = None;
    let mut app_spake = None;
    let mut app_confirm = None;
    let mut host_confirm = None;
    let operations = input.arbitrary::<u8>().unwrap_or(12).min(32);

    for step in 0..operations {
        if !codes_match {
            assert!(!app.is_verified());
            assert!(!host.is_verified());
        }

        let operation = input.arbitrary::<u8>().unwrap_or(step) % 7;
        match operation {
            0 => pair_start = Some(app.start()),
            1 => {
                if let Some(start) = pair_start.clone() {
                    host_spake = Some(host.on_start(start));
                }
            }
            2 => {
                let message = if input.arbitrary::<bool>().unwrap_or(true) {
                    host_spake.clone().unwrap_or_default()
                } else {
                    random_spake(&mut input)
                };
                if let Ok((spake, confirm)) = app.on_spake(message) {
                    app_spake = Some(spake);
                    app_confirm = Some(confirm);
                }
            }
            3 => {
                let message = if input.arbitrary::<bool>().unwrap_or(true) {
                    app_spake.clone().unwrap_or_default()
                } else {
                    random_spake(&mut input)
                };
                let _ = host.on_spake(message);
            }
            4 => {
                let message = if input.arbitrary::<bool>().unwrap_or(true) {
                    app_confirm.clone().unwrap_or_default()
                } else {
                    random_confirm(&mut input)
                };
                if let Ok(confirm) = host.on_confirm(message) {
                    host_confirm = Some(confirm);
                }
            }
            5 => {
                let message = if input.arbitrary::<bool>().unwrap_or(true) {
                    host_confirm.clone().unwrap_or_default()
                } else {
                    random_confirm(&mut input)
                };
                let _ = app.on_confirm(message);
            }
            _ => {
                let status = input.arbitrary::<u8>().unwrap_or(Status::Denied as u8) % 9;
                let _ = app.on_result(pb::PairResult {
                    status: i32::from(status),
                    host_id: "host".into(),
                    host_name: "fuzz host".into(),
                    attempts_left: input.arbitrary::<u8>().unwrap_or(0) as u32,
                    ..Default::default()
                });
            }
        }
    }

    if !codes_match {
        assert!(
            !app.is_verified(),
            "App verified with a code the Host did not use"
        );
        assert!(
            !host.is_verified(),
            "Host verified with a code the App did not use"
        );
    }
});
