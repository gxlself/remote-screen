#[cfg(target_os = "linux")]
mod linux {
    use std::collections::{HashSet, VecDeque};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use anyhow::{Context, Result};
    use clap::{Args, Parser, Subcommand};
    use host_linux::ctl::{
        approval_body, approval_title, default_socket, pairing_result_text, pairing_text,
        parse_choice, respond_params, status_text, ApprovalPrompt, IpcClient,
    };
    use host_linux::install::{self, InstallPaths, SystemctlCmd};
    use host_linux::notify::{run_notify, Notifier, NotifyLoopOptions};
    use host_linux::update_appimage::RuntimeExtractor;
    use tokio::sync::mpsc::{self, UnboundedReceiver};

    #[derive(Debug, Parser)]
    #[command(
        name = "remote-host-ctl",
        version,
        about = "Remote Host command-line control"
    )]
    struct Cli {
        #[arg(long, global = true)]
        socket: Option<PathBuf>,
        #[command(subcommand)]
        command: Command,
    }

    #[derive(Debug, Subcommand)]
    enum Command {
        Status,
        Pair,
        Approve(ApproveArgs),
        Notify,
        Install(InstallArgs),
        Uninstall(UninstallArgs),
    }

    #[derive(Debug, Args)]
    struct ApproveArgs {
        #[arg(long)]
        once: bool,
    }

    #[derive(Debug, Args)]
    struct InstallArgs {
        #[arg(long = "from")]
        from: Option<PathBuf>,
    }

    #[derive(Debug, Args)]
    struct UninstallArgs {
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        yes: bool,
    }

    fn now_us() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_micros().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0)
    }

    fn socket(cli: &Cli) -> PathBuf {
        cli.socket.clone().unwrap_or_else(default_socket)
    }

    fn service_hint() -> &'static str {
        "服务没在运行：systemctl --user status remote-host.service"
    }

    async fn connect(path: &Path) -> Result<IpcClient> {
        IpcClient::connect(path).await.map_err(anyhow::Error::from)
    }

    async fn status(path: PathBuf) -> Result<i32> {
        let mut client = match connect(&path).await {
            Ok(client) => client,
            Err(error) => {
                eprintln!("{}", service_hint());
                eprintln!("{error}");
                return Ok(1);
            }
        };
        let status = match client.status().await {
            Ok(status) => status,
            Err(error) => {
                eprintln!("{}", service_hint());
                eprintln!("{error}");
                return Ok(1);
            }
        };
        println!("{}", status_text(&status));
        Ok(0)
    }

    async fn pair(path: PathBuf) -> Result<i32> {
        let mut client = connect(&path).await.context(service_hint())?;
        let pairing = client.pairing_start().await?;
        println!("{}", pairing_text(&pairing, now_us()));
        loop {
            let remaining = pairing.expires_at.saturating_sub(now_us());
            if remaining == 0 {
                println!("配对已过期");
                return Ok(1);
            }
            tokio::select! {
                event = client.next_event() => {
                    let event = event?;
                    if event.method == "event.pairingResult" {
                        println!("{}", pairing_result_text(&event.params));
                        return Ok(if event.params.get("status").and_then(serde_json::Value::as_u64) == Some(1) { 0 } else { 1 });
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    let _ = client.pairing_cancel().await;
                    return Ok(130);
                }
                _ = tokio::time::sleep(Duration::from_micros(remaining)) => {
                    println!("配对已过期");
                    return Ok(1);
                }
            }
        }
    }

    fn input_lines() -> UnboundedReceiver<Option<String>> {
        let (sender, receiver) = mpsc::unbounded_channel();
        std::thread::spawn(move || loop {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) => {
                    let _ = sender.send(None);
                    break;
                }
                Ok(_) if sender.send(Some(line)).is_err() => break,
                Err(_) => {
                    let _ = sender.send(None);
                    break;
                }
                _ => {}
            }
        });
        receiver
    }

    fn print_prompt(prompt: &ApprovalPrompt) -> Result<()> {
        println!("\n{}\n{}", approval_title(prompt), approval_body(prompt));
        if prompt.permission_choice {
            println!("选项：1=允许查看，2=允许控制，3=允许无人值守，n=拒绝");
        } else {
            println!("选项：y=允许，n=拒绝");
        }
        std::io::stdout().flush().context("flush prompt")
    }

    async fn approve(path: PathBuf, options: ApproveArgs) -> Result<i32> {
        let mut client = connect(&path).await.context(service_hint())?;
        let mut queue: VecDeque<ApprovalPrompt> = client.approval_pending().await?.into();
        let mut closed = HashSet::new();
        let mut lines = input_lines();
        loop {
            if let Some(prompt) = queue.pop_front() {
                if closed.remove(&prompt.request_id) {
                    println!("{}：已在别处处理或已超时", approval_title(&prompt));
                    continue;
                }
                print_prompt(&prompt)?;
                loop {
                    tokio::select! {
                        line = lines.recv() => {
                            let Some(Some(line)) = line else { return Ok(0) };
                            if let Some(choice) = parse_choice(&line, prompt.permission_choice) {
                                client.approval_respond(&respond_params(&prompt, choice)).await?;
                                break;
                            }
                            println!("输入无效，请重试");
                            print_prompt(&prompt)?;
                        }
                        event = client.next_event() => {
                            let event = event?;
                            match event.method.as_str() {
                                "event.approvalRequested" => {
                                    if let Some(next) = host_linux::ctl::approval_from_value(&event.params) {
                                        queue.push_back(next);
                                    }
                                }
                                "event.approvalClosed" => {
                                    let Some(request_id) = event.params.get("request_id").and_then(serde_json::Value::as_str) else { continue };
                                    if request_id == prompt.request_id {
                                        println!("已在别处处理或已超时");
                                        break;
                                    }
                                    closed.insert(request_id.to_owned());
                                }
                                _ => {}
                            }
                        }
                    }
                }
                continue;
            }
            if options.once {
                return Ok(0);
            }
            let event = client.next_event().await?;
            match event.method.as_str() {
                "event.approvalRequested" => {
                    if let Some(prompt) = host_linux::ctl::approval_from_value(&event.params) {
                        queue.push_back(prompt);
                    }
                }
                "event.approvalClosed" => {
                    if let Some(request_id) = event
                        .params
                        .get("request_id")
                        .and_then(serde_json::Value::as_str)
                    {
                        closed.insert(request_id.to_owned());
                    }
                }
                _ => {}
            }
        }
    }

    async fn notify(path: PathBuf) -> Result<i32> {
        let conn = zbus::Connection::session().await.map_err(|error| {
            anyhow::anyhow!("session bus or notification service unavailable: {error}")
        })?;
        let notifier = Notifier::new(conn).await?;
        run_notify(
            NotifyLoopOptions {
                socket: path,
                reconnect: Duration::from_secs(3),
            },
            notifier,
        )
        .await?;
        Ok(0)
    }

    async fn install(args: InstallArgs) -> Result<i32> {
        let source = args
            .from
            .or_else(|| {
                std::env::var_os("APPIMAGE")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            })
            .ok_or(install::InstallError::NoSource)?;
        let paths = InstallPaths::from_env()?;
        let report = install::install(&source, &paths, &RuntimeExtractor, &SystemctlCmd)?;
        if let Some(old) = &report.upgraded_from {
            println!("已从 {old} 升级到 {}", report.version);
        } else {
            println!("已安装版本 {}", report.version);
        }
        if !report.curl_found {
            println!("未找到 curl，自动更新无法下载，请安装 curl");
        }
        if report.bin_link_skipped {
            println!("~/.local/bin/remote-host-ctl 已存在未覆盖");
        }
        println!("首次连接时桌面会弹出屏幕共享授权，请在电脑前点允许");
        Ok(0)
    }

    async fn uninstall(args: UninstallArgs) -> Result<i32> {
        if !args.yes {
            print!("确认卸载 Remote Host？输入 yes 继续：");
            std::io::stdout().flush().context("flush confirmation")?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if !line.trim().eq_ignore_ascii_case("yes") {
                println!("已取消");
                return Ok(0);
            }
        }
        let paths = InstallPaths::from_env()?;
        install::uninstall(&paths, &SystemctlCmd, args.purge)?;
        println!("已卸载");
        Ok(0)
    }

    pub async fn run() -> Result<i32> {
        let cli = Cli::parse();
        let path = socket(&cli);
        match cli.command {
            Command::Status => status(path).await,
            Command::Pair => pair(path).await,
            Command::Approve(options) => approve(path, options).await,
            Command::Notify => notify(path).await,
            Command::Install(args) => install(args).await,
            Command::Uninstall(args) => uninstall(args).await,
        }
    }
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() {
    match linux::run().await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("remote-host-ctl 只支持 Linux");
    std::process::exit(2);
}
