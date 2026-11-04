//! Templates: a quest shape written once with `$parameters` and used by many quests.
//!
//! A `template` has its `template_param`s and the sets and actions of an ordinary quest
//! (`template_set`, `template_action`), any field of which may hold `$name` for a parameter. A
//! `template_use` names a template and gives every parameter a value (`template_arg`); at load
//! each use becomes ordinary sets named `<use>.<set>` with their actions, the parameters filled
//! in, and a branch or Goto naming one of the template's own sets naming the use's copy of it.
//! The use's sets run at its `priority` plus each set's own. The rows are then checked as any
//! other (their errors name the use and the template row), and the engine sees only plain sets.
use std::collections::{BTreeMap, BTreeSet};
use std::io;

use rusqlite::Connection;

use super::Row;
use super::db::{shape, table};
use super::spec::{Kind, spec};

/// The sets (with their priority) and actions the uses make, in TSV column order.
#[derive(Default)]
pub(super) struct Expanded { pub(super) sets: Vec<(i64, Row)>, pub(super) actions: Vec<Row> }

/// `text` with every `$name` replaced by its argument.
fn fill(text: &str, args: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let end = tail.find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')).unwrap_or(tail.len());
        let name = &tail[..end];
        if name.is_empty() { return Err("a `$` names no parameter".into()); }
        out.push_str(args.get(name).ok_or_else(|| format!("`${name}` is not a parameter of the template"))?);
        rest = &tail[end..];
    }
    out.push_str(rest);
    Ok(out)
}

pub(super) fn expand(db: &Connection) -> io::Result<Expanded> {
    let mut out = Expanded::default();
    let uses = table(db, shape("template_use"), &[])?;
    if uses.is_empty() { return Ok(out); }
    let mut params: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in table(db, shape("template_param"), &[])? { params.entry(f[0].clone()).or_default().push(f[1].clone()); }
    let mut sets: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    for f in table(db, shape("template_set"), &[])? { sets.entry(f[0].clone()).or_default().push(f); }
    let mut actions: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    for f in table(db, shape("template_action"), &[])? { actions.entry(f[0].clone()).or_default().push(f); }
    let mut args: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for f in table(db, shape("template_arg"), &[])? { args.entry(f[0].clone()).or_default().insert(f[1].clone(), f[2].clone()); }
    for u in uses {
        let (name, template, base, source) = (&u[0], &u[1], u[2].parse::<i64>().unwrap_or(0), &u[3]);
        let at = format!("quests template_use {name}");
        let bad = |why: String| io::Error::other(format!("{at}: {why}"));
        let wanted: BTreeSet<&String> = params.get(template).map(|p| p.iter().collect()).unwrap_or_default();
        let given = args.remove(name).unwrap_or_default();
        if let Some(missing) = wanted.iter().find(|p| !given.contains_key(p.as_str())) {
            return Err(bad(format!("template {template} needs `{missing}` (a template_arg row)")));
        }
        if let Some(extra) = given.keys().find(|k| !wanted.contains(k)) {
            return Err(bad(format!("`{extra}` is not a parameter of template {template}")));
        }
        let own: BTreeSet<&str> = sets.get(template).map(|s| s.iter().map(|f| f[1].as_str()).collect()).unwrap_or_default();
        if own.is_empty() { return Err(bad(format!("template {template} has no sets"))); }
        for f in sets.get(template).into_iter().flatten() {
            let row_at = format!("{at} (template {template} set {})", f[1]);
            let filled = f[3..12].iter().map(|v| fill(v, &given)).collect::<Result<Vec<_>, _>>()
                .map_err(|why| io::Error::other(format!("{row_at}: {why}")))?;
            let priority = base + f[2].parse::<i64>().unwrap_or(0);
            let mut row = vec![format!("{name}.{}", f[1])];
            row.extend(filled);
            row.push(format!("{}; template_use {name}: {source}", f[12]));
            out.sets.push((priority, (row_at, row)));
        }
        for f in actions.get(template).into_iter().flatten() {
            let row_at = format!("{at} (template {template} set {} action {})", f[1], f[2]);
            let why = |w: String| io::Error::other(format!("{row_at}: {w}"));
            let action = fill(&f[3], &given).map_err(why)?;
            let mut p: Vec<String> = f[5..10].iter().map(|v| fill(v, &given)).collect::<Result<_, _>>().map_err(why)?;
            // A branch or Goto to one of the template's own sets goes to this use's copy.
            if let Some(spec) = spec(&action) {
                for (i, param) in spec.params.iter().enumerate() {
                    if param.kind == Kind::Set && own.contains(p[i].as_str()) { p[i] = format!("{name}.{}", p[i]); }
                }
            }
            let mut row = vec![format!("{name}.{}", f[1]), f[2].clone(), action, fill(&f[4], &given).map_err(why)?];
            row.extend(p);
            row.push(format!("{}; template_use {name}: {source}", f[10]));
            out.actions.push((row_at, row));
        }
    }
    if let Some(orphan) = args.keys().next() {
        return Err(io::Error::other(format!("quests template_arg {orphan}: no template_use of that name")));
    }
    Ok(out)
}
