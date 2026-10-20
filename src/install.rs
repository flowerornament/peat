//! `peat hook install` and `peat init`: wiring the harness configs and
//! opting a repo in.
//!
//! The hook command is the constant `peat hook`, and it is a silent no-op
//! wherever there is no ledger, so it belongs in the user-level configs —
//! written once per machine, correct in every repo. A repo opts in by
//! having a ledger (`peat init`). A repo's own hook files (what releases up
//! to 0.5.0 wrote) now fire a second time beside the user-level hook; the
//! duplicate guard keeps that harmless, and install removes peat from the
//! untracked ones (a tracked file is shared work, so it is only reported).
//! `--project` keeps the per-repo form for repos shared with people who
//! lack the user-level hook.
//!
//! Every edit is a merge: other keys and other tools' hooks survive in
//! place, peat ends with exactly one `peat hook` on each of [`EVENTS`] and
//! none on any other event, and a copied peat snippet is replaced where it
//! stands, so no foreign group changes index (Codex keys hook trust to
//! `path:event:group:hook`). A config of a shape peat does not write is
//! refused, never repaired. JSON configs round-trip through `serde_json`
//! (a rewrite normalizes key order); Codex's `config.toml` is edited in
//! place with `toml_edit`, so its comments, ordering and trust records
//! survive. Writes are atomic (temp file + rename, mode kept, symlinks
//! followed to their target).

use std::path::{Path, PathBuf};

use crate::db;
use crate::hook::EVENTS;

/// The hook command, in every config and every format.
const COMMAND: &str = "peat hook";

/// The comment that heads the blocks peat adds to a TOML config.
const TOML_BANNER: &str =
    "# peat: the one command every harness hook runs; behaviour ships with the binary\n";

/// Which harness a per-repo (`--project`) install writes for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fabric {
    Claude,
    /// Claude Code's per-user, git-ignored settings layer.
    ClaudeLocal,
    Codex,
}

impl Fabric {
    /// The project-relative config file each harness reads hooks from.
    pub fn config_path(self) -> &'static str {
        match self {
            Fabric::Claude => ".claude/settings.json",
            Fabric::ClaudeLocal => ".claude/settings.local.json",
            Fabric::Codex => ".codex/hooks.json",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Fabric::Claude | Fabric::ClaudeLocal => "claude",
            Fabric::Codex => "codex",
        }
    }
    fn install_flag(self) -> &'static str {
        match self {
            Fabric::Claude => " --project",
            Fabric::ClaudeLocal => " --project --local",
            Fabric::Codex => " --project --codex",
        }
    }
    fn target(self, root: &Path) -> Target {
        Target {
            path: root.join(self.config_path()),
            format: Format::Json,
            harness: self.name(),
            fix: format!("peat hook install{}", self.install_flag()),
        }
    }
}

/// How a config file spells its hooks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// `{"hooks": {"<Event>": [{"hooks": [{"type": …, "command": …}]}]}}` —
    /// Claude Code settings and Codex `hooks.json`.
    Json,
    /// `[[hooks.<Event>]]` blocks holding `hooks = [{ type, command }]` —
    /// Codex `config.toml`.
    Toml,
}

/// One config file install maintains, and the command that repairs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    pub format: Format,
    pub harness: &'static str,
    fix: String,
}

/// Where the user-level configs live. Read from the environment once, so
/// tests hand in temp dirs and never see the real home.
#[derive(Clone, Debug)]
pub struct Homes {
    pub home: PathBuf,
    pub claude_config_dir: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
}

impl Homes {
    pub fn from_env() -> Option<Homes> {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Some(Homes {
            home: var("HOME")?,
            claude_config_dir: var("CLAUDE_CONFIG_DIR"),
            codex_home: var("CODEX_HOME"),
        })
    }
}

/// Every user-level config peat wires: `settings.json` in each Claude
/// config dir (`~/.claude`, `$CLAUDE_CONFIG_DIR`, and any live `~/.claude-*`
/// sibling — one holding both a `settings.json` and the `projects/` Claude
/// Code keeps its sessions in, e.g. `~/.claude-work`, never a backup copy),
/// and Codex's `config.toml` (`$CODEX_HOME`, else `~/.codex`) when Codex
/// is set up on this machine.
pub fn user_targets(h: &Homes) -> Vec<Target> {
    let user_fix = || "peat hook install".to_string();
    let mut out: Vec<Target> = Vec::new();
    for d in claude_dirs(h) {
        let path = d.join("settings.json");
        if !out.iter().any(|t| same_file(&t.path, &path)) {
            out.push(Target {
                path,
                format: Format::Json,
                harness: "claude",
                fix: user_fix(),
            });
        }
    }
    if let Some(codex) = codex_home(h) {
        out.push(Target {
            path: codex.join("config.toml"),
            format: Format::Toml,
            harness: "codex",
            fix: user_fix(),
        });
    }
    out
}

/// Codex's home (`$CODEX_HOME`, else `~/.codex`), when Codex is set up on
/// this machine.
fn codex_home(h: &Homes) -> Option<PathBuf> {
    let codex = h
        .codex_home
        .clone()
        .unwrap_or_else(|| h.home.join(".codex"));
    (h.codex_home.is_some() || codex.is_dir()).then_some(codex)
}

/// Every Claude Code config dir, in order: `~/.claude`,
/// `$CLAUDE_CONFIG_DIR`, then each live `~/.claude-*` sibling. Two
/// spellings of one dir may both appear; callers dedupe by file.
fn claude_dirs(h: &Homes) -> Vec<PathBuf> {
    let mut dirs = vec![h.home.join(".claude")];
    dirs.extend(h.claude_config_dir.clone());
    let mut siblings: Vec<PathBuf> = std::fs::read_dir(&h.home)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".claude-"))
                && p.join("settings.json").is_file()
                && p.join("projects").is_dir()
        })
        .collect();
    siblings.sort();
    dirs.extend(siblings);
    dirs
}

/// The per-repo files a harness reads hooks from — what releases up to
/// 0.5.0 wrote, and what now double-fires beside the user-level hook.
pub fn project_targets(root: &Path) -> Vec<Target> {
    let mut out: Vec<Target> = [Fabric::Claude, Fabric::ClaudeLocal, Fabric::Codex]
        .into_iter()
        .map(|f| f.target(root))
        .collect();
    out.push(Target {
        path: root.join(".codex/config.toml"),
        format: Format::Toml,
        harness: "codex",
        fix: String::new(),
    });
    out
}

/// Two spellings of one path (a `$CLAUDE_CONFIG_DIR` naming `~/.claude`).
fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| {
        p.parent()
            .and_then(|d| d.canonicalize().ok())
            .map(|d| d.join(p.file_name().unwrap_or_default()))
            .unwrap_or_else(|| p.to_path_buf())
    };
    a == b || canon(a) == canon(b)
}

/// Is this repo file committed work? Asked read-only of jj
/// (`--ignore-working-copy`, so nothing is snapshotted) where `.jj` is the
/// nearest root, else of git, fenced to this root. A tool that cannot
/// answer counts as tracked: peat leaves alone what it cannot vouch for.
///
/// Without a snapshot jj answers from `@` as last recorded, so a file
/// created since then counts as untracked — accepted: such a file is
/// this machine's until a jj command records it.
fn tracked(root: &Path, rel: &str) -> bool {
    let out = if root.join(".jj").exists() {
        std::process::Command::new("jj")
            .args(["file", "list", "--ignore-working-copy", "-r", "@"])
            .arg(format!("root-file:{rel:?}"))
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .output()
    } else {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["ls-files", "--", rel])
            .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap_or(root))
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(std::process::Stdio::null())
            .output()
    };
    out.map_or(true, |o| !o.status.success() || !o.stdout.is_empty())
}

// ---- edits: one algorithm, two formats ---------------------------------

/// What an install does to one file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    /// Exactly one `peat hook` on each of [`EVENTS`], none elsewhere.
    Install,
    /// No peat entry anywhere (a project file the user-level hook covers).
    Strip,
}

/// The result of editing a config's text.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Unchanged,
    Write(String),
    /// The edit left nothing but an empty document: the file held only peat.
    Remove,
}

/// Is this program word peat itself (`peat`, `/nix/…/bin/peat`)?
fn is_peat_word(word: &str) -> bool {
    word == "peat" || word.ends_with("/peat")
}

/// Every verb a released peat hook snippet ran (census of the snippets in
/// this repo's history: `capture`, `obs`, `brief`, `hook`; plus the 0.4
/// moments `push`, `distill`, `recall`). A peat word followed by anything
/// else is not a peat hook.
const VERBS: [&str; 7] = [
    "hook", "capture", "brief", "obs", "push", "distill", "recall",
];

/// A shell line as simple commands, each a list of words with quoting
/// removed. Separators (`;` `&` `|` `(` `)` `` ` `` `{` `}` newline) split
/// commands only outside quotes: quoted text is one word, never a command
/// — `say "peat finished"` runs `say`.
fn simple_commands(line: &str) -> Vec<Vec<String>> {
    let (mut cmds, mut words, mut word) = (Vec::new(), Vec::new(), None::<String>);
    let mut chars = line.chars();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    word.get_or_insert_default().push(n);
                }
            }
            (Some(_), c) => word.get_or_insert_default().push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert_default();
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    word.get_or_insert_default().push(n);
                }
            }
            (None, c) if ";&|()`{}\n".contains(c) => {
                words.extend(word.take());
                cmds.push(std::mem::take(&mut words));
            }
            (None, c) if c.is_whitespace() => words.extend(word.take()),
            (None, c) => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word);
    cmds.push(words);
    cmds
}

/// Does this shell line run peat with one of its [`VERBS`]? The program
/// of a simple command is its first word past `VAR=value` assignments and
/// prefix commands (`nohup`, `exec`, …); a quoted string is looked inside
/// only as the script of a shell's `-c` (`sh -c '…'`, `sh -mc "…"`).
fn runs_peat(line: &str, depth: u8) -> bool {
    const PREFIX: [&str; 7] = ["nohup", "exec", "command", "env", "time", "nice", "setsid"];
    const SHELLS: [&str; 5] = ["sh", "bash", "zsh", "dash", "ksh"];
    simple_commands(line).iter().any(|words| {
        let mut rest = words.iter().map(String::as_str).skip_while(|w| {
            (w.contains('=') && !w.starts_with('=') && !w.starts_with('-')) || PREFIX.contains(w)
        });
        let Some(program) = rest.next() else {
            return false;
        };
        let rest: Vec<&str> = rest.collect();
        if is_peat_word(program) {
            return rest.first().is_some_and(|v| VERBS.contains(v));
        }
        let shell = program.rsplit('/').next().unwrap_or(program);
        if depth == 0 || !SHELLS.contains(&shell) {
            return false;
        }
        // `-c`, or a flag cluster ending in it (`-mc`, `-lc`)
        let script = rest
            .iter()
            .position(|w| {
                w.len() >= 2 && w.starts_with('-') && !w.starts_with("--") && w.ends_with('c')
            })
            .and_then(|i| rest.get(i + 1));
        script.is_some_and(|s| runs_peat(s, depth - 1))
    })
}

/// The one-verb form, by any path to the binary.
fn is_current(cmd: &str) -> bool {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    matches!(words[..], [p, "hook"] if is_peat_word(p))
}

/// Every hook command peat has ever installed: the one-verb form, and the
/// copied bash snippets of releases before 0.4.0 (a line that runs peat).
fn is_peat(cmd: &str) -> bool {
    is_current(cmd) || runs_peat(cmd, 2)
}

/// A peat command that is not the one-verb form.
fn is_legacy(cmd: &str) -> bool {
    !is_current(cmd) && is_peat(cmd)
}

/// One event's hook groups, in whichever format holds them.
trait Groups {
    /// Per group, the command of each entry; `None` for a group whose hook
    /// list is not a shape peat knows (left alone, never removed).
    fn commands(&self) -> Vec<Option<Vec<Option<String>>>>;
    /// Make entry `e` of group `g` the one-verb hook, in place.
    fn replace(&mut self, g: usize, e: usize);
    fn remove(&mut self, g: usize, e: usize);
    fn remove_group(&mut self, g: usize);
    /// Append a group holding the one-verb hook; `prefix` heads it (TOML).
    fn push(&mut self, prefix: &str);
}

/// What one event needs.
#[derive(Clone, Copy)]
enum Want {
    /// Exactly one `peat hook`: the first copied snippet becomes it where
    /// it stands; any further snippet goes.
    One,
    /// No peat at all.
    None,
}

/// Bring one event to `want`. Returns (changed, the event is now empty).
/// Only entries peat removes can empty a group, and only groups peat
/// empties are removed, so no foreign group or entry moves except past a
/// removed peat entry; each foreign command that does move is pushed onto
/// `moved` (Codex's trust for it, keyed by index, is lost).
fn sweep(
    groups: &mut dyn Groups,
    want: Want,
    prefix: &str,
    moved: &mut Vec<String>,
) -> (bool, bool) {
    let cmds = groups.commands();
    let all = || cmds.iter().flatten().flatten();
    let mut have = matches!(want, Want::One) && all().flatten().any(|c| is_current(c));
    let drop: fn(&str) -> bool = match want {
        Want::One => is_legacy,
        Want::None => is_peat,
    };
    let mut changed = false;
    let mut removes: Vec<(usize, usize)> = Vec::new();
    for (g, list) in cmds.iter().enumerate() {
        for (e, c) in list.iter().flatten().enumerate() {
            if c.as_deref().is_some_and(drop) {
                if matches!(want, Want::One) && !have {
                    groups.replace(g, e);
                    have = true;
                } else {
                    removes.push((g, e));
                }
                changed = true;
            }
        }
    }
    for &(g, e) in removes.iter().rev() {
        groups.remove(g, e);
    }
    let emptied: Vec<usize> = cmds
        .iter()
        .enumerate()
        .filter(|(g, list)| {
            list.as_ref().is_some_and(|l| {
                !l.is_empty() && removes.iter().filter(|(rg, _)| rg == g).count() == l.len()
            })
        })
        .map(|(g, _)| g)
        .collect();
    for &g in emptied.iter().rev() {
        groups.remove_group(g);
    }
    for (g, list) in cmds.iter().enumerate() {
        for (e, c) in list.iter().flatten().enumerate() {
            let shifts = removes.iter().any(|&(rg, re)| rg == g && re < e)
                || emptied.iter().any(|&eg| eg < g);
            if shifts && !removes.contains(&(g, e)) && !c.as_deref().is_some_and(is_peat) {
                moved.push(
                    c.clone()
                        .unwrap_or_else(|| "(entry without a command)".into()),
                );
            }
        }
    }
    if matches!(want, Want::One) && !have {
        groups.push(prefix);
        changed = true;
    }
    let empty = !cmds.is_empty() && emptied.len() == cmds.len();
    (changed, empty)
}

/// Apply `op` to a config's text (`None`: the file does not exist). A
/// config of a shape peat does not write is refused, naming the key.
#[cfg(test)]
fn edit(format: Format, text: Option<&str>, op: Op) -> Result<Outcome, String> {
    edit_noting(format, text, op, &mut vec![])
}

/// [`edit`], also collecting `Event: command` for every foreign hook the
/// edit moves to another index.
fn edit_noting(
    format: Format,
    text: Option<&str>,
    op: Op,
    moved: &mut Vec<String>,
) -> Result<Outcome, String> {
    let blank = text.is_none_or(|t| t.trim().is_empty());
    if blank && op == Op::Strip {
        return Ok(Outcome::Unchanged);
    }
    let (changed, empty, out) = match format {
        Format::Json => {
            let mut v: serde_json::Value = if blank {
                serde_json::json!({})
            } else {
                serde_json::from_str(text.unwrap_or_default())
                    .map_err(|e| format!("not valid JSON ({e})"))?
            };
            let changed = json::edit(&mut v, op, moved)?;
            let empty = v.as_object().is_some_and(|o| o.is_empty());
            let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n";
            (changed, empty, out)
        }
        Format::Toml => {
            let mut doc: toml_edit::DocumentMut = text
                .unwrap_or_default()
                .parse()
                .map_err(|e| format!("not valid TOML ({e})"))?;
            let changed = toml::edit(&mut doc, op, moved)?;
            (changed, doc.as_table().is_empty(), doc.to_string())
        }
    };
    Ok(if !changed || text == Some(out.as_str()) {
        Outcome::Unchanged
    } else if op == Op::Strip && empty {
        Outcome::Remove
    } else {
        Outcome::Write(out)
    })
}

/// Which of [`EVENTS`] carry `peat hook` in this config's text.
pub fn installed_events(format: Format, text: &str) -> Vec<&'static str> {
    let has = |cmds: Vec<Option<Vec<Option<String>>>>| {
        cmds.into_iter()
            .flatten()
            .flatten()
            .flatten()
            .any(|c| is_current(&c))
    };
    match format {
        Format::Json => {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
                return vec![];
            };
            EVENTS
                .into_iter()
                .filter(|ev| {
                    v.get("hooks")
                        .and_then(|h| h.get(ev))
                        .and_then(|g| g.as_array())
                        .is_some_and(|g| has(json::commands(g)))
                })
                .collect()
        }
        Format::Toml => {
            let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
                return vec![];
            };
            let Some(hooks) = doc.get("hooks").and_then(toml_edit::Item::as_table) else {
                return vec![];
            };
            EVENTS
                .into_iter()
                .filter(|ev| hooks.get(ev).is_some_and(|i| has(toml::commands(i))))
                .collect()
        }
    }
}

/// The full JSON snippet — what `--print` shows for a JSON config.
pub fn snippet() -> serde_json::Value {
    let mut cfg = serde_json::json!({});
    let _ = json::edit(&mut cfg, Op::Install, &mut vec![]);
    cfg
}

/// The hook entry every event gets.
fn entry() -> serde_json::Value {
    serde_json::json!({ "type": "command", "command": COMMAND })
}

mod json {
    use serde_json::{Map, Value, json};

    use super::{EVENTS, Groups, Op, Want, entry, sweep};

    fn list(group: &Value) -> Option<&Vec<Value>> {
        group.get("hooks").and_then(Value::as_array)
    }

    pub fn commands(groups: &[Value]) -> Vec<Option<Vec<Option<String>>>> {
        groups
            .iter()
            .map(|g| {
                list(g).map(|l| {
                    l.iter()
                        .map(|h| h.get("command").and_then(Value::as_str).map(str::to_string))
                        .collect()
                })
            })
            .collect()
    }

    impl Groups for Vec<Value> {
        fn commands(&self) -> Vec<Option<Vec<Option<String>>>> {
            commands(self)
        }
        fn replace(&mut self, g: usize, e: usize) {
            self[g]["hooks"][e] = entry();
        }
        fn remove(&mut self, g: usize, e: usize) {
            if let Some(l) = self[g].get_mut("hooks").and_then(Value::as_array_mut) {
                l.remove(e);
            }
        }
        fn remove_group(&mut self, g: usize) {
            self.remove(g);
        }
        fn push(&mut self, _prefix: &str) {
            self.push(json!({ "hooks": [entry()] }));
        }
    }

    /// The `hooks` object, created when absent and `create`; refused when
    /// the config or any of [`EVENTS`] under it has another shape.
    fn hooks(config: &mut Value, create: bool) -> Result<Option<&mut Map<String, Value>>, String> {
        let Some(root) = config.as_object_mut() else {
            return Err("the top level is not an object — not ours to repair".into());
        };
        if create && !root.contains_key("hooks") {
            root.insert("hooks".into(), json!({}));
        }
        let Some(hooks) = root.get_mut("hooks") else {
            return Ok(None);
        };
        let Some(hooks) = hooks.as_object_mut() else {
            return Err("`hooks` is not an object — not ours to repair".into());
        };
        for ev in EVENTS {
            if hooks.get(ev).is_some_and(|g| !g.is_array()) {
                return Err(format!("`hooks.{ev}` is not an array — not ours to repair"));
            }
        }
        Ok(Some(hooks))
    }

    pub fn edit(config: &mut Value, op: Op, moved: &mut Vec<String>) -> Result<bool, String> {
        let Some(hooks) = hooks(config, op == Op::Install)? else {
            return Ok(false);
        };
        let mut changed = false;
        let keys: Vec<String> = hooks.keys().cloned().collect();
        for ev in keys {
            let want = match op {
                Op::Install if EVENTS.contains(&ev.as_str()) => Want::One,
                _ => Want::None,
            };
            // a non-array under another event name is not an event peat wrote
            let Some(groups) = hooks.get_mut(&ev).and_then(Value::as_array_mut) else {
                continue;
            };
            let mut shifted = vec![];
            let (c, empty) = sweep(groups, want, "", &mut shifted);
            moved.extend(shifted.into_iter().map(|c| format!("{ev}: {c}")));
            changed |= c;
            if c && empty {
                hooks.remove(&ev);
            }
        }
        if op == Op::Install {
            for ev in EVENTS {
                if !hooks.contains_key(ev) {
                    let mut groups: Vec<Value> = vec![];
                    sweep(&mut groups, Want::One, "", &mut vec![]);
                    hooks.insert(ev.into(), Value::Array(groups));
                    changed = true;
                }
            }
        } else if changed && hooks.is_empty() {
            config.as_object_mut().unwrap().remove("hooks");
        }
        Ok(changed)
    }
}

/// The same algorithm over Codex's `config.toml`, in place. An event is
/// an array of tables (`[[hooks.Stop]]`) or an inline array of tables;
/// each group's `hooks` is an inline array (`hooks = [{ … }]`) or a nested
/// array of tables (`[[hooks.Stop.hooks]]`). Non-array keys under `hooks`
/// other than the four events (`[hooks.state]`, Codex's trust records) are
/// not events and are never touched.
mod toml {
    use toml_edit::{
        Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value,
    };

    use super::{COMMAND, EVENTS, Groups, Op, TOML_BANNER, Want, sweep};

    fn command(t: &dyn TableLike) -> Option<String> {
        t.get("command")
            .and_then(|c| c.as_str())
            .map(str::to_string)
    }

    fn entries(group: &dyn TableLike) -> Option<Vec<Option<String>>> {
        match group.get("hooks")? {
            Item::Value(Value::Array(a)) => Some(
                a.iter()
                    .map(|v| v.as_inline_table().and_then(|t| command(t)))
                    .collect(),
            ),
            Item::ArrayOfTables(a) => Some(a.iter().map(|t| command(t)).collect()),
            _ => None,
        }
    }

    pub fn commands(item: &Item) -> Vec<Option<Vec<Option<String>>>> {
        match item {
            Item::ArrayOfTables(a) => a.iter().map(|g| entries(g)).collect(),
            Item::Value(Value::Array(a)) => a
                .iter()
                .map(|g| g.as_inline_table().and_then(|t| entries(t)))
                .collect(),
            _ => vec![],
        }
    }

    fn the_entry() -> InlineTable {
        let mut e = InlineTable::new();
        e.insert("type", "command".into());
        e.insert("command", COMMAND.into());
        e
    }

    fn group() -> Table {
        let mut list = Array::new();
        list.push(the_entry());
        let mut g = Table::new();
        g.insert("hooks", toml_edit::value(list));
        g
    }

    fn group_mut(item: &mut Item, g: usize) -> Option<&mut dyn TableLike> {
        match item {
            Item::ArrayOfTables(a) => a.get_mut(g).map(|t| t as &mut dyn TableLike),
            Item::Value(Value::Array(a)) => a
                .get_mut(g)
                .and_then(Value::as_inline_table_mut)
                .map(|t| t as &mut dyn TableLike),
            _ => None,
        }
    }

    impl Groups for Item {
        fn commands(&self) -> Vec<Option<Vec<Option<String>>>> {
            commands(self)
        }
        fn replace(&mut self, g: usize, e: usize) {
            let Some(list) = group_mut(self, g).and_then(|g| g.get_mut("hooks")) else {
                return;
            };
            match list {
                // replace keeps the slot's surrounding whitespace
                Item::Value(Value::Array(a)) => {
                    a.replace(e, the_entry());
                }
                Item::ArrayOfTables(a) => {
                    if let Some(t) = a.get_mut(e) {
                        t.clear();
                        t.insert("type", toml_edit::value("command"));
                        t.insert("command", toml_edit::value(COMMAND));
                    }
                }
                _ => {}
            }
        }
        fn remove(&mut self, g: usize, e: usize) {
            let Some(list) = group_mut(self, g).and_then(|g| g.get_mut("hooks")) else {
                return;
            };
            match list {
                Item::Value(Value::Array(a)) => {
                    a.remove(e);
                    // a new first entry keeps the space that followed a comma
                    if e == 0
                        && let Some(first) = a.get_mut(0)
                    {
                        first.decor_mut().set_prefix("");
                    }
                }
                Item::ArrayOfTables(a) => {
                    a.remove(e);
                }
                _ => {}
            }
        }
        fn remove_group(&mut self, g: usize) {
            match self {
                Item::ArrayOfTables(a) => {
                    a.remove(g);
                }
                Item::Value(Value::Array(a)) => {
                    a.remove(g);
                }
                _ => {}
            }
        }
        fn push(&mut self, prefix: &str) {
            let mut g = group();
            if !prefix.is_empty() {
                g.decor_mut().set_prefix(prefix);
            }
            match self {
                Item::ArrayOfTables(a) => a.push(g),
                Item::Value(Value::Array(a)) => a.push(group().into_inline_table()),
                _ => {}
            }
        }
    }

    pub fn edit(doc: &mut DocumentMut, op: Op, moved: &mut Vec<String>) -> Result<bool, String> {
        // a blank line separates peat's blocks from what precedes them
        let lead = if doc.as_table().is_empty() { "" } else { "\n" };
        if op == Op::Install && !doc.as_table().contains_key("hooks") {
            let mut t = Table::new();
            t.set_implicit(true);
            doc.insert("hooks", Item::Table(t));
        }
        let Some(item) = doc.get_mut("hooks") else {
            return Ok(false);
        };
        let Some(hooks) = item.as_table_mut() else {
            return Err("`hooks` is not a [hooks] table — not ours to repair".into());
        };
        for ev in EVENTS {
            match hooks.get(ev) {
                None | Some(Item::ArrayOfTables(_)) | Some(Item::Value(Value::Array(_))) => {}
                Some(_) => {
                    return Err(format!(
                        "`hooks.{ev}` is not an array of tables ([[hooks.{ev}]]) — not ours to repair"
                    ));
                }
            }
        }
        let mut changed = false;
        let mut banner = true;
        let keys: Vec<String> = hooks.iter().map(|(k, _)| k.to_string()).collect();
        for ev in keys {
            let item = hooks.get_mut(&ev).unwrap();
            if !matches!(item, Item::ArrayOfTables(_) | Item::Value(Value::Array(_))) {
                continue;
            }
            let want = match op {
                Op::Install if EVENTS.contains(&ev.as_str()) => Want::One,
                _ => Want::None,
            };
            let prefix = if banner {
                format!("{lead}{TOML_BANNER}")
            } else {
                String::new()
            };
            let before = commands(item).len();
            let mut shifted = vec![];
            let (c, empty) = sweep(item, want, &prefix, &mut shifted);
            moved.extend(shifted.into_iter().map(|c| format!("{ev}: {c}")));
            banner &= commands(item).len() <= before;
            changed |= c;
            if c && empty {
                hooks.remove(&ev);
            }
        }
        if op == Op::Install {
            for ev in EVENTS {
                if !hooks.contains_key(ev) {
                    let mut item = Item::ArrayOfTables(ArrayOfTables::new());
                    let prefix = if banner {
                        banner = false;
                        format!("{lead}{TOML_BANNER}")
                    } else {
                        String::new()
                    };
                    sweep(&mut item, Want::One, &prefix, &mut vec![]);
                    hooks.insert(ev, item);
                    changed = true;
                }
            }
        } else if changed && hooks.is_empty() {
            doc.remove("hooks");
        }
        Ok(changed)
    }
}

// ---- applying an edit to a file --------------------------------------

/// Read a config: `Ok(None)` only when it does not exist. Any other
/// failure (permissions, not UTF-8) refuses, so nothing peat cannot read
/// is ever overwritten.
fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "{}: cannot read ({e}); fix or move it first",
            path.display()
        )),
    }
}

/// Write `text` to `path` atomically: a temp file beside the real target
/// (a symlink is followed, so a linked config stays linked), created with
/// the target's mode (0600 for a new file) so it is never wider than the
/// file it replaces, then a rename. A read-only target is refused. A
/// hardlinked config ends up a separate file (the rename replaces the
/// link, not the shared inode).
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    write_atomic_mode(path, text, 0o600)
}

/// [`write_atomic`] with the mode a new file gets: 0600 suits a config
/// (it may hold secrets), 0644 a skill (it is documentation).
fn write_atomic_mode(path: &Path, text: &str, new_mode: u32) -> Result<(), String> {
    let shown = path.display();
    let real = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => path
            .canonicalize()
            .map_err(|e| format!("{shown}: dangling symlink ({e})"))?,
        _ => path.to_path_buf(),
    };
    let meta = std::fs::metadata(&real).ok();
    if meta.as_ref().is_some_and(|m| m.permissions().readonly()) {
        return Err(format!("{shown}: read-only; not rewriting it"));
    }
    let dir = real.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{shown}: {e}"))?;
    let tmp = dir.join(format!(
        ".{}.peat-{}",
        real.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        meta.as_ref()
            .map_or(new_mode, |m| m.permissions().mode() & 0o7777)
    };
    // a temp file a crashed run left under this pid is ours to replace
    let _ = std::fs::remove_file(&tmp);
    let done = (|| {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        // the umask may have narrowed the mode at creation
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
        std::fs::rename(&tmp, &real)
    })();
    if let Err(e) = done {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{shown}: {e}"));
    }
    Ok(())
}

/// Edit (or, with `check`, inspect) one config file. `Ok(None)`: nothing
/// worth a line (a project file with no peat in it). `Err` carries a
/// check's drift or a file peat will not touch.
pub fn apply(t: &Target, op: Op, check: bool) -> Result<Option<String>, String> {
    let path = t.path.display();
    let text = read(&t.path)?;
    let mut moved = vec![];
    let outcome = edit_noting(t.format, text.as_deref(), op, &mut moved)
        .map_err(|e| format!("{path}: {e}; fix or move it first"))?;
    // Codex keys trust to a hook's index; say which foreign ones moved
    let retrust = if t.harness == "codex" && !moved.is_empty() {
        format!(
            "\n  Codex trust is per index: re-trust in /hooks: {}",
            moved.join("; ")
        )
    } else {
        String::new()
    };
    let all = || format!("peat hook on all {} events", EVENTS.len());
    match (op, outcome) {
        (Op::Install, Outcome::Unchanged) if check => Ok(Some(format!("✓ {path}: {}", all()))),
        (Op::Install, Outcome::Unchanged) => Ok(Some(format!("= {path}: already current"))),
        (Op::Strip, Outcome::Unchanged) => Ok(None),
        (Op::Install, _) if check => {
            let have = installed_events(t.format, text.as_deref().unwrap_or_default());
            let missing: Vec<&str> = EVENTS.into_iter().filter(|e| !have.contains(e)).collect();
            let what = if missing.is_empty() {
                "stale peat entries (retired events or copied snippets)".to_string()
            } else {
                format!("peat hook missing on {}", missing.join(", "))
            };
            Err(format!("{path}: {what} — run: {}", t.fix))
        }
        (Op::Strip, _) if check => Ok(Some(format!("- {path}: {DOUBLE}"))),
        (_, Outcome::Write(s)) => {
            write_atomic(&t.path, &s)?;
            Ok(Some(match op {
                Op::Install => format!("✓ {path}: {}{retrust}", all()),
                Op::Strip => format!(
                    "✓ {path}: peat removed (the user-level hook covers this repo){retrust}"
                ),
            }))
        }
        (_, Outcome::Remove) => {
            std::fs::remove_file(&t.path).map_err(|e| format!("{path}: {e}"))?;
            Ok(Some(format!(
                "✓ {path}: removed (it held only peat's hooks)"
            )))
        }
    }
}

/// What a repo's own peat hook means beside the user-level one.
const DOUBLE: &str = "peat hook is also wired here, so it fires twice beside the user-level hook \
(harmless: the brief is guarded and captures are idempotent)";

/// The repo root holding `.peat`, when cwd is inside a repo — and never
/// the home directory, which may be a repo (`~/.git`) but whose
/// `.claude/` and `.codex/` are the user-level configs themselves.
fn repo_root(home: Option<&Path>) -> Option<PathBuf> {
    let root = db::peat_dir().parent()?.to_path_buf();
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let is_home = home.is_some_and(|h| canon(h) == canon(&root));
    (!is_home && (root.join(".git").exists() || root.join(".jj").exists())).then_some(root)
}

/// The current repo's own copies: untracked ones are stripped (they are
/// this machine's, like `.claude/settings.local.json`), tracked ones only
/// reported unless `strip_tracked` — they are shared work, and peat does
/// not edit what a commit carries unasked.
fn project_copies(
    root: &Path,
    check: bool,
    strip_tracked: bool,
    tracked: impl Fn(&Path, &str) -> bool,
) -> Vec<Result<Option<String>, String>> {
    project_targets(root)
        .iter()
        .map(|p| {
            let rel = p.path.strip_prefix(root).unwrap_or(&p.path);
            let rel = rel.to_string_lossy();
            if check || strip_tracked {
                return apply(p, Op::Strip, check);
            }
            // look before asking the VCS: most repos carry no copy at all
            match apply(p, Op::Strip, true)? {
                None => Ok(None),
                Some(_) if tracked(root, &rel) => Ok(Some(format!(
                    "- {}: {DOUBLE}; tracked, so left alone — run: peat hook install --strip-project",
                    p.path.display()
                ))),
                Some(_) => apply(p, Op::Strip, false),
            }
        })
        .collect()
}

/// The text `--print` shows for one target's format.
fn print_snippet(format: Format) -> String {
    match format {
        Format::Json => serde_json::to_string_pretty(&snippet()).unwrap() + "\n",
        Format::Toml => {
            let mut doc = toml_edit::DocumentMut::new();
            let _ = toml::edit(&mut doc, Op::Install, &mut vec![]);
            doc.to_string()
        }
    }
}

// ---- the verbs -------------------------------------------------------

/// `peat hook install [--project [--local|--codex]] [--strip-project]
/// [--check|--print]`.
pub fn install_cli(project: Option<Fabric>, strip_tracked: bool, check: bool, print: bool) {
    let ok = match project {
        Some(fabric) => install_project(fabric, check, print),
        None => install_user(strip_tracked, check, print),
    };
    if !ok {
        std::process::exit(1);
    }
}

/// Report each line; returns whether every one was `Ok`.
fn report(results: Vec<Result<Option<String>, String>>) -> bool {
    let mut ok = true;
    for r in results {
        match r {
            Ok(Some(line)) => println!("{line}"),
            Ok(None) => {}
            Err(e) => {
                crate::ui::error(&e);
                ok = false;
            }
        }
    }
    ok
}

fn install_user(strip_tracked: bool, check: bool, print: bool) -> bool {
    let Some(homes) = Homes::from_env() else {
        crate::ui::error("HOME is not set; cannot find the user-level configs");
        return false;
    };
    let targets = user_targets(&homes);
    if print {
        for t in &targets {
            println!("# {}", t.path.display());
            print!("{}", print_snippet(t.format));
        }
        return true;
    }
    let mut results: Vec<_> = targets
        .iter()
        .map(|t| apply(t, Op::Install, check))
        .collect();
    let root = repo_root(Some(&homes.home));
    if let Some(root) = &root {
        results.extend(project_copies(root, check, strip_tracked, tracked));
    }
    results.extend(skill_status(&homes));
    let ok = report(results);
    if !check && ok {
        let mut harnesses: Vec<&str> = targets.iter().map(|t| t.harness).collect();
        harnesses.dedup();
        println!("  {} hook on: {}", harnesses.join(" + "), EVENTS.join(", "));
        if harnesses.contains(&"codex") {
            println!(
                "  Codex runs no hook until it is trusted: open /hooks in the Codex CLI and trust it"
            );
        }
        if root.is_some() && !crate::hook::enabled() {
            println!(
                "  this repo has no ledger, so peat records nothing here — run `peat init` to opt it in"
            );
        }
    }
    ok
}

/// The per-repo form releases up to 0.5.0 wrote: hooks and the skill at
/// the desk root, plus the ledger.
fn install_project(fabric: Fabric, check: bool, print: bool) -> bool {
    if print {
        print!("{}", print_snippet(Format::Json));
        return true;
    }
    let root = db::peat_dir()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    if !check {
        // `peat hook` is a no-op without a ledger
        init(&db::init_path());
    }
    let ok = report(vec![
        apply(&fabric.target(&root), Op::Install, check),
        install_skill(&root, fabric, check),
    ]);
    if !check && ok {
        println!("  {} hook on: {}", fabric.name(), EVENTS.join(", "));
        if fabric == Fabric::Codex {
            println!(
                "  Codex runs no hook until it is trusted: open /hooks in the Codex CLI and trust it"
            );
        }
    }
    ok
}

/// The peat skill: the judgment half (when to read, how to deposit, how
/// far to trust a line), shipped inside the binary so a project install
/// can lay it beside the hooks, `peat skill --user` can write it for the
/// user and `peat skill` can print it. The nix package installs the same
/// file at `share/peat/skills/peat/SKILL.md` for the home-manager module.
pub const SKILL: &str = include_str!("../skills/peat/SKILL.md");

/// Where each harness discovers project skills.
pub fn skill_path(fabric: Fabric) -> &'static str {
    match fabric {
        Fabric::Claude | Fabric::ClaudeLocal => ".claude/skills/peat/SKILL.md",
        Fabric::Codex => ".agents/skills/peat/SKILL.md",
    }
}

/// Write the skill file. `check` only reports, and a desk without a copy
/// passes: the skill may be installed user-level instead, and only a
/// stale copy in the desk is wrong.
pub fn install_skill(root: &Path, fabric: Fabric, check: bool) -> Result<Option<String>, String> {
    let path = root.join(skill_path(fabric));
    let current = read(&path)?;
    if check {
        return match current.as_deref() {
            Some(SKILL) => Ok(Some(format!("✓ {}: current", path.display()))),
            None => Ok(Some(format!(
                "- {}: not in this desk (the user-level skill serves it; `peat hook install --check` reports it)",
                path.display()
            ))),
            Some(_) => Err(format!(
                "{}: stale — run: peat hook install{}",
                path.display(),
                fabric.install_flag()
            )),
        };
    }
    if current.as_deref() == Some(SKILL) {
        return Ok(Some(format!("= {}: already current", path.display())));
    }
    write_atomic_mode(&path, SKILL, 0o644)?;
    Ok(Some(format!("✓ {}: written", path.display())))
}

/// One harness's user-level skill: every file it reads the peat skill
/// from, the one `peat skill --user` writes first.
pub struct UserSkill {
    pub harness: &'static str,
    pub paths: Vec<SkillPath>,
}

/// A skill file under the dir peat treats as given (a Claude config dir,
/// `~`, `$CODEX_HOME`); only the components below `root` are peat's to
/// judge, so a symlinked home or `/var` → `/private/var` never counts.
pub struct SkillPath {
    pub root: PathBuf,
    pub file: PathBuf,
}

impl SkillPath {
    fn new(root: &Path, rel: &str) -> SkillPath {
        SkillPath {
            root: root.to_path_buf(),
            file: root.join(rel),
        }
    }

    /// Whether the file, or any dir between it and `root`, is a symlink:
    /// managed elsewhere (home-manager's store links, or a config dir's
    /// `skills` linked into a shared hub), never written through.
    fn linked(&self) -> bool {
        self.file
            .ancestors()
            .take_while(|a| *a != self.root)
            .any(|a| std::fs::symlink_metadata(a).is_ok_and(|m| m.file_type().is_symlink()))
    }
}

/// Where each harness on this machine reads a user-level skill: `skills/`
/// in every Claude Code config dir that exists, and for Codex (when set
/// up) the shared `~/.agents/skills`, then `$CODEX_HOME/skills`.
pub fn user_skills(h: &Homes) -> Vec<UserSkill> {
    let mut out: Vec<UserSkill> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for d in claude_dirs(h) {
        let skills = d.join("skills");
        if !d.is_dir() || seen.iter().any(|s| same_file(s, &skills)) {
            continue;
        }
        out.push(UserSkill {
            harness: "Claude Code",
            paths: vec![SkillPath::new(&d, "skills/peat/SKILL.md")],
        });
        seen.push(skills);
    }
    if let Some(codex) = codex_home(h) {
        out.push(UserSkill {
            harness: "Codex",
            paths: vec![
                SkillPath::new(&h.home, ".agents/skills/peat/SKILL.md"),
                SkillPath::new(&codex, "skills/peat/SKILL.md"),
            ],
        });
    }
    out
}

/// How to get a user-level skill.
const SKILL_FIX: &str =
    "home-manager: programs.peat.skill.enable = true; otherwise run: peat skill --user";

/// The user-level skill, reported for `peat hook install [--check]`: a
/// note per harness, never drift. The hooks capture without it, and a
/// copy home-manager installed from another peat build differs from this
/// binary's by design, so neither may turn the check red.
pub fn skill_status(h: &Homes) -> Vec<Result<Option<String>, String>> {
    let mut out = Vec::new();
    for s in user_skills(h) {
        let mut found = false;
        for p in s.paths.iter().map(|p| &p.file) {
            let line = match read(p) {
                Ok(None) => continue,
                Ok(Some(t)) if t == SKILL => format!("✓ {}: peat skill current", p.display()),
                Ok(Some(_)) => format!(
                    "- {}: peat skill differs from this peat's — home-manager: rebuild after upgrading peat; otherwise run: peat skill --user",
                    p.display()
                ),
                Err(e) => format!("- {e}"),
            };
            found = true;
            out.push(Ok(Some(line)));
        }
        if !found {
            out.push(Ok(Some(format!(
                "- {}: no peat skill for {} — {SKILL_FIX}",
                s.paths[0].file.display(),
                s.harness
            ))));
        }
    }
    out
}

/// `peat skill --user`'s writes, one per harness, so no harness ever
/// reads two peat skills: a current copy anywhere is left as it is; a
/// path that is or sits in a symlink (below its root) means the skill is
/// managed elsewhere — home-manager's store links, a `skills` dir linked
/// into a shared hub — and the harness gets nothing; an existing regular
/// copy is rewritten in place; only a harness with no copy at all gets a
/// new file, at its first path.
pub fn write_user_skills(h: &Homes) -> Vec<Result<Option<String>, String>> {
    let skills = user_skills(h);
    if skills.is_empty() {
        return vec![Err(format!(
            "no Claude Code or Codex config dir under {}; nothing to write",
            h.home.display()
        ))];
    }
    skills
        .into_iter()
        .map(|s| {
            let holds = |p: &&SkillPath| std::fs::symlink_metadata(&p.file).is_ok();
            if let Some(p) = s
                .paths
                .iter()
                .find(|p| read(&p.file).ok().flatten().as_deref() == Some(SKILL))
            {
                return Ok(Some(format!("= {}: already current", p.file.display())));
            }
            if let Some(p) = s.paths.iter().find(|p| p.linked()) {
                return Ok(Some(format!(
                    "- {}: a symlink, managed elsewhere (e.g. home-manager's programs.peat.skill); {} left alone",
                    p.file.display(),
                    s.harness
                )));
            }
            let path = &s.paths.iter().find(holds).unwrap_or(&s.paths[0]).file;
            write_atomic_mode(path, SKILL, 0o644)?;
            Ok(Some(format!("✓ {}: written", path.display())))
        })
        .collect()
}

/// `peat skill --user`: the user-level skill for machines without the
/// home-manager module.
pub fn skill_user_cli() {
    let Some(homes) = Homes::from_env() else {
        crate::ui::error("HOME is not set; cannot find the user-level skill dirs");
        std::process::exit(1);
    };
    if !report(write_user_skills(&homes)) {
        std::process::exit(1);
    }
}

/// Create the ledger at `path` if absent (opening creates it, its
/// self-ignoring `.gitignore` and a current view-version marker); returns
/// whether it already existed. The one path by which a ledger comes to
/// exist.
pub fn init(path: &Path) -> bool {
    let existed = path.is_dir();
    drop(db::open_db(path.to_path_buf()));
    existed
}

/// `peat init`: opt this repo in. A jj workspace or git worktree opts in
/// its anchor repo, whose ledger every desk of it shares.
pub fn init_cli() {
    let path = db::init_path();
    if init(&path) {
        println!("= ledger already at {}", path.display());
    } else {
        println!("✓ ledger created at {}", path.display());
    }
    let wired = Homes::from_env().is_some_and(|h| {
        user_targets(&h).iter().any(|t| {
            read(&t.path)
                .ok()
                .flatten()
                .is_some_and(|s| installed_events(t.format, &s).len() == EVENTS.len())
        })
    });
    if !wired {
        println!(
            "  no user-level hook found, so nothing is captured yet — run `peat hook install` once per machine"
        );
    }
    if crate::hook::paused() {
        println!("  a .peat/off marker pauses capture here; delete it to resume");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("peat-install-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn homes(home: &Path) -> Homes {
        Homes {
            home: home.to_path_buf(),
            claude_config_dir: None,
            codex_home: None,
        }
    }

    /// What Codex's user config looks like with other tools' hooks, trust
    /// records and comments around it.
    const CODEX: &str = r#"model = "gpt"   # keep me

[hooks.state."/x/.codex/hooks.json:stop:0:0"]
trusted_hash = "sha256:abc"

# hail: agent messaging
[[hooks.SessionStart]]
hooks = [{ type = "command", command = "hail brief" }]

[[hooks.UserPromptSubmit]]
hooks = [{ type = "command", command = "peat hook" }, { type = "command", command = "hail deliver" }]

[[hooks.Stop]]
hooks = [{ type = "command", command = "in=$(cat); nohup peat capture \"$TP\" &" }]
"#;

    #[test]
    fn user_targets_cover_every_claude_dir_and_codex() {
        let home = tmp("targets");
        let h = homes(&home);
        let paths =
            |h: &Homes| -> Vec<PathBuf> { user_targets(h).into_iter().map(|t| t.path).collect() };
        assert_eq!(
            paths(&h),
            vec![home.join(".claude/settings.json")],
            "no Codex dir: Claude only"
        );
        std::fs::create_dir_all(home.join(".claude-work/projects")).unwrap();
        std::fs::write(home.join(".claude-work/settings.json"), "{}").unwrap();
        // a backup copy holds settings.json but no live sessions
        std::fs::create_dir_all(home.join(".claude-backups")).unwrap();
        std::fs::write(home.join(".claude-backups/settings.json"), "{}").unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let other = tmp("ccd");
        let h = Homes {
            claude_config_dir: Some(other.clone()),
            ..homes(&home)
        };
        assert_eq!(
            paths(&h),
            vec![
                home.join(".claude/settings.json"),
                other.join("settings.json"),
                home.join(".claude-work/settings.json"),
                home.join(".codex/config.toml"),
            ],
            "a sibling without settings.json and projects/ is not a config dir"
        );
        let h = Homes {
            claude_config_dir: Some(home.join(".claude")),
            codex_home: Some(other.clone()),
            ..homes(&home)
        };
        let t = user_targets(&h);
        assert_eq!(t.len(), 3, "CLAUDE_CONFIG_DIR=~/.claude is one dir: {t:?}");
        assert_eq!(t[2].path, other.join("config.toml"));
        assert_eq!(t[2].format, Format::Toml);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn fresh_install_writes_every_user_config_and_rerun_is_a_no_op() {
        let home = tmp("fresh");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let targets = user_targets(&homes(&home));
        for t in &targets {
            assert!(
                apply(t, Op::Install, true).is_err(),
                "{t:?}: nothing installed yet"
            );
            assert!(
                apply(t, Op::Install, false)
                    .unwrap()
                    .unwrap()
                    .starts_with("✓ ")
            );
            apply(t, Op::Install, true).unwrap();
            assert!(
                apply(t, Op::Install, false)
                    .unwrap()
                    .unwrap()
                    .starts_with("= "),
                "{t:?}: second run changes nothing"
            );
            let text = std::fs::read_to_string(&t.path).unwrap();
            assert_eq!(installed_events(t.format, &text), EVENTS);
        }
        let toml = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
        assert_eq!(
            toml,
            format!(
                "{TOML_BANNER}[[hooks.SessionStart]]\nhooks = [{{ type = \"command\", command = \"peat hook\" }}]\n\n\
[[hooks.Stop]]\nhooks = [{{ type = \"command\", command = \"peat hook\" }}]\n\n\
[[hooks.PreCompact]]\nhooks = [{{ type = \"command\", command = \"peat hook\" }}]\n\n\
[[hooks.SessionEnd]]\nhooks = [{{ type = \"command\", command = \"peat hook\" }}]\n"
            ),
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn toml_merge_keeps_foreign_hooks_comments_and_trust_records() {
        let Outcome::Write(out) = edit(Format::Toml, Some(CODEX), Op::Install).unwrap() else {
            panic!("an install into a config without peat changes it")
        };
        assert!(out.starts_with("model = \"gpt\"   # keep me\n"), "{out}");
        assert!(out.contains(
            "[hooks.state.\"/x/.codex/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:abc\""
        ));
        assert!(out.contains("# hail: agent messaging\n[[hooks.SessionStart]]\nhooks = [{ type = \"command\", command = \"hail brief\" }]"));
        assert!(
            out.contains("[[hooks.UserPromptSubmit]]\nhooks = [{ type = \"command\", command = \"hail deliver\" }]"),
            "peat leaves the retired event, the other tool stays: {out}"
        );
        assert!(!out.contains("nohup"), "the copied snippet is gone: {out}");
        assert_eq!(
            out.matches("command = \"peat hook\"").count(),
            EVENTS.len(),
            "{out}"
        );
        assert_eq!(out.matches(TOML_BANNER).count(), 1);
        assert_eq!(installed_events(Format::Toml, &out), EVENTS);
        assert_eq!(
            edit(Format::Toml, Some(&out), Op::Install).unwrap(),
            Outcome::Unchanged
        );
    }

    #[test]
    fn toml_strips_retired_events_entirely_when_only_peat_was_there() {
        let cfg = "[[hooks.PostToolUse]]\nhooks = [{ type = \"command\", command = \"peat hook\" }]\n\n\
[[hooks.Stop]]\nhooks = [{ type = \"command\", command = \"peat hook\" }]\n";
        let Outcome::Write(out) = edit(Format::Toml, Some(cfg), Op::Install).unwrap() else {
            panic!()
        };
        assert!(!out.contains("PostToolUse"), "{out}");
        assert_eq!(
            out.matches("[[hooks.Stop]]").count(),
            1,
            "present events are not duplicated: {out}"
        );
        // the nested array-of-tables spelling is recognised too
        let nested =
            "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"peat hook\"\n";
        assert!(installed_events(Format::Toml, nested).contains(&"Stop"));
        assert_eq!(
            edit(Format::Toml, Some(nested), Op::Strip).unwrap(),
            Outcome::Remove
        );
    }

    #[test]
    fn json_merge_keeps_foreign_hooks_and_strips_retired_events() {
        let mut cfg = serde_json::json!({
            "permissions": {"allow": ["Bash(ls)"]},
            "hooks": {
                "SessionStart": [
                    {"hooks": [{"type":"command","command":"bd prime"}]},
                    {"hooks": [{"type":"command","command":"in=$(cat); PEAT_DB=/x peat brief || true"}]}
                ],
                "Stop": [{"hooks": [{"type":"command","command":"peat hook","timeout":30}]}],
                "PostToolUse": [
                    {"matcher": "Bash", "hooks": [{"type":"command","command":"peat hook"}]}
                ],
                "Notification": [{"hooks": [{"type":"command","command":"say done"}]}]
            }
        });
        assert!(json::edit(&mut cfg, Op::Install, &mut vec![]).unwrap());
        assert_eq!(installed_events(Format::Json, &cfg.to_string()), EVENTS);
        assert_eq!(
            cfg["permissions"]["allow"][0], "Bash(ls)",
            "other keys survive"
        );
        let ss = cfg["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(
            ss[0]["hooks"][0]["command"], "bd prime",
            "other tools' hooks survive"
        );
        assert_eq!(
            ss.len(),
            2,
            "legacy peat group removed, one-verb group added"
        );
        assert_eq!(
            cfg["hooks"]["Stop"].as_array().unwrap().len(),
            1,
            "a peat hook with extra fields counts as installed"
        );
        assert!(cfg["hooks"].get("PostToolUse").is_none(), "{cfg}");
        assert_eq!(
            cfg["hooks"]["Notification"][0]["hooks"][0]["command"],
            "say done"
        );
        assert!(
            !json::edit(&mut cfg, Op::Install, &mut vec![]).unwrap(),
            "second merge changes nothing"
        );
        assert_eq!(snippet()["hooks"]["Stop"][0]["hooks"][0], entry());
    }

    #[test]
    fn check_reports_drift_per_file() {
        let home = tmp("drift");
        let t = &user_targets(&homes(&home))[0];
        std::fs::create_dir_all(t.path.parent().unwrap()).unwrap();
        std::fs::write(
            &t.path,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"peat hook"}]}]}}"#,
        )
        .unwrap();
        let e = apply(t, Op::Install, true).unwrap_err();
        assert!(
            e.contains("missing on SessionStart, PreCompact, SessionEnd"),
            "{e}"
        );
        apply(t, Op::Install, false).unwrap();
        // fully wired but a retired event still carries peat: drift too
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.path).unwrap()).unwrap();
        v["hooks"]["UserPromptSubmit"] = serde_json::json!([{"hooks":[entry()]}]);
        std::fs::write(&t.path, v.to_string()).unwrap();
        assert!(
            apply(t, Op::Install, true)
                .unwrap_err()
                .contains("stale peat entries")
        );
        std::fs::write(&t.path, "{not json").unwrap();
        assert!(
            apply(t, Op::Install, false)
                .unwrap_err()
                .contains("not valid JSON")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn project_copies_are_stripped_keeping_other_tools() {
        let root = tmp("project");
        let [claude, local, codex_json, codex_toml] = &project_targets(&root)[..] else {
            panic!()
        };
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::create_dir_all(root.join(".codex")).unwrap();
        std::fs::write(&claude.path, serde_json::to_string(&serde_json::json!({
            "permissions": {"allow": ["Bash(ls)"]},
            "hooks": {"SessionStart": [{"hooks": [{"type":"command","command":"bd prime"}, entry()]}]}
        })).unwrap()).unwrap();
        std::fs::write(&codex_json.path, serde_json::to_string(&snippet()).unwrap()).unwrap();
        std::fs::write(&codex_toml.path, CODEX).unwrap();

        assert!(
            apply(claude, Op::Strip, true)
                .unwrap()
                .is_some_and(|l| l.starts_with("- ") && l.contains("fires twice")),
            "a double-firing copy is a note, not drift"
        );
        assert_eq!(
            apply(local, Op::Strip, true),
            Ok(None),
            "an absent file is fine"
        );
        apply(claude, Op::Strip, false).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&claude.path).unwrap()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "permissions": {"allow": ["Bash(ls)"]},
                "hooks": {"SessionStart": [{"hooks": [{"type":"command","command":"bd prime"}]}]}
            })
        );
        assert!(
            apply(codex_json, Op::Strip, false)
                .unwrap()
                .unwrap()
                .contains("removed")
        );
        assert!(
            !codex_json.path.exists(),
            "a file that held only peat is removed"
        );
        apply(codex_toml, Op::Strip, false).unwrap();
        let toml = std::fs::read_to_string(&codex_toml.path).unwrap();
        assert!(!toml.contains("peat"), "{toml}");
        assert!(
            toml.contains("hail brief")
                && toml.contains("hail deliver")
                && toml.contains("trusted_hash")
        );
        for t in [claude, local, codex_json, codex_toml] {
            assert_eq!(
                apply(t, Op::Strip, true),
                Ok(None),
                "{t:?}: clean after the strip"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn project_install_round_trips_and_a_missing_skill_passes_check() {
        let dir = tmp("project-install");
        let t = Fabric::Claude.target(&dir);
        assert!(
            apply(&t, Op::Install, true).is_err(),
            "nothing installed yet"
        );
        apply(&t, Op::Install, false).unwrap();
        apply(&t, Op::Install, true).unwrap();
        assert!(apply(&Fabric::Codex.target(&dir), Op::Install, false).is_ok());
        assert!(dir.join(".codex/hooks.json").exists());
        assert!(
            install_skill(&dir, Fabric::Claude, true)
                .unwrap()
                .unwrap()
                .starts_with("- "),
            "a desk without the skill passes"
        );
        install_skill(&dir, Fabric::Claude, false).unwrap();
        install_skill(&dir, Fabric::Claude, true).unwrap();
        std::fs::write(dir.join(skill_path(Fabric::Claude)), "old").unwrap();
        assert!(
            install_skill(&dir, Fabric::Claude, true).is_err(),
            "a stale copy fails"
        );
        assert!(SKILL.starts_with("---\nname: peat\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn lines(rs: Vec<Result<Option<String>, String>>) -> Vec<String> {
        rs.into_iter().map(|r| r.unwrap().unwrap()).collect()
    }

    #[test]
    fn user_skill_is_found_per_harness_and_written_once() {
        let home = tmp("user-skill");
        for d in [".claude", ".claude-work/projects", ".codex"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        std::fs::write(home.join(".claude-work/settings.json"), "{}").unwrap();
        let h = Homes {
            // a second spelling of ~/.claude is one harness, not two
            claude_config_dir: Some(home.join(".claude/../.claude")),
            ..homes(&home)
        };
        let names: Vec<_> = user_skills(&h)
            .iter()
            .map(|s| s.paths[0].file.clone())
            .collect();
        assert_eq!(
            names,
            [
                home.join(".claude/skills/peat/SKILL.md"),
                home.join(".claude-work/skills/peat/SKILL.md"),
                home.join(".agents/skills/peat/SKILL.md"),
            ]
        );
        let none = lines(skill_status(&h));
        assert_eq!(none.len(), 3, "{none:?}");
        assert!(
            none.iter()
                .all(|l| l.starts_with("- ") && l.contains("programs.peat.skill.enable")),
            "a missing skill is a note naming the fix: {none:?}"
        );
        let wrote = lines(write_user_skills(&h));
        assert!(wrote.iter().all(|l| l.starts_with("✓ ")), "{wrote:?}");
        for p in &names {
            assert_eq!(std::fs::read_to_string(p).unwrap(), SKILL);
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644, "a skill is not secret: {}", p.display());
        }
        assert!(lines(skill_status(&h)).iter().all(|l| l.starts_with("✓ ")));
        assert!(
            lines(write_user_skills(&h))
                .iter()
                .all(|l| l.starts_with("= "))
        );
        // a copy from another peat build is reported, never drift
        std::fs::write(&names[1], "older").unwrap();
        let status = skill_status(&h);
        assert!(status.iter().all(Result::is_ok));
        assert!(lines(status)[1].contains("differs"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn codex_reads_either_skill_dir_and_a_linked_one_is_left_alone() {
        let home = tmp("user-skill-codex");
        std::fs::create_dir_all(home.join(".codex/skills/peat")).unwrap();
        std::fs::write(home.join(".codex/skills/peat/SKILL.md"), SKILL).unwrap();
        let h = homes(&home);
        assert_eq!(
            lines(skill_status(&h)),
            [format!(
                "✓ {}: peat skill current",
                home.join(".codex/skills/peat/SKILL.md").display()
            )],
            "~/.claude absent: no Claude Code skill expected"
        );
        assert!(lines(write_user_skills(&h))[0].starts_with("= "));
        assert!(!home.join(".agents").exists());
        // home-manager's shape: the skill dir is a symlink into the store
        std::fs::remove_dir_all(home.join(".codex/skills")).unwrap();
        let store = home.join("store/peat");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("SKILL.md"), "another build").unwrap();
        std::fs::create_dir_all(home.join(".agents/skills")).unwrap();
        std::os::unix::fs::symlink(&store, home.join(".agents/skills/peat")).unwrap();
        let wrote = lines(write_user_skills(&h));
        assert!(wrote[0].contains("left alone"), "{wrote:?}");
        assert_eq!(
            std::fs::read_to_string(store.join("SKILL.md")).unwrap(),
            "another build"
        );
        // home-manager links ~/.codex/skills/peat and ~/.agents is absent:
        // Codex already has a peat skill, so --user creates no second one
        std::fs::remove_dir_all(home.join(".agents")).unwrap();
        std::fs::create_dir_all(home.join(".codex/skills")).unwrap();
        std::os::unix::fs::symlink(&store, home.join(".codex/skills/peat")).unwrap();
        let wrote = lines(write_user_skills(&h));
        assert!(wrote[0].contains("left alone"), "{wrote:?}");
        assert!(!home.join(".agents").exists(), "{wrote:?}");
        // a stale regular copy there is rewritten in place, still one copy
        std::fs::remove_file(home.join(".codex/skills/peat")).unwrap();
        std::fs::create_dir_all(home.join(".codex/skills/peat")).unwrap();
        std::fs::write(home.join(".codex/skills/peat/SKILL.md"), "older").unwrap();
        let wrote = lines(write_user_skills(&h));
        assert!(wrote[0].starts_with("✓ "), "{wrote:?}");
        assert_eq!(
            std::fs::read_to_string(home.join(".codex/skills/peat/SKILL.md")).unwrap(),
            SKILL
        );
        assert!(!home.join(".agents").exists(), "{wrote:?}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_claude_skills_dir_linked_into_a_hub_is_never_written_through() {
        let home = tmp("user-skill-hub");
        let hub = home.join("hub");
        std::fs::create_dir_all(&hub).unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        // the shape claude-config tracks: ~/.claude/skills → a shared hub
        std::os::unix::fs::symlink(&hub, home.join(".claude/skills")).unwrap();
        let wrote = lines(write_user_skills(&homes(&home)));
        assert!(wrote[0].contains("left alone"), "{wrote:?}");
        assert!(!hub.join("peat").exists(), "{wrote:?}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn no_harness_means_nothing_to_write() {
        let home = tmp("user-skill-none");
        let h = homes(&home);
        assert!(skill_status(&h).is_empty());
        assert!(write_user_skills(&h)[0].is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn peat_is_matched_only_as_the_program_a_command_runs() {
        for foreign in [
            "repeat 3 say done",
            "make; speat 1",
            "echo peat hook",
            "hail deliver --format claude",
            "bd prime",
            // quoted text is never a command (all three were deleted once)
            "say \"peat finished\"",
            "osascript -e 'display notification \"peat run done\"'",
            "hail fyi peat 'build ok'",
            // a quoted string naming a real verb is still not a command
            "notify-send \"peat capture finished\"",
            // peat, but not a verb any hook ran
            "peat --version",
            "sh -c 'peat --help'",
        ] {
            assert!(!is_peat(foreign), "{foreign:?} is not peat");
        }
        for legacy in [
            "in=$(cat); PEAT_DB=/x peat brief || true",
            "in=$(cat); nohup peat capture \"$TP\" &",
            "sh -c 'peat capture x'",
            "sh -mc \"nohup peat capture \\\"$TP\\\" &\"",
            "bash -lc 'in=$(cat); peat obs x \"y\"'",
            "/usr/local/bin/peat capture x",
            "peat hook --event Stop",
        ] {
            assert!(is_legacy(legacy), "{legacy:?} is a copied snippet");
        }
        for current in [
            "peat hook",
            "/Users/m/.nix-profile/bin/peat hook",
            " peat  hook ",
        ] {
            assert!(is_current(current) && !is_legacy(current), "{current:?}");
        }
        // a full path to the binary is already the one-verb hook
        let cfg = serde_json::json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": "/nix/store/x/bin/peat hook"}]}]}});
        assert_eq!(
            installed_events(Format::Json, &cfg.to_string()),
            vec!["Stop"]
        );
        let mut v = cfg.clone();
        json::edit(&mut v, Op::Install, &mut vec![]).unwrap();
        assert_eq!(v["hooks"]["Stop"], cfg["hooks"]["Stop"], "not rewritten");
    }

    #[test]
    fn foreign_shapes_are_refused_not_repaired() {
        for (cfg, key) in [
            ("[1, 2]", "top level"),
            (r#"{"hooks": "x"}"#, "`hooks`"),
            (r#"{"hooks": {"Stop": {"hooks": []}}}"#, "`hooks.Stop`"),
        ] {
            for op in [Op::Install, Op::Strip] {
                let e = edit(Format::Json, Some(cfg), op).unwrap_err();
                assert!(
                    e.contains(key) && e.contains("not ours to repair"),
                    "{cfg}: {e}"
                );
            }
        }
        for (cfg, key) in [
            ("hooks = \"x\"\n", "`hooks`"),
            ("hooks = { Stop = [] }\n", "`hooks`"),
            ("[hooks.Stop]\ncommand = \"peat hook\"\n", "`hooks.Stop`"),
        ] {
            for op in [Op::Install, Op::Strip] {
                let e = edit(Format::Toml, Some(cfg), op).unwrap_err();
                assert!(
                    e.contains(key) && e.contains("not ours to repair"),
                    "{cfg}: {e}"
                );
            }
        }
        // and --check says so instead of a ✓
        let home = tmp("shapes");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let t = user_targets(&homes(&home)).pop().unwrap();
        std::fs::write(&t.path, "[hooks.Stop]\ncommand = \"peat hook\"\n").unwrap();
        assert!(apply(&t, Op::Install, true).is_err());
        assert!(apply(&t, Op::Install, false).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_unreadable_config_is_never_overwritten() {
        let home = tmp("unreadable");
        let t = &user_targets(&homes(&home))[0];
        std::fs::create_dir_all(t.path.parent().unwrap()).unwrap();
        let bytes = b"{\"model\": \"opus\xff\"}".to_vec();
        std::fs::write(&t.path, &bytes).unwrap();
        let e = apply(t, Op::Install, false).unwrap_err();
        assert!(e.contains("cannot read"), "{e}");
        assert_eq!(std::fs::read(&t.path).unwrap(), bytes, "left as it was");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Codex keys hook trust to `path:event:group:hook`, so a copied peat
    /// snippet is replaced where it stands and no foreign group moves.
    #[test]
    fn a_copied_snippet_is_replaced_in_place() {
        let cfg = r#"[[hooks.SessionStart]]
hooks = [{ type = "command", command = "in=$(cat); peat brief" }]

[[hooks.SessionStart]]
hooks = [{ type = "command", command = "hail brief" }]
"#;
        let Outcome::Write(out) = edit(Format::Toml, Some(cfg), Op::Install).unwrap() else {
            panic!()
        };
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        let ss = toml::commands(&doc["hooks"]["SessionStart"]);
        assert_eq!(
            ss,
            vec![
                Some(vec![Some(COMMAND.to_string())]),
                Some(vec![Some("hail brief".to_string())]),
            ],
            "peat stays group 0, hail stays group 1: {out}"
        );
        let mut v = serde_json::json!({"hooks": {"SessionStart": [
            {"hooks": [{"type": "command", "command": "nohup peat capture x &"}]},
            {"matcher": "", "hooks": [{"type": "command", "command": "bd prime"}]},
            {"hooks": [{"type": "command", "command": "PEAT_DB=/x peat brief"}]}
        ]}});
        json::edit(&mut v, Op::Install, &mut vec![]).unwrap();
        let ss = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(ss[0]["hooks"][0], entry(), "replaced in place");
        assert_eq!(
            ss[1]["hooks"][0]["command"], "bd prime",
            "foreign group keeps index 1"
        );
        assert_eq!(ss.len(), 2, "the second copy goes; nothing after it moved");
    }

    #[test]
    fn writes_are_atomic_keep_the_mode_and_follow_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let home = tmp("atomic");
        let real = home.join("dotfiles/settings.json");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, r#"{"model": "opus"}"#).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(&real, home.join(".claude/settings.json")).unwrap();
        let t = &user_targets(&homes(&home))[0];
        apply(t, Op::Install, false).unwrap();
        assert!(
            std::fs::symlink_metadata(&t.path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link stays a link"
        );
        let text = std::fs::read_to_string(&real).unwrap();
        assert_eq!(installed_events(Format::Json, &text), EVENTS);
        assert!(text.contains("\"model\": \"opus\""));
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            std::fs::read_dir(real.parent().unwrap()).unwrap().count(),
            1,
            "no temp file left behind"
        );
        // a read-only target (a home-manager store path) is refused
        std::fs::write(&real, "{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(
            apply(t, Op::Install, false)
                .unwrap_err()
                .contains("read-only")
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{}");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_file_is_rewritten_only_when_its_content_changes() {
        // already installed, keys in an order serde would not write
        let cfg = r#"{"model":"x","hooks":{"Stop":[{"hooks":[{"command":"peat hook","type":"command"}]}],
            "SessionStart":[{"hooks":[{"type":"command","command":"peat hook"}]}],
            "PreCompact":[{"hooks":[{"type":"command","command":"peat hook"}]}],
            "SessionEnd":[{"hooks":[{"type":"command","command":"peat hook"}]}]}}"#;
        assert_eq!(
            edit(Format::Json, Some(cfg), Op::Install).unwrap(),
            Outcome::Unchanged
        );
        // a no-op edit that would only re-serialize is not a write either
        let pretty = serde_json::to_string_pretty(&snippet()).unwrap() + "\n";
        assert_eq!(
            edit(Format::Json, Some(&pretty), Op::Install).unwrap(),
            Outcome::Unchanged
        );
    }

    #[test]
    fn untracked_repo_copies_are_stripped_and_tracked_ones_reported() {
        let root = tmp("copies");
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::create_dir_all(root.join(".codex")).unwrap();
        let peat = serde_json::to_string(&snippet()).unwrap();
        for rel in [
            ".claude/settings.json",
            ".claude/settings.local.json",
            ".codex/hooks.json",
        ] {
            std::fs::write(root.join(rel), &peat).unwrap();
        }
        let tracked = |_: &Path, rel: &str| rel == ".claude/settings.json";
        let lines: Vec<String> = project_copies(&root, false, false, tracked)
            .into_iter()
            .filter_map(|r| r.unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].starts_with("- ") && lines[0].contains("--strip-project"));
        assert!(
            root.join(".claude/settings.json").exists(),
            "tracked: left alone"
        );
        assert!(
            !root.join(".claude/settings.local.json").exists(),
            "untracked: stripped"
        );
        assert!(!root.join(".codex/hooks.json").exists());
        let _ = project_copies(&root, false, true, tracked);
        assert!(
            !root.join(".claude/settings.json").exists(),
            "--strip-project"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Removing peat from a retired event can move a foreign hook to a
    /// lower index; Codex keys trust to that index, so install says which.
    #[test]
    fn foreign_hooks_that_move_are_named_for_re_trust() {
        let home = tmp("retrust");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let t = user_targets(&homes(&home)).pop().unwrap();
        std::fs::write(
            &t.path,
            "[[hooks.UserPromptSubmit]]\nhooks = [{ type = \"command\", command = \"peat hook\" }, { type = \"command\", command = \"hail deliver\" }]\n",
        )
        .unwrap();
        let line = apply(&t, Op::Install, false).unwrap().unwrap();
        assert!(
            line.contains("re-trust in /hooks: UserPromptSubmit: hail deliver"),
            "{line}"
        );
        // an edit that moves nothing says nothing
        let again = apply(&t, Op::Install, false).unwrap().unwrap();
        assert!(!again.contains("re-trust"), "{again}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
