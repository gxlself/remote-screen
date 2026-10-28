mod support;

use std::path::Path;
use std::sync::Arc;

use rs_mobile::rscore::api_sftp::{self, SftpConnector};
use rs_mobile::rscore::dto_core::{CoreConfig, DevicePlatform, NetPath};
use rs_mobile::rscore::dto_files::{
    FileSourceRef, FileTransfer, FileTransferDirection, FileTransferState,
};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::{
    core, init_core, register_transfer_drivers, take_core, Core, CoreOptions,
};
use rs_mobile::rscore::xfer;
use rs_ssh::SshSession;

struct PendingConnector;

#[async_trait::async_trait]
impl SftpConnector for PendingConnector {
    async fn connect(&self, _core: &Arc<Core>, _server_id: &str) -> Result<SshSession, CoreError> {
        // Keep the transfer active until cancel; shutdown aborts connection setup.
        std::future::pending().await
    }
}

fn transfer(id: &str, source: FileSourceRef, state: FileTransferState) -> FileTransfer {
    let net_path = matches!(&source, FileSourceRef::Computer { .. }).then_some(NetPath::Lan);
    FileTransfer {
        transfer_id: id.to_owned(),
        direction: FileTransferDirection::Download,
        source,
        name: format!("{id}.bin"),
        remote_path: Some(format!("/remote/{id}.bin")),
        local_path: None,
        size: 3 * 1024 * 1024,
        done: 0,
        bytes_per_sec: 0,
        state,
        fail: None,
        net_path,
        saved_name: None,
        started_at: 1_000_000,
        updated_at: 1_000_000,
    }
}

fn write_transfers(data_dir: &Path) {
    let transfers = [
        transfer(
            "computer-running",
            FileSourceRef::Computer {
                host_id: "host-1".to_owned(),
            },
            FileTransferState::Running,
        ),
        transfer(
            "server-paused",
            FileSourceRef::Server {
                server_id: "server-1".to_owned(),
            },
            FileTransferState::Paused,
        ),
    ];
    std::fs::write(
        data_dir.join("transfers.json"),
        serde_json::to_vec(&transfers).unwrap(),
    )
    .unwrap();
}

fn reopen(data_dir: &Path) -> Arc<Core> {
    Core::new(
        CoreConfig {
            files_dir: None,
            data_dir: data_dir.to_string_lossy().into_owned(),
            app_version: "test".to_owned(),
            platform: DevicePlatform::Ios,
        },
        CoreOptions {
            init_logging: false,
            ..CoreOptions::default()
        },
    )
    .unwrap()
}

fn state(core: &Arc<Core>, id: &str) -> FileTransferState {
    xfer::list(core)
        .into_iter()
        .find(|transfer| transfer.transfer_id == id)
        .unwrap()
        .state
}

fn clear_global_core() {
    if let Some(core) = take_core() {
        core.shutdown();
    }
}

fn assert_reopened_transfer_controls(core: &Arc<Core>) {
    api_sftp::set_connector(core, Arc::new(PendingConnector));
    assert_eq!(xfer::list(core).len(), 2);
    assert_eq!(
        state(core, "computer-running"),
        FileTransferState::Interrupted
    );
    assert_eq!(state(core, "server-paused"), FileTransferState::Paused);
    assert_eq!(xfer::cancel(core, "computer-running"), Ok(()));
    assert_eq!(
        state(core, "computer-running"),
        FileTransferState::Cancelled
    );

    let resume = core.block_on(xfer::resume(core, "server-paused"));
    if let Err(error) = resume {
        assert!(
            !error.to_string().contains("noDriver"),
            "server resume unexpectedly lacked a driver: {error}"
        );
    }
    assert_eq!(xfer::cancel(core, "server-paused"), Ok(()));
    assert_eq!(state(core, "server-paused"), FileTransferState::Cancelled);
}

#[test]
fn registering_drivers_restores_reopened_transfer_controls() {
    let data_dir = support::temp_dir("xfer-drivers");
    write_transfers(&data_dir);
    let core = reopen(&data_dir);
    register_transfer_drivers(&core);

    assert_reopened_transfer_controls(&core);
    core.shutdown();
}

#[test]
fn registering_transfer_drivers_twice_is_idempotent() {
    let data_dir = support::temp_dir("xfer-drivers-twice");
    write_transfers(&data_dir);
    let core = reopen(&data_dir);
    register_transfer_drivers(&core);
    register_transfer_drivers(&core);

    assert_reopened_transfer_controls(&core);
    core.shutdown();
}

#[test]
fn missing_transfer_driver_returns_source_specific_error() {
    let data_dir = support::temp_dir("xfer-no-driver");
    write_transfers(&data_dir);

    let core = reopen(&data_dir);
    let error = xfer::cancel(&core, "computer-running").unwrap_err();
    assert_eq!(
        error,
        CoreError::unsupported("transfers.noDriver: computer")
    );
    core.shutdown();
}

#[test]
fn init_core_registers_drivers_on_create_and_reload() {
    clear_global_core();
    let data_dir = support::temp_dir("xfer-init");
    write_transfers(&data_dir);
    let cfg = CoreConfig {
        files_dir: None,
        data_dir: data_dir.to_string_lossy().into_owned(),
        app_version: "test".to_owned(),
        platform: DevicePlatform::Ios,
    };

    init_core(cfg.clone()).unwrap();
    let first = core().unwrap();
    init_core(cfg).unwrap();
    let reloaded = core().unwrap();
    assert!(Arc::ptr_eq(&first, &reloaded));
    assert_eq!(xfer::cancel(&reloaded, "computer-running"), Ok(()));
    assert_eq!(
        state(&reloaded, "computer-running"),
        FileTransferState::Cancelled
    );

    let active = take_core().unwrap();
    active.shutdown();
}
