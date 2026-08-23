use sysinfo::{ProcessStatus, System, Users};

#[derive(Debug, Clone, Default)]
pub struct ProcessSnapshot {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub user: String,
    pub cpu: f32,
    pub mem: u64,
    pub virt: u64,
    pub status: String,
    pub is_zombie: bool,
    pub cmd: String,
    pub exe: String,
    pub cwd: String,
    pub start_time: u64,
    pub run_time: u64,
    pub io_read: u64,
    pub io_write: u64,
    pub io_read_total: u64,
    pub io_write_total: u64,
}

pub fn collect(sys: &System, users: &Users) -> Vec<ProcessSnapshot> {
    let mut out: Vec<ProcessSnapshot> = sys
        .processes()
        .iter()
        .filter(|(_, proc)| proc.thread_kind().is_none())
        .map(|(pid, proc)| {
            let user = proc
                .user_id()
                .and_then(|uid| users.get_user_by_id(uid).map(|u| u.name().to_string()))
                .or_else(|| proc.user_id().map(|uid| uid.to_string()))
                .unwrap_or_else(|| "?".into());
            let cmd = if proc.cmd().is_empty() {
                proc.name().to_string_lossy().into_owned()
            } else {
                proc.cmd()
                    .iter()
                    .map(|s| s.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let status = proc.status();
            ProcessSnapshot {
                pid: pid.as_u32(),
                parent: proc.parent().map(|p| p.as_u32()),
                name: proc.name().to_string_lossy().into_owned(),
                user,
                cpu: proc.cpu_usage(),
                mem: proc.memory(),
                virt: proc.virtual_memory(),
                status: format!("{status:?}"),
                is_zombie: matches!(status, ProcessStatus::Zombie),
                cmd,
                exe: proc
                    .exe()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                cwd: proc
                    .cwd()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                start_time: proc.start_time(),
                run_time: proc.run_time(),
                io_read: proc.disk_usage().read_bytes,
                io_write: proc.disk_usage().written_bytes,
                io_read_total: proc.disk_usage().total_read_bytes,
                io_write_total: proc.disk_usage().total_written_bytes,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

pub fn signal(pid: u32, kill9: bool) -> anyhow::Result<()> {
    use sysinfo::{Pid, ProcessesToUpdate, Signal};

    let mut sys = System::new();
    let handle = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[handle]), true);
    let Some(proc) = sys.process(handle) else {
        anyhow::bail!("process {pid} not found");
    };
    let sig = if kill9 { Signal::Kill } else { Signal::Term };
    match proc.kill_with(sig) {
        Some(true) => Ok(()),
        Some(false) => anyhow::bail!("failed to signal pid {pid}"),
        None => anyhow::bail!("signal not supported on this platform"),
    }
}

#[cfg(unix)]
pub fn renice(pid: u32, nice: i32) -> anyhow::Result<()> {
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, pid, nice) };
    if rc != 0 {
        anyhow::bail!(
            "renice {pid} -> {nice} failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn renice(_pid: u32, _nice: i32) -> anyhow::Result<()> {
    anyhow::bail!("renice is only available on Unix")
}

pub fn inspect(pid: u32, users: &Users) -> anyhow::Result<InspectInfo> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate};

    let mut sys = System::new();
    let handle = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[handle]),
        true,
        ProcessRefreshKind::everything(),
    );
    let Some(proc) = sys.process(handle) else {
        anyhow::bail!("process {pid} not found");
    };
    let user = proc
        .user_id()
        .and_then(|uid| users.get_user_by_id(uid).map(|u| u.name().to_string()))
        .unwrap_or_else(|| "?".into());
    Ok(InspectInfo {
        pid,
        name: proc.name().to_string_lossy().into_owned(),
        user,
        cmd: proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" "),
        exe: proc
            .exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        cwd: proc
            .cwd()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        parent: proc.parent().map(|p| p.as_u32()),
        status: format!("{:?}", proc.status()),
        cpu: proc.cpu_usage(),
        mem: proc.memory(),
        virt: proc.virtual_memory(),
        start_time: proc.start_time(),
        run_time: proc.run_time(),
        open_files: proc.open_files(),
        environ_count: proc.environ().len(),
    })
}

#[derive(Debug, Clone)]
pub struct InspectInfo {
    pub pid: u32,
    pub name: String,
    pub user: String,
    pub cmd: String,
    pub exe: String,
    pub cwd: String,
    pub parent: Option<u32>,
    pub status: String,
    pub cpu: f32,
    pub mem: u64,
    pub virt: u64,
    pub start_time: u64,
    pub run_time: u64,
    pub open_files: Option<usize>,
    pub environ_count: usize,
}
