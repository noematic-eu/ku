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

ku orphans                # leftover data from uninstalled apps (dry-run)
ku orphans --json
sudo ku orphans           # also /Library and /var/lib when readable
ku orphans --rm PATH      # delete one leftover file or directory (confirms)
ku orphans --rm APP_ID --all   # delete every leftover for that app (confirms)
ku orphans --ignore APP_OR_PATH   # hide from future scans (config.toml)
ku orphans --unignore APP_OR_PATH
ku orphans --ignored              # print allowlist
ku orphans --clear-ignore
ku orphans --fda          # macOS: open Full Disk Access settings
sudo ku                   # same user config/data; or press e after a permission error
```

On macOS, leftover delete under `~/Library/Containers` (and similar) needs **Full Disk Access** for the **terminal app** (Terminal, iTerm, Ghostty…). `sudo` does not replace it. `F` in the leftovers view, or `ku orphans --fda`, opens the Privacy pane.

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
- `Enter` — détail (disk / process) ; Growth : pourquoi ça a changé (fichiers vs dossiers récursifs)
- `a` — actions process (kill, kill -9, renice, inspect)
- `h` — historique process, ou fenêtre growth
- `e` — Growth : `ncdu` si disponible, sinon révéler dans Finder / explorateur Linux
- `t` — Growth : top 50 des contributions (apparitions / disparitions) ou toute la liste
- `o` — Growth : leftover apps (orphelins) ; `d` un chemin, `a` tout le groupe
- `i` / `I` — ignorer un chemin / toute l’app (allowlist dans `config.toml`)
- `u` — voir l’allowlist ; `d`/`x` retirer, `c` tout vider
- `F` — macOS : ouvrir Accès complet au disque (ajouter **le Terminal**, pas `ku`)
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
page_jump = 0           # 0 = auto (~80% of visible rows); or 5, 10, 20…

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

[orphans]
# leftover app ids or exact paths hidden from `o` / `ku orphans`
ignore = []
```

## Données

Snapshots SQLite (rétention 7 jours, max 30) :

- Linux : `~/.local/share/ku/history.db`
- macOS : `~/Library/Application Support/ku/history.db`

`sudo ku` lit et écrit ces chemins pour `SUDO_USER` (pas `/var/root` / `/root`), et rend les fichiers à cet utilisateur.

Le scan growth des `watched_paths` tourne en arrière-plan (intervalle `snapshot_interval`, 5 min par défaut). Les arbres trop larges sont bornés (profondeur, nombre d’entrées, dossiers type `node_modules` / `.git`).

## Roadmap

Voir `spec.md`. Cette version couvre le MVP (dashboard, disk, process temps réel) plus les fondations V0.2 (historique process, growth) et V0.3 (config, thèmes, alertes). Network est prévu en V0.4.
