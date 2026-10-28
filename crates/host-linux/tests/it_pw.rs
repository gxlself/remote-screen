#![cfg(feature = "desktop-it")]

use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use host_linux::pw::{Direction, PwLib, PwStream, Remote, StreamCtl, StreamHandler, StreamSpec};
use host_linux::pw_ffi::{self, stream_flag};
use host_linux::spa_pod::{self, MetaSize};

fn assert_pipewire_layer() {
    assert_eq!(std::env::var("RS_IT_LAYER").as_deref(), Ok("pipewire"));
}

fn video_params(formats: &[u32]) -> Vec<Vec<u8>> {
    vec![
        spa_pod::video_enum_format(formats, (64, 48), (64, 48), 30),
        spa_pod::object(
            spa_pod::object_type::PARAM_BUFFERS,
            spa_pod::param::BUFFERS,
            &[
                (spa_pod::buffers_key::BUFFERS, spa_pod::Value::Int(4)),
                (spa_pod::buffers_key::BLOCKS, spa_pod::Value::Int(1)),
                (spa_pod::buffers_key::SIZE, spa_pod::Value::Int(64 * 48 * 4)),
                (spa_pod::buffers_key::STRIDE, spa_pod::Value::Int(256)),
                (
                    spa_pod::buffers_key::DATA_TYPE,
                    spa_pod::Value::Choice {
                        kind: spa_pod::choice::FLAGS,
                        values: vec![spa_pod::Value::Int(pw_ffi::data_type::MEM_FD as i32)],
                    },
                ),
            ],
        ),
        spa_pod::meta_param(spa_pod::meta_type::HEADER, MetaSize::Fixed(32)),
        spa_pod::meta_param(spa_pod::meta_type::VIDEO_DAMAGE, MetaSize::Fixed(64)),
    ]
}

#[derive(Default)]
struct VersionHandler {
    state: Option<mpsc::Sender<i32>>,
}

impl StreamHandler for VersionHandler {
    fn state_changed(&mut self, _old: i32, new: i32, _error: Option<&str>) {
        if let Some(tx) = self.state.take() {
            let _ = tx.send(new);
        }
    }
}

#[test]
fn load_and_version() {
    assert_pipewire_layer();
    let lib = PwLib::load().expect("PipeWire should be available in the desktop layer");
    let version = host_linux::pw_ffi::parse_version(lib.version()).expect("valid version");
    assert!(version >= host_linux::pw_ffi::MIN_VERSION);
}

struct Producer {
    frames: Arc<Mutex<u64>>,
}

impl StreamHandler for Producer {
    fn process(&mut self, ctl: &mut StreamCtl<'_>) {
        let Some(mut buf) = ctl.dequeue() else { return };
        let mut frame = self.frames.lock().expect("frame lock");
        let value = (*frame & 0xff) as u8;
        if let Some(bytes) = buf.data_mut(0) {
            for pixel in bytes.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[value, value, value, 0xff]);
            }
            let size = bytes.len() as u32;
            buf.set_chunk(0, 0, size, 256, 0);
        }
        if let Some(header) = buf.meta_mut(spa_pod::meta_type::HEADER) {
            header.fill(0);
            header[24..32].copy_from_slice(&frame.to_le_bytes());
        }
        if let Some(damage) = buf.meta_mut(spa_pod::meta_type::VIDEO_DAMAGE) {
            damage.fill(0);
            damage[..16].copy_from_slice(&1_u32.to_le_bytes());
            damage[4..8].copy_from_slice(&2_u32.to_le_bytes());
            damage[8..12].copy_from_slice(&3_u32.to_le_bytes());
            damage[12..16].copy_from_slice(&4_u32.to_le_bytes());
        }
        *frame = frame.wrapping_add(1);
        ctl.queue(buf);
    }
}

struct Consumer {
    format: mpsc::Sender<(u32, u32, u32)>,
    frames: mpsc::Sender<(u64, Vec<u8>, Vec<spa_pod::Region>)>,
}

impl StreamHandler for Consumer {
    fn param_changed(&mut self, _ctl: &mut StreamCtl<'_>, id: u32, pod: Option<&[u8]>) {
        if id == spa_pod::param::FORMAT {
            if let Some(pod) = pod {
                if let Ok(format) = spa_pod::parse_video_format(pod) {
                    let _ = self
                        .format
                        .send((format.format, format.width, format.height));
                }
            }
        }
    }

    fn process(&mut self, ctl: &mut StreamCtl<'_>) {
        let Some(buf) = ctl.dequeue() else { return };
        let frame = buf
            .meta(spa_pod::meta_type::HEADER)
            .and_then(spa_pod::parse_meta_header)
            .map(|header| header.seq)
            .unwrap_or_default();
        let bytes = buf
            .data(0)
            .and_then(|data| data.valid().map(ToOwned::to_owned))
            .unwrap_or_default();
        let damage = buf
            .meta(spa_pod::meta_type::VIDEO_DAMAGE)
            .map(spa_pod::parse_damage)
            .unwrap_or_default();
        let _ = self.frames.send((frame, bytes, damage));
        ctl.queue(buf);
    }
}

fn producer(lib: Arc<PwLib>, formats: &[u32]) -> PwStream {
    PwStream::start(
        lib,
        Remote::Default,
        StreamSpec {
            name: "pw-test-producer".into(),
            props: vec![("media.type".into(), "Video".into())],
            direction: Direction::Output,
            target: None,
            flags: stream_flag::DRIVER | stream_flag::MAP_BUFFERS | stream_flag::ALLOC_BUFFERS,
            params: video_params(formats),
        },
        Box::new(Producer {
            frames: Arc::new(Mutex::new(0)),
        }),
    )
    .expect("producer stream")
}

#[test]
fn video_roundtrip() {
    assert_pipewire_layer();
    let lib = PwLib::load().expect("PipeWire");
    let producer = producer(
        lib.clone(),
        &[spa_pod::video_format::BGRX, spa_pod::video_format::BGRA],
    );
    let target = producer.node_id();
    let (format_tx, format_rx) = mpsc::channel();
    let (frame_tx, frame_rx) = mpsc::channel();
    let consumer = PwStream::start(
        lib,
        Remote::Default,
        StreamSpec {
            name: "pw-test-consumer".into(),
            props: vec![("media.type".into(), "Video".into())],
            direction: Direction::Input,
            target: Some(target),
            flags: stream_flag::AUTOCONNECT | stream_flag::MAP_BUFFERS,
            params: video_params(&[
                spa_pod::video_format::BGRX,
                spa_pod::video_format::BGRA,
                spa_pod::video_format::RGBX,
                spa_pod::video_format::RGBA,
            ]),
        },
        Box::new(Consumer {
            format: format_tx,
            frames: frame_tx,
        }),
    )
    .expect("consumer stream");
    for _ in 0..8 {
        producer.trigger().expect("trigger producer");
        thread::sleep(Duration::from_millis(20));
    }
    let (_, width, height) = format_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("format");
    assert_eq!((width, height), (64, 48));
    let mut previous = None;
    for _ in 0..3 {
        let (seq, bytes, damage) = frame_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("frame");
        assert_eq!(bytes.len(), 64 * 48 * 4);
        assert_eq!(bytes[0], seq as u8);
        assert!(previous.is_none_or(|old| seq > old));
        assert_eq!(
            damage.first(),
            Some(&spa_pod::Region {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            })
        );
        previous = Some(seq);
    }
    drop(consumer);
    drop(producer);
}

#[test]
fn rgbx_negotiation() {
    assert_pipewire_layer();
    let lib = PwLib::load().expect("PipeWire");
    let producer = producer(lib.clone(), &[spa_pod::video_format::RGBX]);
    let (format_tx, format_rx) = mpsc::channel();
    let (frame_tx, _frame_rx) = mpsc::channel();
    let _consumer = PwStream::start(
        lib,
        Remote::Default,
        StreamSpec {
            name: "pw-test-rgbx-consumer".into(),
            props: vec![("media.type".into(), "Video".into())],
            direction: Direction::Input,
            target: Some(producer.node_id()),
            flags: stream_flag::AUTOCONNECT | stream_flag::MAP_BUFFERS,
            params: video_params(&[spa_pod::video_format::RGBX]),
        },
        Box::new(Consumer {
            format: format_tx,
            frames: frame_tx,
        }),
    )
    .expect("consumer stream");
    producer.trigger().expect("trigger producer");
    let (format, _, _) = format_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("format");
    assert_eq!(format, spa_pod::video_format::RGBX);
}

#[test]
fn drop_promptly() {
    assert_pipewire_layer();
    let lib = PwLib::load().expect("PipeWire");
    let producer = producer(lib.clone(), &[spa_pod::video_format::BGRX]);
    let consumer = PwStream::start(
        lib,
        Remote::Default,
        StreamSpec {
            name: "pw-test-drop-consumer".into(),
            props: vec![("media.type".into(), "Video".into())],
            direction: Direction::Input,
            target: Some(producer.node_id()),
            flags: stream_flag::AUTOCONNECT | stream_flag::MAP_BUFFERS,
            params: video_params(&[spa_pod::video_format::BGRX]),
        },
        Box::new(Consumer {
            format: mpsc::channel().0,
            frames: mpsc::channel().0,
        }),
    )
    .expect("consumer stream");
    let started = Instant::now();
    drop(consumer);
    assert!(started.elapsed() < Duration::from_secs(1));
    let started = Instant::now();
    drop(producer);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn bad_remote_fails() {
    assert_pipewire_layer();
    let lib = PwLib::load().expect("PipeWire");
    let file = std::fs::File::open("/dev/null").expect("/dev/null");
    let fd = std::os::fd::OwnedFd::from(file);
    let (state_tx, state_rx) = mpsc::channel();
    let result = PwStream::start(
        lib,
        Remote::Fd(fd),
        StreamSpec {
            name: "pw-test-bad-remote".into(),
            props: Vec::new(),
            direction: Direction::Input,
            target: None,
            flags: stream_flag::AUTOCONNECT,
            params: Vec::new(),
        },
        Box::new(VersionHandler {
            state: Some(state_tx),
        }),
    );
    match result {
        Err(_) => {}
        Ok(stream) => {
            assert_eq!(
                state_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("error state"),
                pw_ffi::state::ERROR
            );
            drop(stream);
        }
    }
}
