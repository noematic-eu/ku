# ku

TUI de monitoring système pour administrateurs. Linux et macOS. Écrit en Rust avec [ratatui](https://ratatui.rs).

Au-delà d’un `htop` classique, `ku` ajoute le suivi d’espace disque, l’évolution des dossiers (growth tracking) et un historique des processus.

## Installer / lancer

```bash
cargo run --release
```

```bash
cargo install --path .
ku
```

```bash
ku --config /chemin/config.toml
ku --dump-config
ku --once                 # un snapshot texte, sans TUI
sudo ku                   # mêmes dossiers config/data que SUDO_USER (pas root)
```

## Vues

| Touche | Vue |
|--------|-----|
| `1` | Dashboard (CPU, mémoire, load, volumes, alertes) |
| `2` | Disk (volumes, inodes, seuils) |
| `3` | Growth (dossiers qui grandissent / rétrécissent) |
| `4` | Processes (live + historique) |
| `5` | Settings |
| `6` / `?` | Help |

## Raccourcis

- `Tab` / `Shift+Tab` — changer de vue
- `j` `k` ou flèches — navigation
- `/` — filtre (`nginx cpu>5 mem>100M user:root`)
- `s` / `S` — tri / inverser
- `Enter` — détail
- `a` — actions process (kill, kill -9, renice, inspect)
- `h` — historique process, ou fenêtre growth
- `r` — recharger
- `q` — quitter
- souris — clic sur un onglet, une valeur de config, une ligne (double-clic = détail), molette, clic droit process, `help` / `quit` en bas

## Configuration

Fichier par défaut (celui de l’utilisateur qui lance `ku`, y compris via `sudo`) :

- Linux : `~/.config/ku/config.toml`
- macOS : `~/Library/Application Support/ku/config.toml`

```toml
[general]
refresh_interval = 2
theme = "dark"          # dark | light
history_retention_days = 7

[disk]
warning_threshold = 80
critical_threshold = 90
snapshot_interval = 300
watched_paths = [
  "/var/log",
  "/tmp"
]

[processes]
history_window = ["1m", "5m", "1h", "24h"]
```

## Données

Snapshots SQLite (rétention 7 jours, max 30) :

- Linux : `~/.local/share/ku/history.db`
- macOS : `~/Library/Application Support/ku/history.db`

`sudo ku` lit et écrit ces chemins pour `SUDO_USER` (pas `/var/root` / `/root`), et rend les fichiers à cet utilisateur.

Le scan growth des `watched_paths` tourne en arrière-plan (intervalle `snapshot_interval`, 5 min par défaut). Les arbres trop larges sont bornés (profondeur, nombre d’entrées, dossiers type `node_modules` / `.git`).

## Roadmap

Voir `spec.md`. Cette version couvre le MVP (dashboard, disk, process temps réel) plus les fondations V0.2 (historique process, growth) et V0.3 (config, thèmes, alertes). Network est prévu en V0.4.
