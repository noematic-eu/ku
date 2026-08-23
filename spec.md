**Spécification détaillée du projet**

**Nom provisoire :** `ku`  
**Description :** TUI de monitoring système avancé pour administrateurs, multiplateforme (Linux + macOS), écrite en Rust avec `ratatui`.

---

### 1. Objectif

Fournir un outil de monitoring système **rapide, léger et orienté administrateur** qui va au-delà des classiques `htop`/`btop`/`glances` en ajoutant :

- Suivi de l’espace disque en temps réel + historique
- Évolution de l’usage des fichiers et dossiers (growth tracking)
- Analyse des processus les plus actifs **dans le temps** (pas seulement instantané)
- Interface purement clavier, claire et dense en informations

---

### 2. Plateformes supportées

- **Linux** (principale) : distributions modernes (kernel ≥ 5.10 recommandé)
- **macOS** (Intel + Apple Silicon)
- Support initial des fichiersystems courants :
  - Linux : ext4, xfs, btrfs, zfs, tmpfs, nfs…
  - macOS : APFS, HFS+, volumes Time Machine

---

### 3. Stack technique

| Composant              | Choix                          | Justification |
|------------------------|--------------------------------|-------------|
| Langage                | **Rust**                       | Performance, sécurité mémoire, excellent écosystème système |
| TUI                    | **ratatui** + **crossterm**    | Moderne, performant, bien maintenu |
| Collecte système       | `sysinfo` + bindings natifs    | Multiplateforme |
| Disque / fichiers      | `sysinfo` + `walkdir` + snapshots locaux | |
| Historique             | **SQLite** (via `rusqlite`)    | Léger, fiable, pas de serveur |
| Async / timers         | `tokio`                        | Collecte non-bloquante |
| Configuration          | `toml` + `dirs`                | Simple et standard |
| Logging                | `tracing`                      | |

**Dépendances principales prévues :**
- `ratatui`, `crossterm`
- `sysinfo`
- `tokio`
- `rusqlite`
- `serde` + `toml`
- `chrono`
- `clap` (CLI)
- `anyhow` / `thiserror`

---

### 4. Fonctionnalités (MVP → V1)

#### 4.1 Vue Globale (Dashboard)
- CPU (global + par cœur)
- Mémoire (utilisée / disponible / swap)
- Load average
- Uptime
- Nombre de process
- Alertes actives (disque plein, process zombies, etc.)

#### 4.2 Espace disque
- Liste de tous les volumes/partitions avec :
  - Point de montage
  - Filesystem
  - Taille totale / utilisée / libre
  - Pourcentage + barre de progression colorée
  - Inodes (si pertinent)
- Seuils d’alerte configurables (ex. 80 %, 90 %, 95 %)
- Tri et filtrage
- Détail d’un volume (double-clic / Entrée)

#### 4.3 Évolution de l’usage fichiers/dossiers (Growth Tracking)
- Snapshots périodiques de l’occupation disque
- Top des dossiers qui ont le **plus grandi** ou **rétréci** sur :
  - 1 heure
  - 6 heures
  - 24 heures
  - 7 jours
- Possibilité de surveiller des chemins spécifiques (configurables)
- Affichage de la variation absolue (+X Go) et relative (+Y %)
- Vue historique simple (graphique ASCII ou sparklines)

#### 4.4 Processus
- Liste classique (PID, user, CPU%, MEM%, command…)
- **Mode historique** :
  - Top process par CPU / mémoire / I/O sur différentes fenêtres de temps
  - Process qui ont consommé le plus depuis le démarrage de `ku`
  - Détection des process qui montent progressivement (memory leak suspects)
- Actions : kill, kill -9, renice, inspect (fichiers ouverts, cwd, etc.)
- Filtrage puissant (par user, nom, CPU > X, etc.)

#### 4.5 Autres vues (V1)
- Réseau (interfaces, débit, connexions)
- Journal / logs récents (optionnel)
- Alertes et notifications (bandeau + historique)

---

### 5. Interface utilisateur

**Principes de design :**
- Navigation 100 % clavier (inspirée de `k9s`, `lazygit`, `btop`)
- Layouts adaptatifs selon la taille du terminal
- Thèmes clairs et sombres (configurables)
- Densité d’information élevée mais lisible
- Mode « focus » pour zoomer sur une section

**Raccourcis principaux (propositions) :**
- `Tab` / `Shift+Tab` : changer de vue
- `j/k` ou flèches : navigation
- `/` : recherche / filtre
- `Enter` : détail
- `?` : aide
- `q` : quitter
- `r` : refresh forcé
- `s` : tri
- `a` : actions sur process

**Structure des écrans :**
1. Dashboard
2. Disk
3. Growth (évolution fichiers)
4. Processes
5. Network (V1)
6. Settings / Config
7. Help

---

### 6. Collecte de données & Historique

- Collecte en arrière-plan via `tokio` (intervalles configurables)
- Snapshots disque et process stockés dans une base SQLite locale
- Rétention configurable (ex. 7 jours par défaut, 30 jours max)
- Emplacement des données :
  - Linux : `~/.local/share/ku/`
  - macOS : `~/Library/Application Support/ku/`

---

### 7. Configuration

Fichier `config.toml` :

```toml
[general]
refresh_interval = 2          # secondes
theme = "dark"
history_retention_days = 7

[disk]
warning_threshold = 80
critical_threshold = 90
watched_paths = [
  "/var/log",
  "/home",
  "/var/lib/docker"
]

[processes]
history_window = ["1m", "5m", "1h", "24h"]
```

---

### 8. Architecture logicielle (haut niveau)

```
ku/
├── src/
│   ├── main.rs
│   ├── app.rs              # État global de l’application
│   ├── ui/                 # Widgets ratatui
│   │   ├── dashboard.rs
│   │   ├── disk.rs
│   │   ├── growth.rs
│   │   ├── processes.rs
│   │   └── ...
│   ├── collector/          # Collecte de métriques
│   │   ├── cpu.rs
│   │   ├── memory.rs
│   │   ├── disk.rs
│   │   ├── process.rs
│   │   └── growth.rs
│   ├── storage/            # SQLite + snapshots
│   ├── config.rs
│   └── utils.rs
├── Cargo.toml
└── README.md
```

---

### 9. Roadmap proposée

| Phase       | Contenu                                      | Priorité |
|-------------|----------------------------------------------|----------|
| **MVP**     | Dashboard + Disk + Processes (temps réel)   | Haute    |
| **V0.2**    | Historique process + Growth tracking basique| Haute    |
| **V0.3**    | Configuration, thèmes, alertes              | Moyenne  |
| **V0.4**    | Network + améliorations UX                  | Moyenne  |
| **V1.0**    | Stabilité, packaging (Homebrew, AUR, etc.)  | —        |

---

### 10. Non-objectifs (pour l’instant)

- Monitoring distant (SSH / agents)
- Graphiques vectoriels complexes
- Support Windows
- Alerting externe (email, Slack…)
- Plugin system

---

