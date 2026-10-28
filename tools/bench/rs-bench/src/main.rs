//! rs-bench 命令行：rs-bench <clock|capture|ssim> [参数...]
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: rs-bench <clock|capture|ssim> [args...]");
        return ExitCode::from(2);
    }
    let sub = args[0].clone();
    // 子命令自己的 clap 解析器把 args[0] 当程序名。
    args[0] = format!("rs-bench {sub}");
    let result = match sub.as_str() {
        "clock" => rs_bench::clock::cli(args),
        "capture" => rs_bench::capture::cli(args),
        "ssim" => rs_bench::ssim::cli(args),
        other => {
            eprintln!("rs-bench: unknown subcommand '{other}'");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(err) => {
            eprintln!("rs-bench {sub}: {err:#}");
            ExitCode::from(1)
        }
    }
}
