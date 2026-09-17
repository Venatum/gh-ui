# Onglet « Actions » (runs GitHub) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ajouter un 2ᵉ onglet affichant le flux des runs GitHub Actions (multi-repo), avec un filtre « seulement mes PRs » coché par défaut et une navigation par onglets.

**Architecture:** Approche « état typé parallèle » : `App` garde `prs: Vec<Pr>` ET `runs: Vec<Run>`. Un `enum Tab` choisit l'onglet affiché ; un `enum Loaded` transporte le résultat du thread de fond dans le canal mpsc existant ; un `enum Job` dit au thread quoi charger. Aucune généricité ni trait. Le filtre « mes PRs » est une vue dérivée (filtrage local par branche), pas un re-fetch.

**Tech Stack:** Rust edition 2024, ratatui 0.30 + crossterm, serde/serde_json, anyhow, `std::thread::scope`, `std::sync::mpsc`, `std::collections::HashSet`.

**Spec:** `docs/superpowers/specs/2026-09-17-onglet-actions-design.md`

## Global Constraints

- Rust edition 2024 ; aucune nouvelle dépendance (tout est en std + deps existantes).
- Commentaires et libellés UI en **français**, ton pédagogique (projet d'apprentissage).
- `Run` suit exactement le style de `Pr` : `#[derive(Debug, Deserialize)]` + `#[serde(rename_all = "camelCase")]`, champ `repo` en `#[serde(skip)]`.
- Chaque tâche finit **compilable** (`cargo build`) avec les tests existants au vert.
- Messages de commit terminés par les 2 lignes d'attribution (voir chaque étape « Commit »).
- Après la dernière tâche : `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` doivent passer.

---

### Task 1: Champ branche sur `Pr`

**Files:**
- Modify: `src/model.rs` (struct `Pr`, + module de tests)
- Modify: `src/gh.rs:14-15` (const `JSON_FIELDS`)

**Interfaces:**
- Produces: `Pr.head_ref_name: String` (branche de la PR, ex. `feature/x`), utilisé par le filtre « mes PRs » (Task 5).

- [ ] **Step 1: Écrire le test qui échoue**

Dans `src/model.rs`, ajouter en bas du fichier :

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_deserialise_la_branche() {
        let json = r#"{
            "number": 1, "title": "t", "author": {"login": "moi"},
            "isDraft": false, "url": "u", "updatedAt": "2026-01-01T00:00:00Z",
            "additions": 0, "deletions": 0, "labels": [],
            "headRefName": "feature/x"
        }"#;
        let pr: Pr = serde_json::from_str(json).unwrap();
        assert_eq!(pr.head_ref_name, "feature/x");
    }
}
```

- [ ] **Step 2: Lancer le test pour le voir échouer**

Run: `cargo test pr_deserialise_la_branche`
Expected: FAIL à la compilation — `no field head_ref_name on type Pr`.

- [ ] **Step 3: Ajouter le champ**

Dans `src/model.rs`, struct `Pr`, après `pub labels: Vec<Label>,` (avant le champ `repo`) :

```rust
    // La branche source de la PR (ex. "feature/x"). Sert à relier une PR à ses
    // runs GitHub Actions (on croise sur cette branche dans l'onglet Actions).
    pub head_ref_name: String,
```

- [ ] **Step 4: Demander le champ à `gh`**

Dans `src/gh.rs`, remplacer la const `JSON_FIELDS` (lignes 14-15) par :

```rust
const JSON_FIELDS: &str =
    "number,title,author,reviewDecision,isDraft,url,updatedAt,additions,deletions,labels,headRefName";
```

- [ ] **Step 5: Lancer le test pour le voir passer**

Run: `cargo test pr_deserialise_la_branche`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/model.rs src/gh.rs
git commit -m "$(cat <<'EOF'
feat(model): ajoute head_ref_name (branche) à Pr

Nécessaire pour croiser une PR avec ses runs GitHub Actions.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Struct `Run`

**Files:**
- Modify: `src/model.rs` (nouvelle struct + test)

**Interfaces:**
- Produces: `struct Run { workflow_name, display_title, head_branch, status, conclusion, event, created_at, number, url: String/u64, repo: String }` — tous `pub`. `conclusion` vaut `""` si le run n'est pas terminé.

- [ ] **Step 1: Écrire le test qui échoue**

Dans le `mod tests` de `src/model.rs`, ajouter :

```rust
    #[test]
    fn run_deserialise_et_gere_conclusion_absente() {
        // "conclusion" est absent tant que le run n'est pas terminé.
        let json = r#"{
            "workflowName": "CI", "displayTitle": "corrige un bug",
            "headBranch": "feature/x", "status": "in_progress",
            "event": "push", "createdAt": "2026-01-01T00:00:00Z",
            "number": 42, "url": "u"
        }"#;
        let run: Run = serde_json::from_str(json).unwrap();
        assert_eq!(run.workflow_name, "CI");
        assert_eq!(run.head_branch, "feature/x");
        assert_eq!(run.conclusion, ""); // défaut car absent
    }
```

- [ ] **Step 2: Lancer le test pour le voir échouer**

Run: `cargo test run_deserialise`
Expected: FAIL à la compilation — `cannot find type Run`.

- [ ] **Step 3: Ajouter la struct**

Dans `src/model.rs`, après la struct `Pr` :

```rust
/// Un run GitHub Actions, tel que `gh run list --json ...` le renvoie.
/// Même style que `Pr` : `repo` est rempli par nous (pas dans le JSON).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub workflow_name: String,
    pub display_title: String,
    pub head_branch: String,
    /// "queued" | "in_progress" | "completed".
    pub status: String,
    /// "success" | "failure" | "cancelled" | "skipped"... ; "" si pas terminé.
    #[serde(default)]
    pub conclusion: String,
    /// "push" | "pull_request" | "schedule"...
    pub event: String,
    pub created_at: String,
    pub number: u64,
    pub url: String,
    #[serde(skip)]
    pub repo: String,
}
```

- [ ] **Step 4: Lancer le test pour le voir passer**

Run: `cargo test run_deserialise`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/model.rs
git commit -m "$(cat <<'EOF'
feat(model): ajoute la struct Run (run GitHub Actions)

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `gh::fetch_runs`

**Files:**
- Modify: `src/gh.rs` (import `Run`, consts `RUN_LIMIT`/`RUN_JSON_FIELDS`, fn `fetch_runs`)

**Interfaces:**
- Consumes: `model::Run`.
- Produces: `pub fn fetch_runs(repo_dir: &Path) -> Result<Vec<Run>>` — lance `gh run list --limit 20 --json ...` dans `repo_dir`.

- [ ] **Step 1: Élargir l'import du modèle**

Dans `src/gh.rs`, ligne 5, remplacer :

```rust
use crate::model::Pr;
```

par :

```rust
use crate::model::{Pr, Run};
```

- [ ] **Step 2: Ajouter les constantes**

Dans `src/gh.rs`, après la const `JSON_FIELDS` :

```rust
/// Nombre de runs récupérés par repo (derniers runs, toutes branches).
const RUN_LIMIT: &str = "20";

/// Champs JSON demandés à `gh` pour chaque run.
const RUN_JSON_FIELDS: &str =
    "workflowName,displayTitle,headBranch,status,conclusion,event,createdAt,number,url";
```

- [ ] **Step 3: Ajouter la fonction**

Dans `src/gh.rs`, après `fetch_prs` :

```rust
/// Lance `gh run list` dans `repo_dir` et parse le JSON en `Vec<Run>`.
/// Pas de filtres ici : on récupère les N derniers runs bruts ; le croisement
/// avec les PRs se fait côté App (voir `App::visible_runs`).
pub fn fetch_runs(repo_dir: &Path) -> Result<Vec<Run>> {
    let output = Command::new("gh")
        .args([
            "run",
            "list",
            "--limit",
            RUN_LIMIT,
            "--json",
            RUN_JSON_FIELDS,
        ])
        .current_dir(repo_dir)
        .output()
        .context("lancement de `gh` (est-il installé et dans le PATH ?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`gh run list` a échoué : {}", stderr.trim());
    }

    let runs: Vec<Run> = serde_json::from_slice(&output.stdout)
        .context("parsing du JSON renvoyé par `gh run list`")?;
    Ok(runs)
}
```

- [ ] **Step 4: Vérifier la compilation**

Run: `cargo build`
Expected: compile (warning « fonction jamais utilisée » toléré — elle sera branchée en Task 4).

- [ ] **Step 5: Commit**

```bash
git add src/gh.rs
git commit -m "$(cat <<'EOF'
feat(gh): ajoute fetch_runs (gh run list --json)

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Canal générique `Loaded` + couche fetch des runs

Refactor sans changement de comportement visible : le canal transporte désormais un `enum Loaded`. L'app continue de ne charger que les PRs ; la couche runs existe mais n'est pas encore déclenchée.

**Files:**
- Modify: `src/fetch.rs` (import `Run`, `RunsResult`, `Loaded`, `Job`, renommage `load`→`load_prs`, ajout `load_runs`, nouvelle signature `spawn`)
- Modify: `src/app.rs` (types du canal → `Loaded`, `refresh` passe `Job::Prs`, `on_tick` fait un `match`)

**Interfaces:**
- Produces:
  - `pub enum Job { Prs, Runs }`
  - `pub enum Loaded { Prs(FetchResult), Runs(RunsResult) }`
  - `pub struct RunsResult { runs: Vec<Run>, all_repos: Vec<String>, scanned: usize, errors: usize }`
  - `pub fn spawn(job: Job, root: PathBuf, filters: Filters, tx: Sender<Loaded>)`

- [ ] **Step 1: `fetch.rs` — imports et types**

Dans `src/fetch.rs`, élargir l'import modèle :

```rust
use crate::model::{Pr, Run};
```

et remplacer l'import du canal `use std::sync::mpsc::Sender;` (inchangé). Ajouter, après la struct `FetchResult` :

```rust
/// Miroir de `FetchResult`, mais pour les runs.
pub struct RunsResult {
    pub runs: Vec<Run>,
    pub all_repos: Vec<String>,
    pub scanned: usize,
    pub errors: usize,
}

/// Ce que le thread de fond renvoie : soit des PRs, soit des runs.
pub enum Loaded {
    Prs(FetchResult),
    Runs(RunsResult),
}

/// Ce qu'on demande au thread de charger.
pub enum Job {
    Prs,
    Runs,
}
```

- [ ] **Step 2: `fetch.rs` — nouvelle `spawn` + aiguillage**

Remplacer la fonction `spawn` par :

```rust
/// Démarre le chargement de `job` dans un thread de fond.
pub fn spawn(job: Job, root: PathBuf, filters: Filters, tx: Sender<Loaded>) {
    thread::spawn(move || {
        let result = match job {
            Job::Prs => Loaded::Prs(load_prs(&root, &filters)),
            Job::Runs => Loaded::Runs(load_runs(&root, &filters)),
        };
        let _ = tx.send(result);
    });
}
```

- [ ] **Step 3: `fetch.rs` — renommer `load` en `load_prs`**

Renommer la fonction `load` existante en `load_prs` (signature et corps inchangés, seul le nom change).

- [ ] **Step 4: `fetch.rs` — ajouter `load_runs`**

Après `load_prs`, ajouter le miroir pour les runs (même logique `thread::scope`, mais `gh::fetch_runs` ne prend pas de filtres) :

```rust
/// Comme `load_prs`, mais pour les runs. Le filtre repo (s'il est posé)
/// restreint aussi les repos scannés ici, par cohérence avec l'onglet PRs.
fn load_runs(root: &Path, filters: &Filters) -> RunsResult {
    let all_repos = gh::discover_repos(root).unwrap_or_default();

    let to_scan: Vec<&String> = match &filters.repo {
        Some(sel) => all_repos.iter().filter(|r| *r == sel).collect(),
        None => all_repos.iter().collect(),
    };

    let joined = thread::scope(|scope| {
        let handles: Vec<_> = to_scan
            .iter()
            .map(|&repo| scope.spawn(move || (repo.clone(), gh::fetch_runs(&root.join(repo)))))
            .collect();
        handles.into_iter().map(|h| h.join()).collect::<Vec<_>>()
    });

    let mut runs = Vec::new();
    let mut errors = 0;
    for result in joined {
        match result {
            Ok((repo, Ok(mut list))) => {
                for run in &mut list {
                    run.repo = repo.clone();
                }
                runs.extend(list);
            }
            Ok((_, Err(_))) | Err(_) => errors += 1,
        }
    }

    RunsResult {
        runs,
        scanned: to_scan.len(),
        errors,
        all_repos,
    }
}
```

- [ ] **Step 5: `app.rs` — types du canal**

Dans `src/app.rs`, remplacer l'import :

```rust
use crate::fetch::{self, FetchResult};
```

par :

```rust
use crate::fetch::{self, Job, Loaded};
```

et les deux champs de `App` :

```rust
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
```

- [ ] **Step 6: `app.rs` — `refresh` passe un `Job`**

Dans `refresh`, remplacer l'appel `fetch::spawn(...)` par :

```rust
        fetch::spawn(Job::Prs, self.root.clone(), self.filters.clone(), self.tx.clone());
```

- [ ] **Step 7: `app.rs` — `on_tick` fait un `match`**

Dans `on_tick`, remplacer la boucle `while let Ok(result) = self.rx.try_recv() { ... }` par un `match` sur `Loaded` (le corps PRs est l'existant ; l'arm Runs est un placeholder rempli en Task 5) :

```rust
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Loaded::Prs(result) => {
                    self.prs = result.prs;
                    self.repos = result.all_repos;
                    self.loading = false;
                    changed = true;

                    let errors = if result.errors > 0 {
                        format!(" — {} en erreur", result.errors)
                    } else {
                        String::new()
                    };
                    self.status = format!(
                        "{} PR(s) — {} repo(s){}",
                        self.prs.len(),
                        result.scanned,
                        errors
                    );

                    self.table_state
                        .select(if self.prs.is_empty() { None } else { Some(0) });
                }
                Loaded::Runs(_) => {} // rempli en Task 5
            }
        }
```

- [ ] **Step 8: Vérifier build + tests existants**

Run: `cargo build && cargo test`
Expected: build OK ; tous les tests existants (filtres, model) PASS. Comportement identique à avant (seules les PRs se chargent).

- [ ] **Step 9: Commit**

```bash
git add src/fetch.rs src/app.rs
git commit -m "$(cat <<'EOF'
refactor(fetch): canal générique Loaded + couche fetch des runs

Le thread de fond renvoie désormais un enum Loaded (Prs|Runs) et prend un
Job. Comportement inchangé : seules les PRs sont encore déclenchées.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: État onglets + filtre « mes PRs » (logique App)

**Files:**
- Modify: `src/app.rs` (enum `Tab`, nouveaux champs, `set_tab`/`next_tab`, `filter_runs`, `visible_runs`, `toggle_only_pr_runs`, navigation tab-aware, `selected_url`, `on_tick` arm Runs, `refresh` par onglet)

**Interfaces:**
- Consumes: `fetch::Job`, `fetch::Loaded`, `model::Run`.
- Produces (tous `pub` sauf `filter_runs`) :
  - `pub enum Tab { Prs, Runs }` + `pub fn next(self) -> Tab`
  - champs `active_tab: Tab`, `runs: Vec<Run>`, `run_table_state: TableState`, `only_pr_runs: bool`, `runs_loaded: bool`
  - `pub fn set_tab(&mut self, tab: Tab)`, `pub fn next_tab(&mut self)`
  - `pub fn visible_runs(&self) -> Vec<&Run>`
  - `pub fn toggle_only_pr_runs(&mut self)`
  - `pub fn selected_url(&self) -> Option<String>`
  - `next`/`previous` deviennent tab-aware (agissent sur l'onglet actif)

- [ ] **Step 1: Écrire les tests qui échouent**

Dans un `#[cfg(test)] mod tests` en bas de `src/app.rs` (créer si absent) :

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Run;

    fn run(branch: &str) -> Run {
        Run {
            workflow_name: "CI".into(),
            display_title: "t".into(),
            head_branch: branch.into(),
            status: "completed".into(),
            conclusion: "success".into(),
            event: "push".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            number: 1,
            url: "u".into(),
            repo: "r".into(),
        }
    }

    #[test]
    fn tab_cycle() {
        assert_eq!(Tab::Prs.next(), Tab::Runs);
        assert_eq!(Tab::Runs.next(), Tab::Prs);
    }

    #[test]
    fn filtre_runs_par_branches_de_pr() {
        let runs = vec![run("feature/x"), run("main"), run("feature/y")];
        let mut branches = std::collections::HashSet::new();
        branches.insert("feature/x");
        branches.insert("feature/y");

        // only = true : on ne garde que les runs sur une branche de PR.
        let kept = filter_runs(&runs, &branches, true);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|r| r.head_branch != "main"));

        // only = false : on garde tout.
        assert_eq!(filter_runs(&runs, &branches, false).len(), 3);
    }
}
```

- [ ] **Step 2: Lancer les tests pour les voir échouer**

Run: `cargo test --lib app::tests`
Expected: FAIL à la compilation (`Tab`, `filter_runs` inexistants).

- [ ] **Step 3: Ajouter `Tab` et l'import `HashSet`**

En haut de `src/app.rs`, après les `use` existants :

```rust
use std::collections::HashSet;
```

Après l'enum `FilterField` (ou près des autres enums) :

```rust
/// Les onglets de l'application.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    Prs,
    Runs,
}

impl Tab {
    /// L'onglet suivant (cycle Prs → Runs → Prs).
    pub fn next(self) -> Tab {
        match self {
            Tab::Prs => Tab::Runs,
            Tab::Runs => Tab::Prs,
        }
    }
}
```

- [ ] **Step 4: Ajouter les champs à `App` + les initialiser**

Dans la struct `App`, ajouter :

```rust
    pub active_tab: Tab,
    pub runs: Vec<Run>,
    pub run_table_state: TableState,
    /// Filtre « seulement les runs des branches de mes PRs ». Coché par défaut.
    pub only_pr_runs: bool,
    /// A-t-on déjà chargé les runs au moins une fois ?
    runs_loaded: bool,
```

Ajouter l'import `use crate::model::{Pr, Run};` (élargir l'import `Pr` existant). Dans `App::new`, initialiser :

```rust
            active_tab: Tab::Prs,
            runs: Vec::new(),
            run_table_state: TableState::default(),
            only_pr_runs: true,
            runs_loaded: false,
```

- [ ] **Step 5: Ajouter `filter_runs` + `visible_runs` + `toggle`**

En bas de `src/app.rs`, hors du `impl App` (fonction libre, pure, testable) :

```rust
/// Garde les runs dont la branche est dans `pr_branches`, ou tous si `!only`.
fn filter_runs<'a>(runs: &'a [Run], pr_branches: &HashSet<&str>, only: bool) -> Vec<&'a Run> {
    if !only {
        return runs.iter().collect();
    }
    runs.iter()
        .filter(|r| pr_branches.contains(r.head_branch.as_str()))
        .collect()
}
```

Dans `impl App`, ajouter :

```rust
    /// La vue des runs affichée : filtrée sur les branches de PRs si coché.
    pub fn visible_runs(&self) -> Vec<&Run> {
        let branches: HashSet<&str> = self.prs.iter().map(|p| p.head_ref_name.as_str()).collect();
        filter_runs(&self.runs, &branches, self.only_pr_runs)
    }

    /// Bascule le filtre « mes PRs » (pas de re-fetch : filtrage local).
    pub fn toggle_only_pr_runs(&mut self) {
        self.only_pr_runs = !self.only_pr_runs;
        // La sélection peut sortir de la vue → on la remet au début si besoin.
        let n = self.visible_runs().len();
        self.run_table_state
            .select(if n == 0 { None } else { Some(0) });
    }
```

- [ ] **Step 6: Navigation par onglets**

Dans `impl App`, ajouter :

```rust
    pub fn set_tab(&mut self, tab: Tab) {
        self.active_tab = tab;
        // Premier passage sur Actions → on charge les runs.
        if tab == Tab::Runs && !self.runs_loaded {
            self.refresh();
        }
    }

    pub fn next_tab(&mut self) {
        self.set_tab(self.active_tab.next());
    }
```

- [ ] **Step 7: `refresh` charge l'onglet actif**

Dans `refresh`, remplacer la ligne `fetch::spawn(Job::Prs, ...)` par :

```rust
        let job = match self.active_tab {
            Tab::Prs => Job::Prs,
            Tab::Runs => Job::Runs,
        };
        fetch::spawn(job, self.root.clone(), self.filters.clone(), self.tx.clone());
```

- [ ] **Step 8: Remplir l'arm `Loaded::Runs` de `on_tick`**

Dans `on_tick`, remplacer `Loaded::Runs(_) => {}` par :

```rust
                Loaded::Runs(result) => {
                    self.runs = result.runs;
                    self.repos = result.all_repos;
                    self.runs_loaded = true;
                    self.loading = false;
                    changed = true;

                    let errors = if result.errors > 0 {
                        format!(" — {} en erreur", result.errors)
                    } else {
                        String::new()
                    };
                    let n = self.visible_runs().len();
                    self.status = format!("{n} run(s) — {} repo(s){}", result.scanned, errors);
                    self.run_table_state
                        .select(if n == 0 { None } else { Some(0) });
                }
```

- [ ] **Step 9: Navigation tab-aware + `selected_url`**

Remplacer `next`, `previous` et `selected_pr` par des versions qui tiennent compte de l'onglet actif :

```rust
    pub fn next(&mut self) {
        let (state, len) = match self.active_tab {
            Tab::Prs => (&mut self.table_state, self.prs.len()),
            Tab::Runs => (&mut self.run_table_state, self.visible_runs().len()),
        };
        if len == 0 {
            return;
        }
        let i = match state.selected() {
            Some(i) => (i + 1).min(len - 1),
            None => 0,
        };
        state.select(Some(i));
    }

    pub fn previous(&mut self) {
        let (state, len) = match self.active_tab {
            Tab::Prs => (&mut self.table_state, self.prs.len()),
            Tab::Runs => (&mut self.run_table_state, self.visible_runs().len()),
        };
        if len == 0 {
            return;
        }
        let i = match state.selected() {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        state.select(Some(i));
    }

    /// L'URL de l'élément sélectionné dans l'onglet actif (PR ou run).
    pub fn selected_url(&self) -> Option<String> {
        match self.active_tab {
            Tab::Prs => self
                .table_state
                .selected()
                .and_then(|i| self.prs.get(i))
                .map(|pr| pr.url.clone()),
            Tab::Runs => self
                .run_table_state
                .selected()
                .and_then(|i| self.visible_runs().get(i).map(|r| r.url.clone())),
        }
    }
```

> Note : `selected_pr` est supprimée ; `main.rs` (Task 7) utilisera `selected_url`.

- [ ] **Step 10: Lancer les tests**

Run: `cargo test --lib app::tests`
Expected: `tab_cycle` et `filtre_runs_par_branches_de_pr` PASS.

- [ ] **Step 11: Vérifier le build global**

Run: `cargo build`
Expected: échoue probablement dans `ui.rs`/`main.rs` (`selected_pr` disparue, pas encore d'onglet Runs affiché). **C'est attendu** — Tasks 6 et 7 réparent. Si tu veux un point de commit propre, commit maintenant le seul `app.rs` (les tests de la lib passent isolément avec `cargo test --lib`).

- [ ] **Step 12: Commit**

```bash
git add src/app.rs
git commit -m "$(cat <<'EOF'
feat(app): état onglets (Tab) + filtre "mes PRs" (runs)

Tab enum, champs runs/active_tab/only_pr_runs, navigation tab-aware,
visible_runs (filtrage local par branche), selected_url.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Rendu de l'onglet Actions (UI)

**Files:**
- Modify: `src/ui.rs` (import `Run`/`Tab`, `run_look` + test, `run_to_row`, barre d'onglets, dispatch `render_table`, entête/pied)

**Interfaces:**
- Consumes: `App.active_tab`, `App.visible_runs()`, `App.run_table_state`, `App.only_pr_runs`.
- Produces: rendu de la table runs quand `active_tab == Tab::Runs` ; barre d'onglets `[ PRs ] [ Actions ]`.

- [ ] **Step 1: Écrire le test qui échoue**

Dans `src/ui.rs`, ajouter un `#[cfg(test)] mod tests` :

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_look_couleurs() {
        assert_eq!(run_look("completed", "success").0, "✓ success");
        assert_eq!(run_look("completed", "failure").0, "✗ failure");
        assert_eq!(run_look("completed", "cancelled").0, "cancelled");
        assert_eq!(run_look("in_progress", "").0, "● running");
        assert_eq!(run_look("queued", "").0, "queued");
    }
}
```

- [ ] **Step 2: Lancer le test pour le voir échouer**

Run: `cargo test --lib ui::tests`
Expected: FAIL à la compilation (`run_look` inexistant).

- [ ] **Step 3: Élargir les imports**

Dans `src/ui.rs`, ajouter à l'import modèle et à l'import app :

```rust
use crate::app::{App, FILTER_FIELDS, FilterField, InputKind, Tab};
use crate::model::{Pr, Run};
```

- [ ] **Step 4: Ajouter `run_look` + `run_to_row`**

Après `review_look` dans `src/ui.rs` :

```rust
/// Libellé + couleur d'un run selon (status, conclusion).
fn run_look(status: &str, conclusion: &str) -> (&'static str, Style) {
    match (status, conclusion) {
        ("completed", "success") => ("✓ success", Style::new().fg(Color::Green)),
        ("completed", "failure") => ("✗ failure", Style::new().fg(Color::Red)),
        ("completed", "cancelled") => ("cancelled", Style::new().fg(Color::DarkGray)),
        ("completed", "skipped") => ("skipped", Style::new().fg(Color::DarkGray)),
        ("in_progress", _) => ("● running", Style::new().fg(Color::Yellow)),
        ("queued", _) => ("queued", Style::new().fg(Color::Gray)),
        _ => ("-", Style::new().fg(Color::DarkGray)),
    }
}

fn run_to_row(run: &Run) -> Row<'_> {
    let date = run.created_at.get(..10).unwrap_or("").to_string();
    let (status_label, status_style) = run_look(&run.status, &run.conclusion);

    Row::new(vec![
        Cell::from(run.repo.clone()),
        Cell::from(date),
        Cell::from(run.workflow_name.clone()),
        Cell::from(run.head_branch.clone()),
        Cell::from(run.event.clone()),
        Cell::from(Span::styled(status_label, status_style)),
        Cell::from(run.display_title.clone()),
    ])
}
```

- [ ] **Step 5: Dispatch dans `render_table`**

Remplacer le corps de `render_table` pour choisir la table selon l'onglet. Extraire l'existant PRs dans `render_pr_table` et ajouter `render_run_table` :

```rust
fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.active_tab {
        Tab::Prs => render_pr_table(frame, app, area),
        Tab::Runs => render_run_table(frame, app, area),
    }
}
```

Garder le corps actuel de `render_table` sous le nom `render_pr_table` (signature identique). Ajouter :

```rust
fn render_run_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new(["repo", "created", "workflow", "branch", "event", "status", "title"])
        .style(Style::new().bold());

    let runs = app.visible_runs();
    let rows: Vec<Row> = runs.iter().map(|r| run_to_row(r)).collect();

    let widths = [
        Constraint::Length(16),
        Constraint::Length(10),
        Constraint::Length(18),
        Constraint::Length(22),
        Constraint::Length(14),
        Constraint::Length(11),
        Constraint::Fill(1),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("▌ ")
        .block(Block::bordered());

    frame.render_stateful_widget(table, area, &mut app.run_table_state);
}
```

- [ ] **Step 6: Barre d'onglets dans l'entête**

Dans `render_header`, ajouter une ligne d'onglets. Après la construction de `top` et avant `filters`, insérer une ligne `tabs` et l'inclure dans le `Paragraph`. Remplacer la fin de `render_header` par :

```rust
    // Ligne onglets : [ PRs ] [ Actions ], l'actif surligné.
    let tab_span = |label: &str, active: bool| {
        if active {
            Span::styled(
                format!(" {label} "),
                Style::new().fg(Color::Black).bg(Color::Cyan).bold(),
            )
        } else {
            Span::styled(format!(" {label} "), Style::new().fg(Color::DarkGray))
        }
    };
    let tabs = Line::from(vec![
        tab_span("PRs", app.active_tab == Tab::Prs),
        Span::raw(" "),
        tab_span("Actions", app.active_tab == Tab::Runs),
    ]);

    // Ligne 2 : selon l'onglet, résumé des filtres PRs OU état du toggle runs.
    let subtitle = match app.active_tab {
        Tab::Prs => Span::styled(app.filters.summary(), Style::new().fg(Color::DarkGray)),
        Tab::Runs => Span::styled(
            format!(
                "mes PRs : {}   (m pour basculer)",
                if app.only_pr_runs { "[x]" } else { "[ ]" }
            ),
            Style::new().fg(Color::DarkGray),
        ),
    };

    let header =
        Paragraph::new(vec![Line::from(top), tabs, Line::from(subtitle)]).block(Block::bordered());
    frame.render_widget(header, area);
```

Passer la hauteur de l'entête de 4 à 5 lignes : dans `render`, `Constraint::Length(4)` → `Constraint::Length(5)`.

- [ ] **Step 7: Lancer les tests + build**

Run: `cargo test --lib ui::tests && cargo build`
Expected: `run_look_couleurs` PASS. Le build peut encore échouer sur `main.rs` (`selected_pr`), réparé en Task 7.

- [ ] **Step 8: Commit**

```bash
git add src/ui.rs
git commit -m "$(cat <<'EOF'
feat(ui): table des runs + barre d'onglets + run_look

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Clavier & câblage final (main.rs)

**Files:**
- Modify: `src/main.rs` (touches `Tab`/`1`/`2`/`m`, `open_selected` via `selected_url`)
- Modify: `src/ui.rs` (pied : mention onglets) et `render_help` (raccourcis)

**Interfaces:**
- Consumes: `App.next_tab`, `App.set_tab`, `App.toggle_only_pr_runs`, `App.selected_url`, `Tab`.

- [ ] **Step 1: Import `Tab` dans main**

Dans `src/main.rs`, ajouter à l'import app :

```rust
use app::{App, Tab};
```

- [ ] **Step 2: Touches onglets en mode normal**

Dans `handle_normal_key`, ajouter les bras (avant le `_ => {}`) :

```rust
        KeyCode::Tab => app.next_tab(),
        KeyCode::Char('1') => app.set_tab(Tab::Prs),
        KeyCode::Char('2') => app.set_tab(Tab::Runs),
        KeyCode::Char('m') => app.toggle_only_pr_runs(),
```

> `m` n'a d'effet visible que sur l'onglet Actions ; sur PRs il bascule un booléen ignoré par le rendu, sans conséquence.

- [ ] **Step 3: `open_selected` via `selected_url`**

Remplacer `open_selected` :

```rust
fn open_selected(app: &App) {
    if let Some(url) = app.selected_url() {
        open_url(&url);
    }
}
```

- [ ] **Step 4: Pied + aide**

Dans `src/ui.rs`, `render_footer`, ajouter un indice onglets dans le tableau `hints` (après `("↑↓", "nav")`) :

```rust
        ("tab/1/2", "onglet"),
```

Dans `render_help`, ajouter après la ligne `help_row("f", ...)` :

```rust
        help_row("tab, 1/2", "changer d'onglet (PRs / Actions)"),
        help_row("m", "onglet Actions : filtrer sur mes PRs"),
```

- [ ] **Step 5: Build + lint + format + tests**

Run: `cargo build && cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: tout PASS, zéro warning clippy, format OK. Si `cargo fmt --check` signale des diffs, lancer `cargo fmt` puis re-vérifier.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/ui.rs
git commit -m "$(cat <<'EOF'
feat(main): navigation onglets (tab/1/2), toggle m, ouverture par onglet

Câble l'onglet Actions de bout en bout : bascule PRs/Actions, filtre "mes
PRs" (m), ouverture de la PR ou du run sélectionné.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review (fait à la rédaction)

**Couverture spec :** §2 décisions → Task 1 (Q2 branche), Task 3 (Q3 limit 20), Task 5 (Q2 toggle défaut, Q4 nav), Task 6 (barre onglets Q4), Task 7 (touches Q4 + m). §3.1 model → Tasks 1-2. §3.2 gh → Task 3. §3.3 fetch/Loaded/Job → Task 4. §3.4-3.5 état/logique → Task 5. §3.6 ui → Task 6. §3.7 clavier → Task 7. §4 erreurs → arm errors dans load_runs (Task 4) + on_tick (Task 5). §5 tests → Task 1/2 (serde), Task 5 (filter_runs, Tab), Task 6 (run_look). ✅ Aucun trou.

**Placeholders :** l'arm `Loaded::Runs(_) => {}` de Task 4 est volontaire et explicitement remplacé en Task 5 (pas un TODO orphelin). Aucun autre.

**Cohérence des types :** `Loaded`/`Job`/`RunsResult` (Task 4) réutilisés à l'identique en Task 5. `filter_runs(&[Run], &HashSet<&str>, bool)` défini et testé en Task 5. `run_look(&str,&str)` défini et testé en Task 6. `selected_url` (Task 5) remplace `selected_pr` et est consommé en Task 7. ✅
