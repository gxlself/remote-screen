use std::io::Cursor;
use std::net::UdpSocket;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rs_bench::clock::{estimate, probe_udp, serve_lines, serve_udp_socket, Sample};

fn sample_with_offset(offset: i64, delay: u64, t1: u64) -> Sample {
    let t2 = (t1 as i128 + delay as i128 + offset as i128) as u64;
    let t3 = t2;
    let t4 = (t3 as i128 - offset as i128 + delay as i128) as u64;
    Sample { t1, t2, t3, t4 }
}

fn xorshift64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn sample_math() {
    let sample = Sample {
        t1: 1000,
        t2: 6300,
        t3: 6400,
        t4: 1800,
    };
    assert_eq!(sample.rtt_us(), 700);
    assert_eq!(sample.offset_us(), 4950);

    let negative = Sample {
        t1: 100,
        t2: 80,
        t3: 81,
        t4: 103,
    };
    assert_eq!(negative.offset_us(), -21);
    let odd_negative = Sample {
        t1: 100,
        t2: 80,
        t3: 80,
        t4: 101,
    };
    assert_eq!(odd_negative.offset_us(), -20);
}

#[test]
fn estimate_recovers_offset() {
    let mut seed = 0x9e37_79b9_7f4a_7c15;
    let mut samples = Vec::with_capacity(200);
    for index in 0..200 {
        let outbound_jitter = xorshift64(&mut seed) % 2001;
        let inbound_jitter = xorshift64(&mut seed) % 2001;
        let t1 = 1_000_000 + index as u64 * 10_000;
        let outbound = 300 + outbound_jitter;
        let inbound = 300 + inbound_jitter;
        let t2 = t1 + outbound + 5000;
        let t3 = t2 + 10;
        let t4 = t3 - 5000 + inbound;
        samples.push(Sample { t1, t2, t3, t4 });
    }

    let result = estimate(&samples, 0).expect("200 valid samples should estimate");
    assert!((result.offset_us - 5000).abs() <= 250);
    assert!(result.rtt_us >= 600);
    assert_eq!(result.used, 20);
}

#[test]
fn estimate_drops_negative_and_skip() {
    let mut samples = (0..5)
        .map(|index| sample_with_offset(900_000, 10, 1000 + index * 100))
        .collect::<Vec<_>>();
    samples.push(Sample {
        t1: 10_000,
        t2: 10_000,
        t3: 11_000,
        t4: 10_000,
    });
    samples.extend([
        sample_with_offset(123, 20, 20_000),
        sample_with_offset(123, 21, 21_000),
        sample_with_offset(123, 22, 22_000),
    ]);

    let result = estimate(&samples, 5).expect("three valid samples remain");
    assert_eq!(result.offset_us, 123);
    assert_eq!(result.used, 3);
}

#[test]
fn estimate_too_few() {
    let samples = [
        sample_with_offset(100, 10, 1),
        sample_with_offset(100, 10, 2),
    ];
    assert!(estimate(&samples, 0).is_none());
}

#[test]
fn estimate_lower_median() {
    let mut samples = vec![
        sample_with_offset(10, 10, 1),
        sample_with_offset(20, 20, 2),
        sample_with_offset(30, 30, 3),
        sample_with_offset(40, 40, 4),
    ];
    samples.extend((0..27).map(|index| sample_with_offset(1000, 1000, 100 + index)));
    let result = estimate(&samples, 0).expect("at least three valid samples");
    assert_eq!(result.used, 4);
    assert_eq!(result.offset_us, 20);
}

#[test]
fn serve_lines_protocol() {
    let input = Cursor::new(b"P 1 50\nbad\nP 2 60\nQ\nP 3 70\n".to_vec());
    let mut output = Vec::new();
    let mut values = [100, 101, 200, 201].into_iter();
    let mut now = || values.next().expect("test clock values");
    let response_count = serve_lines(input, &mut output, &mut now).expect("serve lines");
    assert_eq!(response_count, 2);
    assert_eq!(
        String::from_utf8(output).expect("ASCII protocol"),
        "R 1 50 100 101\nE bad\nR 2 60 200 201\n"
    );
}

#[cfg(unix)]
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn shell_quote(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "\\\""))
}

#[test]
fn stdio_loopback_cli() {
    let executable = env!("CARGO_BIN_EXE_rs-bench");
    let command = format!("{} clock serve --stdio", shell_quote(executable));
    let output = Command::new(executable)
        .args([
            "clock",
            "probe",
            "--cmd",
            &command,
            "--count",
            "50",
            "--interval-ms",
            "1",
            "--json",
            "-",
        ])
        .output()
        .expect("run stdio loopback");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let line = output
        .stdout
        .split(|byte| *byte == b'\n')
        .rfind(|line| !line.is_empty())
        .expect("JSON output line");
    let json: serde_json::Value = serde_json::from_slice(line).expect("parse JSON output");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["method"], "stdio");
    assert_eq!(json["ok"], 50);
    let offset = json["offset_us"].as_i64().expect("offset integer");
    let rtt = json["rtt_us"].as_i64().expect("rtt integer");
    eprintln!("stdio loopback offset={offset} us rtt={rtt} us");
    assert!(offset.abs() <= 2000);
    assert!(rtt > 0 && rtt < 50_000);
}

#[test]
fn udp_loopback() {
    let server = UdpSocket::bind("127.0.0.1:0").expect("bind UDP server");
    let address = server.local_addr().expect("server address");
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let thread = thread::spawn(move || serve_udp_socket(server, server_stop));

    let samples = probe_udp(
        address,
        30,
        Duration::from_millis(1),
        Duration::from_millis(500),
    )
    .expect("UDP loopback probe");
    assert_eq!(samples.len(), 30);
    let result = estimate(&samples, 0).expect("UDP estimate");
    assert!(result.offset_us.abs() <= 2000);

    stop.store(true, Ordering::Relaxed);
    let response_count = thread
        .join()
        .expect("UDP server thread")
        .expect("UDP server");
    assert_eq!(response_count, 30);
}

#[test]
fn udp_ignores_garbage() {
    let server = UdpSocket::bind("127.0.0.1:0").expect("bind UDP server");
    let address = server.local_addr().expect("server address");
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let thread = thread::spawn(move || serve_udp_socket(server, server_stop));
    let client = UdpSocket::bind("127.0.0.1:0").expect("bind UDP client");
    client
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("client timeout");

    client.send_to(b"hello", address).expect("send garbage");
    let mut buffer = [0_u8; 2048];
    let error = client
        .recv_from(&mut buffer)
        .expect_err("garbage must not receive response");
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ));

    client
        .send_to(b"RSCK1 P 7 100", address)
        .expect("send valid request");
    let (length, _) = client.recv_from(&mut buffer).expect("valid response");
    let response = std::str::from_utf8(&buffer[..length]).expect("ASCII response");
    assert!(response.starts_with("RSCK1 R 7 100 "));

    stop.store(true, Ordering::Relaxed);
    thread
        .join()
        .expect("UDP server thread")
        .expect("UDP server");
}

#[cfg(unix)]
#[test]
fn probe_cmd_bad_command() {
    let executable = env!("CARGO_BIN_EXE_rs-bench");
    let output = Command::new(executable)
        .args(["clock", "probe", "--cmd", "false", "--count", "3"])
        .output()
        .expect("run bad clock command");
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
}

#[test]
fn cli_arg_errors() {
    let executable = env!("CARGO_BIN_EXE_rs-bench");
    let missing_transport = Command::new(executable)
        .args(["clock", "probe"])
        .output()
        .expect("run invalid probe arguments");
    assert_eq!(missing_transport.status.code(), Some(2));

    let conflicting_transport = Command::new(executable)
        .args(["clock", "serve", "--stdio", "--udp", "127.0.0.1:1"])
        .output()
        .expect("run conflicting serve arguments");
    assert_eq!(conflicting_transport.status.code(), Some(2));
}
