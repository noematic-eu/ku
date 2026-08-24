use std::io::stdout;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, EventStream};
use crossterm::execute;
use futures::StreamExt;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

use ku::app::App;
use ku::collector::{self, Snapshot};
use ku::config::{self, Config};
use ku::orphans;
use ku::storage::Storage;
use ku::ui;

#[derive(Parser, Debug)]
#[command(
    name = "ku",
    version,
    about = "TUI de monitoring système avancé (Linux + macOS)",
    before_help = ku::BANNER
)]
struct Cli {
    /// Path to config.toml
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Print the resolved config and exit
    #[arg(long)]
    dump_config: bool,

    /// Data directory (SQLite + logs). Defaults to the platform user data dir.
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Collect one snapshot, print a summary, and exit (no TUI)
    #[arg(long)]
    once: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Leftover data from uninstalled apps (list is always a dry-run)
    Orphans(OrphansArgs),
}

#[derive(Parser, Debug)]
struct OrphansArgs {
    /// Machine-readable report
    #[arg(long)]
    json: bool,

    /// Delete a leftover file, directory, or (with --all) every path for an app id.
    /// Does nothing if omitted or passed without a target.
    #[arg(long, value_name = "TARGET", num_args = 0..=1, default_missing_value = "")]
    rm: Option<String>,

    /// With --rm <app-id>, delete every leftover related to that uninstalled app
    #[arg(long)]
    all: bool,

    /// Hide an app id or leftover path (persisted in config.toml)
    #[arg(long, value_name = "APP_OR_PATH")]
    ignore: Option<String>,

    /// Remove one allowlist entry
    #[arg(long, value_name = "APP_OR_PATH")]
    unignore: Option<String>,

    /// Drop every allowlist entry
    #[arg(long)]
    clear_ignore: bool,

    /// Print the allowlist and exit
    #[arg(long)]
    ignored: bool,

    /// Open macOS System Settings on Full Disk Access
    #[arg(long)]
    fda: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Command::Orphans(args)) = cli.command {
        return run_orphans(args, cli.config);
    }

    let (config, config_path) = Config::load(cli.config.as_deref())?;

    if cli.dump_config {
        println!("# {}", config_path.display());
        print!("{}", config.to_toml());
        return Ok(());
    }

    if cli.once {
        return print_once(config);
    }

    let data_dir = cli.data_dir.unwrap_or_else(config::default_data_dir);
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;
    ku::paths::chown_to_invoker(&data_dir);

    init_tracing(&data_dir);
    tracing::info!(config = %config_path.display(), data = %data_dir.display(), "starting ku");

    let storage = Storage::open(&data_dir.join("history.db"))?;
    let (tx, rx) = watch::channel(Snapshot::default());
    let collector_cfg = config.clone();
    let collector_store = storage.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_c = stop.clone();
    let _collector = std::thread::Builder::new()
        .name("ku-collector".into())
        .spawn(move || collector::run(collector_cfg, tx, collector_store, &stop_c))
        .context("spawn collector thread")?;

    let app = App::new(config, config_path, data_dir, storage);
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        ratatui::restore();
        original_hook(info);
    }));

    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture).ok();
    let result = run(&mut terminal, app, rx, &stop).await;
    stop.store(true, Ordering::Relaxed);
    drain_input();
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    println!();
    result
}

const SHUTDOWN_ANIM: Duration = Duration::from_millis(560);

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    mut app: App,
    mut rx: watch::Receiver<Snapshot>,
    stop: &AtomicBool,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut shutdown_since: Option<Instant> = None;
    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if app.shutting_down {
            let started = *shutdown_since.get_or_insert_with(Instant::now);
            if started.elapsed() >= SHUTDOWN_ANIM {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(80)) => {}
                event = events.next() => {
                    let _ = event;
                }
            }
            continue;
        }
        tokio::select! {
            biased;
            event = events.next() => {
                match event {
                    Some(Ok(ev)) => {
                        app.handle_event(ev)?;
                        if let Some(path) = app.take_pending_ncdu() {
                            drop(events);
                            let opened = path.clone();
                            if let Err(err) = run_ncdu_session(terminal, &path) {
                                app.flash(err.to_string());
                            } else {
                                app.flash(format!("ncdu {}", opened.display()));
                            }
                            events = EventStream::new();
                        }
                        if app.shutting_down {
                            stop.store(true, Ordering::Relaxed);
                            shutdown_since = Some(Instant::now());
                        }
                    }
                    Some(Err(err)) => {
                        tracing::warn!(error = %err, "terminal event error");
                    }
                    None => break,
                }
            }
            changed = rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let snap = rx.borrow().clone();
                app.apply_snapshot(snap);
            }
            _ = tokio::time::sleep(Duration::from_millis(80)), if app.is_busy() => {}
        }
    }
    Ok(())
}

fn drain_input() {
    while crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
        let _ = crossterm::event::read();
    }
}

fn run_ncdu_session(terminal: &mut ratatui::DefaultTerminal, path: &std::path::Path) -> Result<()> {
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    drain_input();
    let result = tokio::task::block_in_place(|| ku::utils::run_ncdu(path));
    drain_input();
    *terminal = ratatui::init();
    let _ = execute!(stdout(), EnableMouseCapture);
    result
}

fn scan_orphans_cli() -> Result<ku::orphans::OrphanReport> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_t = cancel.clone();
    let handle = std::thread::spawn(move || ku::orphans::scan_with_cancel(&cancel_t));
    let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let mut i = 0usize;
    while !handle.is_finished() {
        eprint!(
            "\r{} scanning leftover app data…  (Ctrl-C to abort)   ",
            frames[i % frames.len()]
        );
        let _ = std::io::Write::flush(&mut std::io::stderr());
        std::thread::sleep(Duration::from_millis(80));
        i += 1;
    }
    eprintln!();
    match handle.join() {
        Ok(r) => r,
        Err(_) => anyhow::bail!("scan thread panicked"),
    }
}

fn run_orphans(args: OrphansArgs, config_path: Option<PathBuf>) -> Result<()> {
    if args.all && args.rm.is_none() {
        bail!("--all requires --rm <app-id|path>");
    }
    if matches!(&args.rm, Some(t) if t.is_empty()) {
        bail!("--rm requires a file, directory, or app id (nothing deleted)");
    }
    let (mut config, path) = Config::load(config_path.as_deref())?;
    if args.fda {
        orphans::open_fda_settings()?;
        eprintln!(
            "enable Full Disk Access for {} then relaunch ku",
            orphans::fda_app_hint()
        );
        return Ok(());
    }
    if args.clear_ignore {
        let n = config.orphans.ignore.len();
        config.orphans.ignore.clear();
        config.save(&path)?;
        eprintln!("cleared {n} allowlist entries");
        return Ok(());
    }
    if args.ignored {
        if config.orphans.ignore.is_empty() {
            println!("allowlist empty");
        } else {
            for rule in &config.orphans.ignore {
                println!("{rule}");
            }
        }
        return Ok(());
    }
    if let Some(target) = args.ignore {
        match orphans::add_ignore(&mut config.orphans.ignore, &target) {
            Ok(true) => {
                config.save(&path)?;
                println!("ignored {target}");
            }
            Ok(false) => println!("already ignored: {target}"),
            Err(err) => bail!("{err}"),
        }
        return Ok(());
    }
    if let Some(target) = args.unignore {
        if orphans::remove_ignore(&mut config.orphans.ignore, &target) {
            config.save(&path)?;
            println!("unignored {target}");
        } else {
            bail!("not on the allowlist: {target}");
        }
        return Ok(());
    }
    let mut report = scan_orphans_cli()?;
    match args.rm {
        None => {
            let hidden = orphans::apply_ignore(&mut report, &config.orphans.ignore);
            if args.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                orphans::print_table(&report);
                if hidden > 0 {
                    eprintln!("{hidden} path(s) hidden by allowlist  (ku orphans --ignored)");
                }
            }
            Ok(())
        }
        Some(target) => {
            let paths = orphans::resolve_delete_targets(&report, &target, args.all)?;
            let prompt = if args.all {
                format!(
                    "Delete ALL {} leftover path(s) for `{target}`?",
                    paths.len()
                )
            } else {
                format!("Delete {} leftover path(s)?", paths.len())
            };
            if !orphans::confirm_delete(&paths, &prompt, true)? {
                eprintln!("aborted");
                return Ok(());
            }
            let outcome = orphans::delete_targets(&paths);
            outcome.print_cli();
            if outcome.incomplete() {
                let leftover: Vec<_> = outcome.failed.iter().map(|f| f.path.clone()).collect();
                if orphans::running_as_root() {
                    eprintln!("already root — retrying with chflags + rm");
                    match orphans::force_remove(&leftover) {
                        Ok(()) if leftover.iter().all(|p| !p.exists()) => return Ok(()),
                        Ok(()) => {
                            let still = leftover.iter().filter(|p| p.exists()).count();
                            eprintln!(
                                "{still} path(s) still present (SIP / Full Disk Access / Finder)"
                            );
                        }
                        Err(err) => eprintln!("{err}"),
                    }
                } else if outcome.permission_denied()
                    && orphans::elevate_detect().is_some()
                    && orphans::confirm_yes("Elevate privileges and retry remaining paths?")
                        .unwrap_or(false)
                {
                    match orphans::elevate_remove(&leftover) {
                        Ok(method) => {
                            let still = leftover.iter().filter(|p| p.exists()).count();
                            eprintln!("elevated via {}", method.label());
                            if still == 0 {
                                return Ok(());
                            }
                            eprintln!("{still} path(s) still present");
                        }
                        Err(err) => eprintln!("{err}"),
                    }
                }
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

fn print_once(config: Config) -> Result<()> {
    use ku::collector::Collector;
    use ku::utils::{format_bytes, format_percent, format_uptime};

    let mut collector = Collector::new(config);
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    let snap = collector.collect();
    println!(
        "{}  {}  up {}",
        snap.hostname,
        snap.os,
        format_uptime(snap.uptime_secs)
    );
    println!(
        "cpu {:>6}   mem {} / {} ({})   swap {} / {}   load {:.2} {:.2} {:.2}   procs {}",
        format_percent(f64::from(snap.cpu.global)).trim(),
        format_bytes(snap.memory.used),
        format_bytes(snap.memory.total),
        format_percent(snap.memory.used_pct()).trim(),
        format_bytes(snap.memory.swap_used),
        format_bytes(snap.memory.swap_total),
        snap.load.one,
        snap.load.five,
        snap.load.fifteen,
        snap.process_count
    );
    for disk in snap.disks.iter().take(12) {
        println!(
            "  {:<18} {:>6}  {} / {}  {}",
            disk.mount,
            format_percent(disk.used_pct()).trim(),
            format_bytes(disk.used),
            format_bytes(disk.total),
            disk.fs
        );
    }
    for alert in &snap.alerts {
        println!("! {}", alert.message);
    }
    Ok(())
}

fn init_tracing(data_dir: &std::path::Path) {
    let log_path = data_dir.join("ku.log");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    else {
        return;
    };
    ku::paths::chown_to_invoker(&log_path);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("ku=info"));
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(filter)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
}
