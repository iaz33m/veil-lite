//! `veil-engine`: the headless background process. The desktop app starts it as a sidecar and talks to
//! it over local IPC; it can also run on its own (`veil-engine run`).

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use veil_core::activity::ActivityLog;
use veil_core::paths::Paths;
use veil_core::policy::{Action, Policy};
use veil_core::shared::Shared;
use veil_core::types::Channel;
use veil_core::{Engine, EngineEvent, Interceptor, Job, Platform};
use veil_detect::{DetectConfig, VeilScanner};
use veil_engine::{ModelStore, ScannerControl, Service, UnsupportedPlatform};
use veil_ipc::{Client, Request, Response, Server};

#[derive(Parser)]
#[command(name = "veil-engine", version, about = "Veil background engine: checks what you send to AI apps")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Keep every Veil file under this directory instead of the per-user locations.
    #[arg(long, global = true, env = "VEIL_HOME")]
    home: Option<PathBuf>,
    /// Log filter, e.g. `info`, `debug`, `veil_core=debug` (`RUST_LOG` wins when set).
    #[arg(long, global = true, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Command {
    /// Run the engine (the default).
    Run,
    /// Check a file, or text from stdin with `-`, and print what Veil would detect.
    Check {
        /// File path, or `-` for stdin.
        target: String,
        /// Print JSON.
        #[arg(long)]
        json: bool,
        /// Also print matched values (stdin only).
        #[arg(long)]
        show_values: bool,
        /// Skip the AI layer.
        #[arg(long)]
        no_model: bool,
    },
    /// Policy helpers.
    Policy {
        /// Print the default policy.
        #[arg(long)]
        print_default: bool,
        /// Validate a policy file.
        #[arg(long)]
        validate: Option<PathBuf>,
    },
    /// Show the status of the running engine.
    Status,
    /// Download the AI model now.
    DownloadModel,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(cli.home, &cli.log_level).map(|_| ExitCode::SUCCESS),
        Command::Check { target, json, show_values, no_model } => check(cli.home, &target, json, show_values, no_model),
        Command::Policy { print_default, validate } => policy_cmd(print_default, validate),
        Command::Status => status(cli.home),
        Command::DownloadModel => download_model(cli.home),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("veil-engine: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn paths(home: Option<PathBuf>) -> anyhow::Result<Paths> {
    match home {
        Some(home) => Ok(Paths::rooted(&home)),
        None => Paths::detect(),
    }
}

// ----- run -----

fn run(home: Option<PathBuf>, log_level: &str) -> anyhow::Result<()> {
    let paths = paths(home)?;
    paths.ensure().context("creating Veil's folders")?;
    veil_engine::logging::init(Some(&paths.log_file()), log_level)?;
    tracing::info!("veil-engine {} starting", env!("CARGO_PKG_VERSION"));

    if let Ok(mut client) = Client::connect(&paths) {
        if client.call(&Request::Status).is_ok() {
            println!("Veil engine is already running.");
            return Ok(());
        }
    }

    let policy = Policy::load_or_create(&paths.policy_file()).unwrap_or_else(|e| {
        tracing::error!("policy.yaml is invalid ({e:#}); using the default policy until it is fixed");
        Policy::default()
    });
    let scanner = Arc::new(VeilScanner::new(DetectConfig::from_policy(&policy, &paths.models_dir())));
    let platform = create_platform();
    tracing::info!("platform provider: {}", platform.name());

    let shared = Arc::new(Shared::new(policy.clone()));
    scanner_keywords(&scanner, &policy);
    let activity = Arc::new(ActivityLog::new(&paths.activity_file()));
    let engine = Arc::new(Engine::new(platform.clone(), scanner.clone(), shared.clone(), activity));
    let (jobs, job_rx) = crossbeam_channel::bounded(1024);
    let _worker = Arc::clone(&engine).spawn(job_rx);
    let interceptor = Arc::new(Interceptor::new(platform.clone(), shared, jobs.clone()));

    let service = Service::new(
        paths.clone(),
        engine.clone(),
        platform.clone(),
        Arc::new(DetectControl(scanner)),
        Arc::new(DetectModels(paths.models_dir())),
    );
    let server = match Server::start(&paths, service.clone()) {
        Ok(s) => s,
        Err(e) if is_addr_in_use(&e) => {
            println!("Veil engine is already running.");
            return Ok(());
        }
        Err(e) => return Err(e.context("starting the IPC server")),
    };
    service.start_policy_watcher();

    // The platform loop owns the main thread (macOS requires it). If the hooks cannot be installed yet
    // (e.g. a permission is missing), keep serving IPC and retry so the setup wizard can fix it.
    let mut warned = false;
    loop {
        service.set_active(platform.name() != "unsupported");
        match platform.clone().run(interceptor.clone()) {
            Ok(()) => break,
            Err(e) => {
                service.set_active(false);
                if service.is_stopping() {
                    break;
                }
                if !warned {
                    let message = format!("Protection is not running yet: {e:#}");
                    tracing::warn!("{message}");
                    engine.events.publish(EngineEvent::Warning { message });
                    warned = true;
                }
                for _ in 0..30 {
                    if service.is_stopping() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if service.is_stopping() {
                    break;
                }
            }
        }
    }

    tracing::info!("veil-engine stopping");
    service.shutdown();
    server.stop();
    let _ = jobs.send(Job::Shutdown);
    Ok(())
}

fn scanner_keywords(scanner: &VeilScanner, policy: &Policy) {
    use veil_core::Scanner;
    scanner.set_keywords(&policy.keywords);
}

fn is_addr_in_use(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
}

fn create_platform() -> Arc<dyn Platform> {
    #[cfg(target_os = "macos")]
    let created = veil_platform_macos::create();
    #[cfg(target_os = "windows")]
    let created = veil_platform_windows::create();
    #[cfg(target_os = "linux")]
    let created = veil_platform_linux::create();
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let created: anyhow::Result<Arc<dyn Platform>> = Err(anyhow::anyhow!("this operating system is not supported"));

    match created {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("platform provider failed to start: {e:#}");
            Arc::new(UnsupportedPlatform::new(format!("Veil could not start protection on this computer: {e:#}")))
        }
    }
}

struct DetectControl(Arc<VeilScanner>);

impl ScannerControl for DetectControl {
    fn apply_policy(&self, policy: &Policy) {
        self.0.apply_policy(policy);
    }
    fn refresh_model(&self) {
        self.0.refresh_model();
    }
    fn set_download_progress(&self, percent: Option<u8>) {
        self.0.set_download_progress(percent);
    }
    fn set_model_failed(&self, reason: String) {
        self.0.set_model_failed(reason);
    }
}

struct DetectModels(PathBuf);

impl ModelStore for DetectModels {
    fn is_installed(&self) -> bool {
        veil_detect::model::is_installed(&self.0)
    }
    fn download(&self, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> anyhow::Result<()> {
        veil_detect::model::download(&self.0, cancel, progress)
    }
    fn remove(&self) -> std::io::Result<()> {
        veil_detect::model::remove(&self.0)
    }
}

// ----- check -----

fn check(
    home: Option<PathBuf>,
    target: &str,
    json: bool,
    show_values: bool,
    no_model: bool,
) -> anyhow::Result<ExitCode> {
    let paths = paths(home)?;
    let mut policy = if paths.policy_file().exists() { Policy::load(&paths.policy_file())? } else { Policy::default() };
    if no_model {
        policy.model.enabled = false;
    }
    let scanner = Arc::new(VeilScanner::new(DetectConfig::from_policy(&policy, &paths.models_dir())));
    scanner_keywords(&scanner, &policy);
    let platform = Arc::new(UnsupportedPlatform::new("command line"));
    let engine = Engine::new(platform, scanner, Arc::new(Shared::new(policy)), Arc::new(ActivityLog::disabled()));
    let budget = Duration::from_secs(120);

    let (ins, channel, text) = if target == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).context("reading stdin")?;
        let ins = engine.inspect(Some((&text, "stdin")), &[], None, budget);
        (ins, Channel::Message, Some(text))
    } else {
        let path = Path::new(target);
        anyhow::ensure!(path.exists(), "{target} does not exist");
        (engine.inspect(None, &[path.to_path_buf()], None, budget), Channel::Upload, None)
    };
    let (action, triggered) = engine.decide(&ins, channel);
    let model = veil_core::Scanner::model_status(&**engine.scanner());

    let findings: Vec<serde_json::Value> = ins
        .findings
        .iter()
        .map(|f| {
            let mut v = serde_json::json!({
                "type": f.type_label(),
                "description": f.describe(),
                "score": f.score,
                "source": f.source,
                "span": f.span.as_ref().map(|s| [s.start, s.end]),
                "triggers": triggered.contains(f),
            });
            if show_values {
                if let (Some(text), Some(span)) = (&text, &f.span) {
                    v["value"] = text.get(span.clone()).unwrap_or_default().into();
                }
            }
            v
        })
        .collect();

    if json {
        let out = serde_json::json!({
            "action": action,
            "findings": findings,
            "unchecked": ins.unchecked,
            "complete": ins.complete,
            "model": model,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Action: {}", serde_json::to_value(action)?.as_str().unwrap_or("?"));
        if findings.is_empty() {
            println!("Nothing detected.");
        }
        for f in &findings {
            let span = f["span"].as_array().map(|s| format!(" [{}..{}]", s[0], s[1])).unwrap_or_default();
            let value = f.get("value").and_then(|v| v.as_str()).map(|v| format!(" = {v:?}")).unwrap_or_default();
            let note = if f["triggers"].as_bool() == Some(true) { "" } else { " (below threshold)" };
            println!(
                "  - {} ({}) score {:.2} in {}{}{}{}",
                f["description"].as_str().unwrap_or(""),
                f["type"].as_str().unwrap_or(""),
                f["score"].as_f64().unwrap_or(0.0),
                f["source"].as_str().unwrap_or(""),
                span,
                value,
                note
            );
        }
        for name in &ins.unchecked {
            println!("  ! could not check {name}");
        }
        if !ins.complete {
            println!("  ! the AI layer did not finish in time");
        }
        println!("AI model: {}", serde_json::to_value(&model)?["state"].as_str().unwrap_or("?"));
    }
    Ok(if action == Action::Allow { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

// ----- small commands -----

fn policy_cmd(print_default: bool, validate: Option<PathBuf>) -> anyhow::Result<ExitCode> {
    if let Some(path) = validate {
        Policy::load(&path).with_context(|| format!("{} is not a valid policy", path.display()))?;
        println!("{} is valid.", path.display());
        return Ok(ExitCode::SUCCESS);
    }
    if print_default {
        print!("{}", Policy::default().to_yaml());
        return Ok(ExitCode::SUCCESS);
    }
    anyhow::bail!("use --print-default or --validate <FILE>")
}

fn status(home: Option<PathBuf>) -> anyhow::Result<ExitCode> {
    let paths = paths(home)?;
    let Ok(mut client) = Client::connect(&paths) else {
        println!("Veil engine is not running.");
        return Ok(ExitCode::from(1));
    };
    match client.call(&Request::Status)? {
        Response::Status(s) => {
            println!("{}", serde_json::to_string_pretty(&s)?);
            Ok(ExitCode::SUCCESS)
        }
        other => anyhow::bail!("unexpected response: {other:?}"),
    }
}

fn download_model(home: Option<PathBuf>) -> anyhow::Result<ExitCode> {
    let paths = paths(home)?;
    paths.ensure()?;
    let dir = paths.models_dir();
    if veil_detect::model::is_installed(&dir) {
        println!("The AI model is already installed in {}.", dir.display());
        return Ok(ExitCode::SUCCESS);
    }
    let cancel = AtomicBool::new(false);
    let mut last = u64::MAX;
    veil_detect::model::download(&dir, &cancel, &mut |done, total| {
        let pct = (done * 100).checked_div(total).unwrap_or(0);
        if pct != last {
            last = pct;
            eprint!("\rDownloading the AI model… {pct}% ({} / {} MB)", done / 1_000_000, total / 1_000_000);
        }
    })?;
    eprintln!();
    println!("The AI model is installed in {}.", dir.display());
    Ok(ExitCode::SUCCESS)
}
