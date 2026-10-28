//! Manual Windows input/session-operation probe.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io;
    use std::thread::sleep;
    use std::time::Duration;

    use host_core::platform::{DisplayInfo, InputMsg};
    use rsp_core::hid::{ScrollPhase, ScrollUnit};

    use host_core::display_model::to_display_infos;
    use host_win::displays::enumerate;
    use host_win::inject::{sas_allowed, send_sas, WinInjector};

    fn primary_display() -> DisplayInfo {
        let displays = to_display_infos(&enumerate());
        displays
            .iter()
            .find(|display| display.primary)
            .cloned()
            .or_else(|| displays.first().cloned())
            .unwrap_or_else(|| DisplayInfo {
                id: 0,
                name: "unknown".to_owned(),
                width_px: 1,
                height_px: 1,
                scale: 1.0,
                x: 0,
                y: 0,
                refresh_hz: 60,
                primary: true,
                rotation: 0,
            })
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(action) = args.first().map(String::as_str) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: input_probe --move | --text <text> | --scroll <lines> | --lock | --sas",
        )
        .into());
    };

    let wait = || sleep(Duration::from_secs(3));
    match action {
        "--move" if args.len() == 1 => {
            let display = primary_display();
            println!("moving to the primary display center in 3 seconds");
            wait();
            WinInjector::new().inject_input(&InputMsg::MouseMove { x: 0.5, y: 0.5 }, &display);
        }
        "--text" if args.len() == 2 => {
            let display = primary_display();
            println!("injecting text in 3 seconds");
            wait();
            WinInjector::new().inject_input(&InputMsg::Text(args[1].clone()), &display);
        }
        "--scroll" if args.len() == 2 => {
            let lines = args[1].parse::<f32>()?;
            let display = primary_display();
            println!("scrolling {lines} lines in 3 seconds");
            wait();
            WinInjector::new().inject_input(
                &InputMsg::Scroll {
                    dx: 0.0,
                    dy: lines,
                    phase: ScrollPhase::None,
                    momentum: ScrollPhase::None,
                    unit: ScrollUnit::Line,
                },
                &display,
            );
        }
        "--lock" if args.len() == 1 => {
            println!("locking the workstation in 3 seconds");
            wait();
            WinInjector::new().lock_workstation()?;
        }
        "--sas" if args.len() == 1 => {
            println!("SoftwareSASGeneration allows services: {}", sas_allowed());
            println!("sending Ctrl+Alt+Del in 3 seconds");
            wait();
            send_sas()?;
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: input_probe --move | --text <text> | --scroll <lines> | --lock | --sas",
            )
            .into())
        }
    }
    Ok(())
}
