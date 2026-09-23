//! rapidgate-cli — 配置校验 / 路由查看工具
//!
//! 子命令：
//! - `check <config-path>` — 校验配置文件语法与规则
//! - `routes <config-path>` — 查看路由规则
//! - `version` — 显示版本号

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use rapidgate::core::routing::RouteTable;
use rapidgate::service::config_loader;

#[derive(Parser)]
#[command(name = "rapidgate-cli")]
#[command(about = "RapidGate configuration utility")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validate configuration file syntax and rules
    Check {
        /// Path to configuration directory or file
        config_path: PathBuf,
    },
    /// Display configured routes
    Routes {
        /// Path to configuration directory or file
        config_path: PathBuf,
    },
    /// Display version information
    Version,
}

#[tokio::main]
async fn main() -> ExitCode {
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    match cli.command {
        Commands::Check { config_path } => run_check(config_path).await,
        Commands::Routes { config_path } => run_routes(config_path).await,
        Commands::Version => {
            println!("rapidgate-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
    }
}

/// 校验配置文件
async fn run_check(config_path: PathBuf) -> ExitCode {
    // 设置配置目录环境变量
    let config_dir = if config_path.is_dir() {
        config_path.clone()
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf()
    };

    std::env::set_var("RGD_CONFIG_DIR", config_dir.to_string_lossy().as_ref());

    match config_loader::load().await {
        Ok(cfg) => {
            println!("Configuration valid.");
            println!("  Routes: {}", cfg.routes.len());
            println!("  Upstreams: {}", cfg.gateway.upstreams.len());
            println!("  Listen: {}", cfg.gateway.listen);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Configuration error: {e}");
            ExitCode::from(1)
        }
    }
}

/// 查看路由规则
async fn run_routes(config_path: PathBuf) -> ExitCode {
    let config_dir = if config_path.is_dir() {
        config_path.clone()
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf()
    };

    std::env::set_var("RGD_CONFIG_DIR", config_dir.to_string_lossy().as_ref());

    let cfg = match config_loader::load().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load configuration: {e}");
            return ExitCode::from(1);
        }
    };

    if cfg.routes.is_empty() {
        println!("No routes configured.");
        return ExitCode::SUCCESS;
    }

    // 编译路由表以验证
    let table = match RouteTable::new(cfg.routes.clone()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Route compilation error: {e}");
            return ExitCode::from(1);
        }
    };

    println!("Routes ({} total):\n", table.len());
    println!("{:<20} {:<10} {:<30} UPSTREAM", "NAME", "METHOD", "PATH");
    println!("{}", "-".repeat(70));

    for route in &cfg.routes {
        let upstream = route
            .upstream
            .as_ref()
            .map(|u| u.id.as_str())
            .unwrap_or("-");
        println!(
            "{:<20} {:<10} {:<30} {}",
            route.name, route.match_rule.method, route.match_rule.path, upstream
        );
    }

    ExitCode::SUCCESS
}
