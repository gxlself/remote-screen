use std::env;
use std::time::Duration;

use host_core::platform::{CursorUpdate, Platform};
use host_mac::MacPlatform;
use tokio::sync::mpsc;

use super::input::{self, Action, Combo};

#[derive(Default)]
struct Args {
    seconds: u64,
    cursor: bool,
    clipboard: bool,
    displays: bool,
    activity: bool,
    console: bool,
    perms: bool,
    keep_awake: Option<u64>,
    wol: Option<(String, String)>,
    nics: bool,
    keystore_file: bool,
    kb_profile: bool,
    input: Vec<Action>,
}

fn parse_args() -> Args {
    let mut args = Args {
        seconds: 10,
        ..Args::default()
    };
    let mut values = env::args().skip(1);
    while let Some(arg) = values.next() {
        match arg.as_str() {
            "--seconds" => {
                args.seconds = values
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(10);
            }
            "--cursor" => args.cursor = true,
            "--clipboard" => args.clipboard = true,
            "--displays" => args.displays = true,
            "--activity" => args.activity = true,
            "--console" => args.console = true,
            "--perms" => args.perms = true,
            "--keep-awake" => {
                args.keep_awake = values.next().and_then(|value| value.parse().ok());
            }
            "--wol" => {
                let mac = values.next();
                let nic = values.next();
                if let (Some(mac), Some(nic)) = (mac, nic) {
                    args.wol = Some((mac, nic));
                } else {
                    eprintln!("--wol requires <mac> <interface>");
                }
            }
            "--nics" => args.nics = true,
            "--keystore-file" => args.keystore_file = true,
            "--kb-profile" => args.kb_profile = true,
            "--inject-text" => args.input.push(Action::Text(required_value(&mut values))),
            "--inject-combo" => {
                let value = required_value(&mut values);
                let Some(combo) = Combo::parse(&value) else {
                    eprintln!("combo must be cmd-tab, cmd-space, mission-control, app-windows, lock or screenshot");
                    std::process::exit(2);
                };
                args.input.push(Action::Combo(combo));
            }
            "--inject-keys" => {
                let value = required_value(&mut values);
                let Some(keys) = input::parse_keys(&value) else {
                    eprintln!("--inject-keys requires comma-separated hexadecimal HID usages");
                    std::process::exit(2);
                };
                args.input.push(Action::Keys(keys));
            }
            "--inject-scroll-demo" => args.input.push(Action::ScrollDemo),
            "--inject-scroll-cut" => args.input.push(Action::ScrollCut),
            "--inject-button" => {
                let value = required_value(&mut values);
                let button = match value.as_str() {
                    "back" => 3,
                    "forward" => 4,
                    _ => {
                        eprintln!("--inject-button requires back or forward");
                        std::process::exit(2);
                    }
                };
                args.input.push(Action::Button(button));
            }
            "--help" | "-h" => {
                println!("usage: platform_probe --seconds 10 [--cursor] [--clipboard] [--displays] [--activity] [--console] [--perms] [--keep-awake SECONDS] [--wol MAC INTERFACE] [--nics] [--keystore-file]");
                println!("encoder: --encode-bench|--encode-step OLD:NEW|--refine [--codec hevc|h264] [--pattern smooth|noise] [--size 1920x1080] [--fps 60] [--kbps 8000] [--seconds 5] [--at 2]");
                println!("input: [--kb-profile] [--inject-text TEXT] [--inject-combo cmd-tab|cmd-space|mission-control|app-windows|lock|screenshot] [--inject-keys 35,64,87,89] [--inject-scroll-demo] [--inject-scroll-cut] [--inject-button back|forward]");
                std::process::exit(0);
            }
            _ => eprintln!("ignoring unknown argument: {arg}"),
        }
    }
    args
}

fn required_value(values: &mut impl Iterator<Item = String>) -> String {
    match values.next() {
        Some(value) => value,
        None => {
            eprintln!("missing input probe argument; see --help");
            std::process::exit(2);
        }
    }
}

fn short_hash(hash: &[u8; 32]) -> String {
    hash[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

#[tokio::main]
pub async fn main() {
    let args = parse_args();
    if args.kb_profile {
        input::print_profile();
    }
    if args.keystore_file {
        // This environment variable is intentionally process-local and is the
        // documented development switch for the file backend.
        // SAFETY: set before creating MacPlatform and before any worker starts.
        unsafe { env::set_var("RS_KEYSTORE", "file") };
    }
    let platform = MacPlatform::new();

    if args.console {
        println!("console: {:?}", platform.console_state());
    }
    if args.perms {
        println!("permissions: {:?}", platform.permissions());
    }

    if args.nics {
        for nic in platform.nics() {
            let mac = nic.mac.map_or_else(
                || "-".to_owned(),
                |bytes| {
                    bytes
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(":")
                },
            );
            println!(
                "nic {} addresses={:?} networks={:?} mac={} tailnet={}",
                nic.name, nic.addresses, nic.networks, mac, nic.tailnet
            );
        }
    }

    if args.keystore_file {
        let store = platform.keystore();
        let name = "platform-probe";
        match store
            .store(name, b"platform-probe-value")
            .and_then(|()| store.load(name))
        {
            Ok(Some(value)) => println!("keystore round trip: {} bytes", value.len()),
            Ok(None) => println!("keystore round trip: missing"),
            Err(error) => println!("keystore round trip failed: {error}"),
        }
        if let Err(error) = store.delete(name) {
            println!("keystore delete failed: {error}");
        }
    }

    let (cursor_tx, mut cursor_rx) = mpsc::channel(128);
    let (clipboard_tx, mut clipboard_rx) = mpsc::channel(16);
    let (display_tx, mut display_rx) = mpsc::channel(16);
    let (activity_tx, mut activity_rx) = mpsc::channel(128);
    if args.cursor {
        platform.watch_cursor(cursor_tx);
    }
    if args.clipboard {
        platform.watch_clipboard(clipboard_tx);
    }
    if args.displays {
        platform.watch_displays(display_tx);
    }
    if args.activity {
        platform.watch_local_activity(activity_tx);
    }

    if let Some((mac, nic_name)) = &args.wol {
        let Some(mac) = parse_mac(mac) else {
            eprintln!("invalid WOL MAC address: {mac}");
            return;
        };
        let nics = platform.nics();
        let Some(nic) = nics.iter().find(|nic| nic.name == *nic_name) else {
            eprintln!("WOL interface not found: {nic_name}");
            return;
        };
        match platform.send_wol(mac, nic) {
            Ok(()) => {
                println!("sent WOL packet via {nic_name} (1 packet; the host repeats 3 times)")
            }
            Err(error) => eprintln!("WOL failed: {error}"),
        }
    }

    if args.keep_awake.is_some() {
        platform.power_keep_awake(true);
        println!("display sleep prevention enabled");
    }

    let run_seconds = args.keep_awake.unwrap_or(args.seconds);
    let deadline = tokio::time::sleep(Duration::from_secs(run_seconds));
    tokio::pin!(deadline);
    let input_probe = input::run(&platform, &args.input);
    tokio::pin!(input_probe);
    let mut input_finished = args.input.is_empty();
    loop {
        tokio::select! {
            _ = &mut deadline, if input_finished => break,
            _ = &mut input_probe, if !input_finished => {
                input_finished = true;
                // Keep watching for --seconds after injection, while also
                // reporting real activity promptly during its countdown.
                deadline.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(run_seconds));
            }
            Some(update) = cursor_rx.recv(), if args.cursor => print_cursor(update),
            Some(()) = clipboard_rx.recv(), if args.clipboard => {
                if let Some(item) = platform.clipboard_get() {
                    println!("clipboard changed: kind={:?} len={} sha256={}", item.kind, item.data.len(), short_hash(&item.sha256));
                } else {
                    println!("clipboard changed: unavailable");
                }
            }
            Some(displays) = display_rx.recv(), if args.displays => {
                println!("displays changed: {displays:?}");
            }
            Some(active) = activity_rx.recv(), if args.activity => {
                println!("activity: {active} at {} us", rsp_core::clock::unix_micros());
            }
            else => break,
        }
    }
    if args.keep_awake.is_some() {
        platform.power_keep_awake(false);
        println!("display sleep prevention released");
    }
}

fn parse_mac(value: &str) -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    let mut parts = value.split(':');
    for byte in &mut mac {
        *byte = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(mac)
}

fn print_cursor(update: CursorUpdate) {
    match update {
        CursorUpdate::Shape(shape) => println!(
            "cursor shape: id={} size={}x{} hot=({}, {}) scale={}",
            shape.id, shape.width, shape.height, shape.hot_x, shape.hot_y, shape.scale
        ),
        CursorUpdate::Use(id) => println!("cursor use: id={id}"),
        CursorUpdate::Hidden(hidden) => println!("cursor hidden: {hidden}"),
        CursorUpdate::Position {
            display_id,
            seq,
            x,
            y,
            visible,
        } => println!(
            "cursor position: display={} seq={} x={:.4} y={:.4} visible={visible}",
            display_id, seq, x, y
        ),
    }
}
