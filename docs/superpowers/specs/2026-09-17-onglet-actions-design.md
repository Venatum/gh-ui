# Design — Onglet « Actions » (runs GitHub) dans gh-prs

Date : 2026-09-17
Statut : validé (brainstorming), à relire avant le plan d'implémentation

## 1. Objectif

Ajouter un 2ᵉ onglet à `gh-prs` affichant le **flux des runs GitHub Actions**
(comme l'onglet Actions de GitHub), multi-repo, avec un filtre « seulement les
runs liés à mes PRs » **coché par défaut**.

L'app passe ainsi d'une « liste de PRs » à un petit **dashboard `gh`** à
onglets, sans casser l'existant.

## 2. Décisions issues du brainstorming

| # | Décision |
|---|----------|
| Q1 | Vue = **flux de runs** (tous les runs récents), pas « santé CI du main ». |
| Q2 | Filtre **« seulement mes PRs »** = toggle dédié, **coché par défaut**. Lien PR↔run par la **branche** (`headBranch` du run ↔ `headRefName` de la PR). |
| Q3 | Filtre décoché → **20 derniers runs par repo**, toutes branches, triés par date. `gh run list --limit 20`. Pas de filtre statut/date pour l'instant (YAGNI). |
| Q4 | Navigation onglets : **`Tab`** cycle **+ `1`/`2`** sautent. Barre d'onglets en haut, actif surligné. |
| Abstraction | Approche ② : **état typé parallèle + enums**. `App` garde `prs: Vec<Pr>` ET `runs: Vec<Run>` ; un `enum Tab` choisit l'affiché ; un `enum Loaded` transporte le résultat du thread. Pas de trait, pas de généricité (réservés à un éventuel 3ᵉ onglet). |

## 3. Architecture (approche ②)

Aucune généricité. On ajoute deux `enum` comme machine à états, et on double
les chemins concrets (fetch + rendu) via `match tab`.

### 3.1 `model.rs`

- **`Pr`** : ajouter le champ `head_ref_name: String` (`#[serde(rename)]` via le
  `rename_all = "camelCase"` déjà présent → `headRefName`). Sert à connaître la
  branche d'une PR pour le croisement.
  - Ajouter `headRefName` à `JSON_FIELDS` dans `gh.rs`.
- **`Run`** (nouvelle struct, `#[serde(rename_all = "camelCase")]`) :
  | Champ Rust | Champ JSON `gh run list` |
  |---|---|
  | `workflow_name: String` | `workflowName` |
  | `display_title: String` | `displayTitle` |
  | `head_branch: String` | `headBranch` |
  | `status: String` | `status` (`queued`/`in_progress`/`completed`) |
  | `conclusion: String` (`#[serde(default)]`) | `conclusion` (vide si pas fini) |
  | `event: String` | `event` |
  | `created_at: String` | `createdAt` |
  | `number: u64` | `number` |
  | `url: String` | `url` |
  | `repo: String` (`#[serde(skip)]`) | rempli côté app comme pour `Pr` |

### 3.2 `gh.rs`

- Ajouter `fetch_runs(repo_dir: &Path) -> Result<Vec<Run>>` :
  `gh run list --limit 20 --json workflowName,displayTitle,headBranch,status,conclusion,event,createdAt,number,url`,
  parse `serde_json::from_slice`. Même structure que `fetch_prs`.
- Constantes dédiées : `RUN_LIMIT`, `RUN_JSON_FIELDS`.
- **Le filtre « mes PRs » ne se fait PAS ici** : on récupère les 20 runs bruts.
  Le croisement branche se fait côté `App` (voir 3.5), pour qu'un simple
  toggle n'exige pas un re-fetch.

### 3.3 `fetch.rs`

Le canal transporte désormais deux formes de résultat via un enum :

```rust
pub enum Loaded {
    Prs(FetchResult),   // struct existante
    Runs(RunsResult),   // nouvelle : { runs, all_repos, scanned, errors }
}
```

- `Sender<Loaded>` / `Receiver<Loaded>` à la place de `Sender<FetchResult>`.
- Le thread choisit quoi faire selon un **`enum Job { Prs, Runs }`** passé à
  `spawn(job, root, filters, tx)`. Le `load` en `thread::scope` (parallèle par
  repo) est réutilisé : une variante appelle `fetch_prs`, l'autre `fetch_runs`.
- `RunsResult` = miroir de `FetchResult` mais avec `runs: Vec<Run>`.

### 3.4 `app.rs` — état

Ajouts :

```rust
enum Tab { Prs, Runs }          // onglet actif

active_tab: Tab,                 // défaut Prs
runs: Vec<Run>,                  // tous les runs récupérés (bruts, non filtrés)
run_table_state: TableState,     // sélection propre à l'onglet Actions
only_pr_runs: bool,              // défaut true
runs_loaded: bool,               // a-t-on déjà chargé les runs ?
```

Le canal devient `Sender/Receiver<Loaded>`.

### 3.5 `app.rs` — logique

- **`refresh()`** lance le fetch de **l'onglet actif** (`Job::Prs` ou
  `Job::Runs`). Le flag `pending_refresh` et l'auto-refresh restent identiques,
  appliqués à l'onglet courant.
- **Changement d'onglet** (`set_tab`) : bascule `active_tab` ; si l'onglet cible
  n'a jamais été chargé (`!runs_loaded` pour Actions), déclenche un `refresh()`.
- **`on_tick()`** : `match` sur `Loaded` reçu → range dans `prs` ou `runs`,
  met à jour le statut et le `TableState` correspondant.
- **Vue filtrée des runs** (dérivée, pas stockée) :
  `visible_runs() -> Vec<&Run>` :
  - si `only_pr_runs` : ne garde que les runs dont `head_branch` ∈
    `{ pr.head_ref_name for pr in self.prs }` (un `HashSet<&str>` construit à la
    volée) ;
  - sinon : tous les `runs`.
  - Le rendu et la navigation de l'onglet Actions s'appuient sur cette vue.
- **`toggle_only_pr_runs()`** : inverse le booléen. Pas de re-fetch (filtrage
  purement local). Réajuste la sélection si elle sort de la vue.
- Dépendance assumée : le croisement utilise `self.prs`, rempli au démarrage
  (`app.refresh()` charge les PRs avant la TUI). Si `prs` est vide, la vue
  filtrée est vide → on affichera un indice (« aucune PR chargée »). Limitation
  connue : on filtre les 20 derniers runs/repo ; si aucun n'est sur une branche
  de PR, la liste peut être vide (acceptable en v1 ; on pourra augmenter la
  limite plus tard).

### 3.6 `ui.rs`

- **Barre d'onglets** : une ligne `[ PRs ] [ Actions ]`, l'actif surligné
  (reversed/cyan), intégrée à l'entête (passer le bloc entête de 4 à 5 lignes
  ou insérer une ligne dédiée).
- **`render_table`** : `match app.active_tab` → table PRs (existante) ou table
  runs.
- **`run_to_row(run: &Run) -> Row<'_>`**, colonnes :
  `repo · created(10) · workflow · branch · event · status · title(Fill)`.
  - `status` coloré via `run_look(status, conclusion)` :
    | état | libellé | couleur |
    |---|---|---|
    | completed + success | `✓ success` | vert |
    | completed + failure | `✗ failure` | rouge |
    | completed + cancelled/skipped | `cancelled`/`skipped` | gris foncé |
    | in_progress | `● running` | jaune |
    | queued | `queued` | gris |
- **Footer** : ajouter l'indicateur d'onglet + la touche du toggle (voir 3.7) ;
  l'aide (`?`) documente les nouveaux raccourcis.
- L'entête (résumé filtres) reste lié à l'onglet PRs ; sur l'onglet Actions,
  afficher l'état du toggle « mes PRs : [x] » à la place.

### 3.7 `main.rs` — clavier

- **Global** (mode normal, tous onglets) :
  - `Tab` → onglet suivant (cycle) ; `1` → PRs ; `2` → Actions.
  - `r` refresh, `a` auto, `q`, `?` : inchangés (agissent sur l'onglet actif).
  - `Enter` : ouvre l'URL de l'élément sélectionné **de l'onglet actif** (PR ou
    run).
  - `j`/`k`/`↑`/`↓` : navigation dans la table de l'onglet actif.
- **Onglet Actions uniquement** : `m` → `toggle_only_pr_runs()` (mnémo
  « mine »). Sur l'onglet PRs, `m` est ignoré.
- Le panneau de filtres (`f`) reste **spécifique aux PRs** en v1 (le filtre
  Actions se limite au toggle `m`). On ne rend pas le panneau tab-aware
  maintenant (YAGNI).

## 4. Gestion d'erreur

Identique aux PRs : un repo dont `gh run list` échoue incrémente `errors` ;
le statut affiche « … — N en erreur ». Un run sans `conclusion` (pas fini) est
normal, pas une erreur.

## 5. Tests (unitaires, `#[cfg(test)]`)

- `run_look` : chaque (status, conclusion) → bon libellé + bonne couleur.
- Filtre `visible_runs` : avec `only_pr_runs = true`, ne garde que les runs
  dont la branche est dans l'ensemble des branches de PRs ; `false` → tout.
- `Tab` : cycle `Tab` (Prs→Runs→Prs).
- Parsing serde d'un échantillon JSON `gh run list` → `Vec<Run>` (champs bien
  mappés, `conclusion` absente gérée par `#[serde(default)]`).

## 6. Hors périmètre (YAGNI, plus tard si besoin)

- Filtre statut (`failure`/`in_progress`) et filtre `since` sur les runs.
- Trait `Listable` / rendu générique (justifié seulement à partir d'un 3ᵉ
  onglet réellement dupliqué).
- Onglet Issues, Releases, Notifications.
- Panneau de filtres tab-aware / filtres Actions dans le panneau.
- Re-fetch server-side par branche (`gh run list --branch …`).

## 7. Impact sur l'existant

- `Sender<FetchResult>` → `Sender<Loaded>` : touche `app.rs` et `fetch.rs`.
- `Pr` gagne un champ + `JSON_FIELDS` gagne `headRefName` : rétro-compatible.
- Le reste (filtres PRs, persistance, panneau, aide) est inchangé.
