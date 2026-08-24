# ku

```
 _           
| | ___ _   _ 
| |/ / | | | |
|   <| |_| |
|_|\_\ \__,_|
htop + df + ncdu
```

TUI d’administrateur, Linux et macOS. CPU, mémoire, volumes et process en direct — plus ce que `htop` ne fait pas. **Growth** montre quels dossiers ont grandi ou disparu, et *pourquoi* (fichier vs arbre récursif). **Orphans** liste les restes d’apps désinstallées. Écrit en Rust avec [ratatui](https://ratatui.rs).

![Dashboard](docs/screenshots/dash.png)

![Disk](docs/screenshots/disk.png)

![Growth tracking](docs/screenshots/growth.png)

## Installer / lancer

```bash
cargo install --path .
ku
```

```bash
cargo run --release
```

```bash
ku --config /chemin/config.toml
ku --dump-config
ku --once                 # un snapshot texte, sans TUI
sudo ku                   # mêmes dossiers config/data que SUDO_USER (pas root)
```

Leftovers d’apps (dry-run par défaut) :

```bash
ku orphans
ku orphans --json
sudo ku orphans           # aussi /Library et /var/lib si lisibles
ku orphans --rm PATH      # supprime un chemin (demande confirmation)
ku orphans --rm APP_ID --all
ku orphans --ignore APP_OR_PATH
ku orphans --fda          # macOS : ouvre Accès complet au disque
```

Sur macOS, supprimer sous `~/Library/Containers` (et assimilés) demande **l’accès complet au disque** pour **l’app terminal** (Terminal, iTerm, Ghostty…), pas pour le binaire `ku`. `sudo` ne contourne pas ça. `F` dans la vue leftovers, ou `ku orphans --fda`, ouvre le panneau Confidentialité.

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
- `Enter` — détail ; Growth : pourquoi ça a changé (fichier vs dossier récursif)
- `e` — Growth : `ncdu` si installé, sinon Finder / explorateur
- `t` — Growth : top 50 des contributions, ou toute la liste
- `o` — leftovers d’apps ; `d` un chemin, `a` tout le groupe
- `i` / `I` — ignorer un chemin / toute l’app
- `F` — macOS : Accès complet au disque
- `a` — actions process (kill, kill -9, renice, inspect)
- `h` — historique process, ou fenêtre growth
- `r` — recharger
- `q` — quitter
- souris — onglets, config, lignes (double-clic = détail), molette

## Configuration

Fichier par défaut (celui de l’utilisateur qui lance `ku`, y compris via `sudo`) :

- Linux : `~/.config/ku/config.toml`
- macOS : `~/Library/Application Support/ku/config.toml`

```toml
[general]
refresh_interval = 2
theme = "dark"          # dark | light
history_retention_days = 7
page_jump = 0           # 0 = auto (~80% of visible rows)

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
ignore = []
```

## Données

Snapshots SQLite (rétention 7 jours, max 30) :

- Linux : `~/.local/share/ku/history.db`
- macOS : `~/Library/Application Support/ku/history.db`

`sudo ku` lit et écrit ces chemins pour `SUDO_USER` (pas `/var/root` / `/root`).

Le scan growth des `watched_paths` tourne en arrière-plan (intervalle `snapshot_interval`, 5 min par défaut).

## Soutenir

Développé par [Noematic](https://github.com/noematic-eu).  
Soutenir le projet : [payhip.com/b/pVwaY](https://payhip.com/b/pVwaY)

## Licence

[MIT](LICENSE)
