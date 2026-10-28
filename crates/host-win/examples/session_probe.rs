//! Fake service for manual Windows session-process checks. Run elevated in the
//! interactive session (or as SYSTEM in that session). Never prints media,
//! clipboard contents or encoded input messages.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    if let Err(error) = probe::main() {
        eprintln!("session probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod probe {
    use std::{
        collections::HashSet,
        ffi::OsString,
        io,
        path::PathBuf,
        process::{Child, Command},
        time::Duration,
    };

    use host_win::{
        pipe::PipeServer,
        pipe_proto::{
            check_hello, input_to_b64, read_frame, write_frame, Ctl, DisplayWire, PipeFrame,
            WatchKind, PIPE_PROTO_VERSION,
        },
        session_run::{CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT},
        wts,
    };
    use rsp_core::{hid::InputMsg, pb};
    use tokio::{
        io::{split, AsyncWrite},
        net::windows::named_pipe::NamedPipeServer,
        sync::mpsc,
        time::{self, Instant, MissedTickBehavior},
    };

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    // Only this manual probe permits administrators; the real service keeps
    // using winproc_model::SESSION_PIPE_SDDL (SYSTEM only).
    const PROBE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)";
    const USAGE: &str = "Usage: session_probe --exe <rs-host-session.exe> [--seconds <n>] [--capture <display|primary>] [--fps <n>] [--encode] [--cursor] [--activity] [--clipboard] [--inject-test] [--esc] [--hold-shift] [--lock]";
    const CAPTURE_REQ: u32 = 1;
    const ENCODER_REQ: u32 = 2;
    const LOCK_REQ: u32 = 3;

    enum CaptureDisplay {
        Primary,
        Id(u32),
    }

    struct Args {
        exe: PathBuf,
        seconds: u32,
        capture: Option<CaptureDisplay>,
        fps: u32,
        encode: bool,
        cursor: bool,
        activity: bool,
        clipboard: bool,
        inject_test: bool,
        esc: bool,
        hold_shift: bool,
        lock: bool,
    }

    impl Args {
        fn parse() -> Result<Option<Self>> {
            let mut args = Self {
                exe: PathBuf::new(),
                seconds: 20,
                capture: None,
                fps: 30,
                encode: false,
                cursor: false,
                activity: false,
                clipboard: false,
                inject_test: false,
                esc: false,
                hold_shift: false,
                lock: false,
            };
            let mut words = std::env::args_os().skip(1);
            while let Some(word) = words.next() {
                let flag = word.to_str().ok_or("argument is not valid Unicode")?;
                match flag {
                    "--help" | "-h" => return Ok(None),
                    "--exe" => args.exe = value(&mut words, flag)?.into(),
                    "--seconds" => args.seconds = number(value(&mut words, flag)?, flag)?,
                    "--fps" => args.fps = number(value(&mut words, flag)?, flag)?,
                    "--capture" => {
                        let display = value(&mut words, flag)?;
                        args.capture = Some(if display == "primary" {
                            CaptureDisplay::Primary
                        } else {
                            CaptureDisplay::Id(
                                display.to_str().ok_or("invalid display ID")?.parse()?,
                            )
                        });
                    }
                    "--encode" => args.encode = true,
                    "--cursor" => args.cursor = true,
                    "--activity" => args.activity = true,
                    "--clipboard" => args.clipboard = true,
                    "--inject-test" => args.inject_test = true,
                    "--esc" => args.esc = true,
                    "--hold-shift" => args.hold_shift = true,
                    "--lock" => args.lock = true,
                    _ => return Err(format!("unknown argument {flag}\n{USAGE}").into()),
                }
            }
            if args.exe.as_os_str().is_empty() {
                return Err(format!("--exe is required\n{USAGE}").into());
            }
            Ok(Some(args))
        }
    }

    fn value(words: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<OsString> {
        words
            .next()
            .ok_or_else(|| format!("missing value for {flag}").into())
    }

    fn number(value: OsString, flag: &str) -> Result<u32> {
        value
            .to_str()
            .and_then(|value| value.parse().ok())
            .filter(|value| *value != 0)
            .ok_or_else(|| format!("{flag} must be a positive integer").into())
    }

    pub fn main() -> Result<()> {
        let Some(args) = Args::parse()? else {
            println!("{USAGE}");
            return Ok(());
        };
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let result = rt.block_on(run(args));
        rt.shutdown_timeout(Duration::from_secs(2));
        result
    }

    async fn run(args: Args) -> Result<()> {
        let pid = std::process::id();
        let session = wts::process_session(pid).ok_or("cannot determine probe session")?;
        if session == 0 {
            return Err("run the probe in an interactive session, not session 0".into());
        }
        let name = format!(r"\\.\pipe\RemoteHost.session.probe-{pid}");
        let mut server = PipeServer::new(&name, PROBE_SDDL)?;
        let mut child = Command::new(&args.exe)
            .args([
                "--session",
                &session.to_string(),
                "--pipe",
                &name,
                "--log-level",
                "debug",
            ])
            .spawn()?;
        println!("session process pid={} session={session}", child.id());

        // Every path after spawning drops both pipe instances and reaps exactly
        // this child, including failed accept/handshake/script operations.
        let result = async {
            let (mut client, client_pid) =
                time::timeout(CONNECT_TIMEOUT, server.accept()).await??;
            if client_pid != child.id() {
                return Err("pipe client PID differs from spawned child".into());
            }
            time::timeout(HANDSHAKE_TIMEOUT, async {
                let Some(PipeFrame::Ctl(hello)) = read_frame(&mut client).await? else {
                    return Err("expected Hello control frame".into());
                };
                let (hello_session, hello_pid) = check_hello(&hello)?;
                if hello_session != session || hello_pid != child.id() {
                    return Err("Hello identity differs from spawned child".into());
                }
                println!("Hello: session={hello_session} pid={hello_pid}");
                send(
                    &mut client,
                    Ctl::Welcome {
                        version: PIPE_PROTO_VERSION,
                    },
                )
                .await
            })
            .await??;
            exercise(client, &args).await
        }
        .await;
        drop(server);
        let reaped = reap(&mut child).await;
        result?;
        reaped
    }

    async fn reap(child: &mut Child) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait()? {
                println!("session process exit code: {:?}", status.code());
                return Ok(());
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "session process did not exit within 5 s; killing pid={}",
                    child.id()
                );
                // A process that exits between try_wait and kill is still reaped.
                if let Err(error) = child.kill() {
                    if child.try_wait()?.is_none() {
                        return Err(error.into());
                    }
                }
                let status = child.wait()?;
                println!("session process exit code: {:?} (forced)", status.code());
                return Err("session process required forced termination".into());
            }
            time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn send(writer: &mut (impl AsyncWrite + Unpin), ctl: Ctl) -> Result<()> {
        write_frame(writer, &PipeFrame::Ctl(ctl)).await?;
        Ok(())
    }

    async fn inject(
        writer: &mut (impl AsyncWrite + Unpin),
        display_id: u32,
        msg: InputMsg,
    ) -> Result<()> {
        send(
            writer,
            Ctl::Inject {
                display_id,
                input: input_to_b64(&msg)?,
            },
        )
        .await
    }

    async fn key(
        writer: &mut (impl AsyncWrite + Unpin),
        display_id: u32,
        usage: u16,
        down: bool,
    ) -> Result<()> {
        inject(
            writer,
            display_id,
            InputMsg::Key {
                usage,
                down,
                mods: Default::default(),
            },
        )
        .await
    }

    #[derive(Default)]
    struct Counts {
        frames: u64,
        video: u64,
        cursor: u64,
        activity: u64,
        clipboard: u64,
    }

    #[derive(Default)]
    struct Script {
        primary: Option<DisplayWire>,
        displays_received: bool,
        started: bool,
        pong_received: bool,
        encoder_ready: bool,
        square_step: u32,
        clipboard_reqs: HashSet<u32>,
        next_req: u32,
        counts: Counts,
    }

    impl Script {
        async fn start(
            &mut self,
            writer: &mut (impl AsyncWrite + Unpin),
            args: &Args,
        ) -> Result<()> {
            if args.encode {
                send(
                    writer,
                    Ctl::EncoderOpen {
                        req: ENCODER_REQ,
                        enc: 1,
                        codec: pb::Codec::Hevc as i32,
                        width: 1920,
                        height: 1080,
                        fps: args.fps,
                        bitrate_kbps: 8000,
                    },
                )
                .await?;
            }
            if let Some(display) = &args.capture {
                let display = match display {
                    CaptureDisplay::Primary => {
                        self.primary.as_ref().ok_or("no primary display")?.id
                    }
                    CaptureDisplay::Id(id) => *id,
                };
                send(
                    writer,
                    Ctl::CaptureStart {
                        req: CAPTURE_REQ,
                        cap: 1,
                        display,
                        fps: args.fps,
                        max_width: 0,
                        max_height: 0,
                        region: None,
                    },
                )
                .await?;
            }
            for (enabled, what) in [
                (args.cursor, WatchKind::Cursor),
                (args.activity, WatchKind::Activity),
                (args.clipboard, WatchKind::Clipboard),
            ] {
                if enabled {
                    send(writer, Ctl::Watch { what }).await?;
                }
            }
            if args.inject_test || args.esc || args.hold_shift {
                let display = self
                    .primary
                    .as_ref()
                    .ok_or("no primary display for injection")?;
                // When combined, finish the square's ReleaseAll before holding Shift.
                if args.hold_shift && !args.inject_test {
                    key(writer, display.id, 0xE1, true).await?;
                }
            }
            if args.lock {
                send(
                    writer,
                    Ctl::Action {
                        req: LOCK_REQ,
                        kind: pb::session_action::Kind::LockScreen as i32,
                    },
                )
                .await?;
            }
            self.started = true;
            Ok(())
        }

        async fn square(
            &mut self,
            writer: &mut (impl AsyncWrite + Unpin),
            args: &Args,
        ) -> Result<()> {
            let Some(display) = &self.primary else {
                return Ok(());
            };
            if !self.started || !args.inject_test || self.square_step > 40 {
                return Ok(());
            }
            let fraction = (self.square_step % 10) as f32 / 10.0;
            let (x, y) = match self.square_step / 10 {
                0 => (-32.0 + 64.0 * fraction, -32.0),
                1 => (32.0, -32.0 + 64.0 * fraction),
                2 => (32.0 - 64.0 * fraction, 32.0),
                3 => (-32.0, 32.0 - 64.0 * fraction),
                _ => (-32.0, -32.0),
            };
            // RSP coordinates are normalized to the selected display's 0..1 range.
            let x = (0.5 + x / display.width_px.max(1) as f32).clamp(0.0, 1.0);
            let y = (0.5 + y / display.height_px.max(1) as f32).clamp(0.0, 1.0);
            inject(writer, display.id, InputMsg::MouseMove { x, y }).await?;
            self.square_step += 1;
            if self.square_step == 41 {
                send(writer, Ctl::ReleaseAll).await?;
                println!("inject-test: square complete, ReleaseAll sent");
                if args.hold_shift {
                    key(writer, display.id, 0xE1, true).await?;
                }
            }
            Ok(())
        }

        async fn receive(
            &mut self,
            frame: PipeFrame,
            writer: &mut (impl AsyncWrite + Unpin),
            ping: Option<Instant>,
        ) -> Result<()> {
            match frame {
                PipeFrame::Ctl(Ctl::Pong { n: 1 }) => {
                    if let Some(ping) = ping {
                        println!("Ping RTT: {:.3} ms", ping.elapsed().as_secs_f64() * 1000.0);
                    }
                    self.pong_received = true;
                }
                PipeFrame::Ctl(Ctl::Displays { list }) => {
                    println!("Displays: count={}", list.len());
                    for display in &list {
                        println!("  {display:?}");
                    }
                    self.primary = list.into_iter().find(|display| display.primary);
                    self.displays_received = true;
                }
                PipeFrame::Ctl(Ctl::Desktop { name }) => println!("Desktop{{{name}}}"),
                PipeFrame::Ctl(Ctl::Done { req, error }) => {
                    println!("Done{{req: {req}, error: {error:?}}}");
                    if req == ENCODER_REQ {
                        self.encoder_ready = error.is_none();
                    }
                    if self.clipboard_reqs.remove(&req) && error.is_none() {
                        println!("clipboard: none");
                    }
                }
                PipeFrame::Ctl(Ctl::Frame { cap, seq, .. }) => {
                    self.counts.frames += 1;
                    if self.encoder_ready {
                        send(
                            writer,
                            Ctl::Encode {
                                enc: 1,
                                cap,
                                seq,
                                force_keyframe: false,
                                quality_boost: false,
                            },
                        )
                        .await?;
                    }
                }
                PipeFrame::Video(_) => self.counts.video += 1,
                PipeFrame::Ctl(Ctl::CaptureEnded { cap, error }) => {
                    println!("CaptureEnded{{cap: {cap}, error: {error:?}}}");
                }
                PipeFrame::Ctl(Ctl::EncoderFailed { enc, error }) => {
                    self.encoder_ready = false;
                    println!("EncoderFailed{{enc: {enc}, error: {error:?}}}");
                }
                PipeFrame::Ctl(
                    Ctl::CursorShape { .. }
                    | Ctl::CursorUse { .. }
                    | Ctl::CursorHidden { .. }
                    | Ctl::CursorPos { .. },
                ) => self.counts.cursor += 1,
                PipeFrame::Ctl(Ctl::Activity { active }) => {
                    self.counts.activity += 1;
                    println!("Activity{{{active}}}");
                }
                PipeFrame::Ctl(Ctl::ClipboardChanged) => {
                    self.counts.clipboard += 1;
                    println!("ClipboardChanged");
                    self.next_req = self
                        .next_req
                        .checked_add(1)
                        .ok_or("request IDs exhausted")?;
                    let req = self.next_req;
                    self.clipboard_reqs.insert(req);
                    send(writer, Ctl::ClipboardGet { req }).await?;
                }
                PipeFrame::Blob(blob) => {
                    self.clipboard_reqs.remove(&blob.req);
                    println!("clipboard: kind={:?} length={}", blob.kind, blob.data.len());
                }
                _ => {}
            }
            Ok(())
        }
    }

    async fn exercise(client: NamedPipeServer, args: &Args) -> Result<()> {
        let (mut reader, mut writer) = split(client);
        let (tx, mut rx) = mpsc::channel(64);
        // read_frame is not cancel-safe. Timer ticks select only over complete
        // frames; the dedicated reader is aborted only when closing the pipe.
        let reader_task = tokio::spawn(async move {
            loop {
                let frame = read_frame(&mut reader).await;
                let terminal = !matches!(&frame, Ok(Some(_)));
                if tx.send(frame).await.is_err() || terminal {
                    break;
                }
            }
        });
        let result = time::timeout(Duration::from_secs(u64::from(args.seconds)), async {
            let started = Instant::now();
            let mut ping = None;
            let mut script = Script { next_req: 100, ..Default::default() };
            let mut report = time::interval_at(started + Duration::from_secs(1), Duration::from_secs(1));
            let mut escape = time::interval_at(started + Duration::from_secs(2), Duration::from_secs(2));
            let mut movement = time::interval(Duration::from_millis(40));
            report.set_missed_tick_behavior(MissedTickBehavior::Skip);
            escape.set_missed_tick_behavior(MissedTickBehavior::Skip);
            movement.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    frame = rx.recv() => {
                        let Some(frame) = frame else {
                            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "pipe reader stopped").into());
                        };
                        let Some(frame) = frame? else {
                            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "session closed the pipe early").into());
                        };
                        script.receive(frame, &mut writer, ping).await?;
                        // Initial display enumeration can block on Windows drivers;
                        // measure Ping only after the session has finished starting.
                        if script.displays_received && ping.is_none() {
                            ping = Some(Instant::now());
                            send(&mut writer, Ctl::Ping { n: 1 }).await?;
                        }
                        // Keep script setup out of the Ping round-trip measurement.
                        if !script.started && script.pong_received && script.displays_received {
                            script.start(&mut writer, args).await?;
                        }
                    }
                    _ = report.tick() => {
                        let counts = std::mem::take(&mut script.counts);
                        println!("per second: Frame={} Video={} cursor={} activity={} clipboard={}",
                            counts.frames, counts.video, counts.cursor, counts.activity, counts.clipboard);
                    }
                    _ = movement.tick(), if args.inject_test && script.square_step <= 40 => {
                        script.square(&mut writer, args).await?;
                    }
                    _ = escape.tick(), if args.esc && script.started => {
                        if let Some(display) = &script.primary {
                            key(&mut writer, display.id, 0x29, true).await?;
                            key(&mut writer, display.id, 0x29, false).await?;
                            println!("Esc: press/release sent");
                        }
                    }
                }
            }
        }).await;

        reader_task.abort();
        let _ = reader_task.await;
        drop(writer);
        match result {
            Err(_) => {
                println!("probe duration elapsed; service pipe closed");
                Ok(())
            }
            Ok(result) => result,
        }
    }
}
