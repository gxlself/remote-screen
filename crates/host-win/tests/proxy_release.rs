use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use host_core::platform::{
    ConsoleState, DisplayInfo, HostInfo, InputMsg, KeyStore, Nic, Platform, Result,
};
use host_win::pipe_proto::{input_from_b64, Ctl, PipeFrame};
use host_win::proxy::{PipePlatform, ServiceLocal};
use rsp_core::hid::{Button, Mods};
use rsp_core::pb;
use tokio::sync::mpsc;

#[derive(Default)]
struct MemoryKeyStore(Mutex<HashMap<String, Vec<u8>>>);

impl KeyStore for MemoryKeyStore {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<()> {
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

#[derive(Clone, Default)]
struct FakeLocal {
    keys: Arc<MemoryKeyStore>,
}

impl ServiceLocal for FakeLocal {
    fn info(&self) -> HostInfo {
        HostInfo {
            name: "test-host".into(),
            os: pb::Platform::Windows,
            os_version: "11".into(),
            model: "fake".into(),
            arch: "x86_64".into(),
        }
    }

    fn console_state(&self) -> ConsoleState {
        ConsoleState::Locked
    }

    fn nics(&self) -> Vec<Nic> {
        Vec::new()
    }

    fn send_wol(&self, _mac: [u8; 6], _via: &Nic) -> Result<()> {
        Ok(())
    }

    fn keystore(&self) -> &dyn KeyStore {
        self.keys.as_ref()
    }

    fn ctrl_alt_del(&self) -> Result<()> {
        Ok(())
    }
}

type TestPlatform = PipePlatform<FakeLocal>;

struct Session {
    generation: u64,
    rx: mpsc::UnboundedReceiver<PipeFrame>,
}

fn attach(platform: &Arc<TestPlatform>) -> Session {
    let (tx, rx) = mpsc::unbounded_channel();
    let generation = platform.attach(tx);
    Session { generation, rx }
}

impl Session {
    fn recv(&mut self) -> PipeFrame {
        self.rx.blocking_recv().unwrap()
    }

    fn recv_input(&mut self) -> (u32, InputMsg) {
        match self.recv() {
            PipeFrame::Ctl(Ctl::Inject { display_id, input }) => {
                (display_id, input_from_b64(&input).unwrap())
            }
            frame => panic!("expected Inject, got {frame:?}"),
        }
    }

    fn assert_empty(&mut self) {
        assert!(matches!(
            self.rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}

fn display(id: u32) -> DisplayInfo {
    DisplayInfo {
        id,
        name: format!("display-{id}"),
        width_px: 1920,
        height_px: 1080,
        scale: 1.0,
        x: 0,
        y: 0,
        refresh_hz: 60,
        primary: true,
        rotation: 0,
    }
}

fn key(usage: u16, down: bool, mods: Mods) -> InputMsg {
    InputMsg::Key { usage, down, mods }
}

fn platform() -> Arc<TestPlatform> {
    Arc::new(PipePlatform::new(FakeLocal::default()))
}

#[test]
fn attach_releases_a_key_before_watch_replay() {
    let platform = platform();
    let mut first = attach(&platform);
    platform.inject_input(&key(0x28, true, Mods::NONE), &display(7));
    assert_eq!(first.recv_input().1, key(0x28, true, Mods::NONE));
    platform.detach(first.generation);

    let mut second = attach(&platform);
    assert_eq!(second.recv_input().1, key(0x28, false, Mods::NONE));
    second.assert_empty();
}

#[test]
fn attach_releases_modifiers_after_their_last_key() {
    let platform = platform();
    let mut first = attach(&platform);
    platform.inject_input(&key(0x04, true, Mods::LCTRL), &display(7));
    platform.inject_input(&key(0x04, false, Mods::LCTRL), &display(7));
    assert_eq!(first.recv_input().1, key(0x04, true, Mods::LCTRL));
    assert_eq!(first.recv_input().1, key(0x04, false, Mods::LCTRL));
    platform.detach(first.generation);

    let mut second = attach(&platform);
    assert_eq!(second.recv_input().1, key(0xE0, false, Mods::NONE));
    second.assert_empty();
}

#[test]
fn attach_releases_buttons_at_the_last_pointer() {
    let platform = platform();
    let mut first = attach(&platform);
    let down = InputMsg::MouseButton {
        button: Button::Left,
        down: true,
        x: 0.25,
        y: 0.75,
        clicks: 1,
    };
    platform.inject_input(&down, &display(7));
    assert_eq!(first.recv_input().1, down);
    platform.detach(first.generation);

    let mut second = attach(&platform);
    let (display_id, msg) = second.recv_input();
    assert_eq!(display_id, 7);
    assert_eq!(
        msg,
        InputMsg::MouseButton {
            button: Button::Left,
            down: false,
            x: 0.25,
            y: 0.75,
            clicks: 1,
        }
    );
    second.assert_empty();
}

#[test]
fn paired_inputs_are_not_replayed() {
    let platform = platform();
    let mut first = attach(&platform);
    platform.inject_input(&key(0x28, true, Mods::NONE), &display(7));
    platform.inject_input(&key(0x28, false, Mods::NONE), &display(7));
    first.recv_input();
    first.recv_input();
    platform.detach(first.generation);

    let mut second = attach(&platform);
    second.assert_empty();
}

#[test]
fn releases_dropped_while_disconnected_do_not_clear_shadow_state() {
    let platform = platform();
    let mut first = attach(&platform);
    platform.inject_input(&key(0x28, true, Mods::NONE), &display(7));
    first.recv_input();
    platform.detach(first.generation);
    platform.inject_input(&key(0x28, false, Mods::NONE), &display(7));

    let mut second = attach(&platform);
    assert_eq!(second.recv_input().1, key(0x28, false, Mods::NONE));
    second.assert_empty();
}

#[test]
fn dropped_key_downs_do_not_create_shadow_state() {
    let platform = platform();
    let first = attach(&platform);
    platform.detach(first.generation);
    platform.inject_input(&key(0x28, true, Mods::NONE), &display(7));

    let mut second = attach(&platform);
    second.assert_empty();
}

#[test]
fn release_all_clears_shadow_state_after_a_successful_send() {
    let platform = platform();
    let mut first = attach(&platform);
    platform.inject_input(&key(0x28, true, Mods::NONE), &display(7));
    first.recv_input();
    platform.release_all();
    assert_eq!(first.recv(), PipeFrame::Ctl(Ctl::ReleaseAll));
    platform.detach(first.generation);

    let mut second = attach(&platform);
    second.assert_empty();
}

#[test]
fn replay_happens_only_in_the_first_generation_after_a_press() {
    let platform = platform();
    let mut first = attach(&platform);
    platform.inject_input(&key(0x28, true, Mods::NONE), &display(7));
    first.recv_input();
    platform.detach(first.generation);

    let mut second = attach(&platform);
    assert_eq!(second.recv_input().1, key(0x28, false, Mods::NONE));
    second.assert_empty();
    platform.detach(second.generation);

    let mut third = attach(&platform);
    third.assert_empty();
}
