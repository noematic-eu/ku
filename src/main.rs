use std::io::stdout;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, EventStream};
use crossterm::execute;
use futures::StreamExt;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

use ku::app::App;
use ku::collector::{self, Snapshot};
use ku::config::{self, Config};
use ku::storage::Storage;
use ku::ui;

#[derive(Parser, Debug)]
#[command(
    name = "ku",
    version,
    about = "TUI de monitoring système avancé (Linux + macOS)"
)]
struct Cli {
    /// Path to config.toml
    #[arg(short, long)]
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
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
    let collector = tokio::spawn(async move {
        collector::run(collector_cfg, tx, collector_store).await;
    });

    let app = App::new(config, config_path, data_dir, storage);
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        ratatui::restore();
        original_hook(info);
    }));

    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture).ok();
    let result = run(&mut terminal, app, rx).await;
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    collector.abort();
    result
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    mut app: App,
    mut rx: watch::Receiver<Snapshot>,
) -> Result<()> {
    let mut events = EventStream::new();
    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if app.should_quit {
            break;
        }
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let snap = rx.borrow().clone();
                app.apply_snapshot(snap);
            }
            event = events.next() => {
                match event {
                    Some(Ok(ev)) => {
                        if app.handle_event(ev)? {
                            break;
                        }
                    }
                    Some(Err(err)) => {
                        tracing::warn!(error = %err, "terminal event error");
                    }
                    None => break,
                }
            }
        }
    }
    Ok(())
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
