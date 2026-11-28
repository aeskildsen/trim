//! Output for a person: one group of lines for each repository, with a
//! heading, colours and a summary line. The output for a pipe is in main.rs.

use crate::detect::How;
use crate::{REMOTE, RepoResult};
use anstyle::{AnsiColor, Style};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const DELETED: Style = AnsiColor::Red.on_default();
const KEPT: Style = AnsiColor::Yellow.on_default();
const SWITCHED: Style = AnsiColor::Blue.on_default();

/// Writes text with styles, or without styles if colour is off.
pub struct Painter {
    pub colour: bool,
}

impl Painter {
    fn paint(&self, style: Style, text: &str) -> String {
        if self.colour {
            format!("{style}{text}{style:#}")
        } else {
            text.to_owned()
        }
    }

    /// A branch name, padded to `width`. The remote prefix is dimmed, so
    /// that remote branches are different from local branches.
    fn branch(&self, name: &str, width: usize) -> String {
        let pad = " ".repeat(width.saturating_sub(name.chars().count()));
        match name.strip_prefix(REMOTE).and_then(|n| n.strip_prefix('/')) {
            Some(rest) => format!("{}{rest}{pad}", self.paint(DIM, &format!("{REMOTE}/"))),
            None => format!("{name}{pad}"),
        }
    }

    fn how(&self, how: How, width: usize) -> String {
        let style = match how {
            How::Merged => AnsiColor::Green,
            How::Rebased => AnsiColor::Cyan,
            How::Squashed => AnsiColor::Magenta,
        }
        .on_default();
        self.paint(style, &format!("{:width$}", how.as_str()))
    }

    fn heading(&self, path: &Path) -> String {
        let full = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let name = full.file_name().map_or_else(
            || full.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        format!(
            "{}  {}",
            self.paint(BOLD, &name),
            self.paint(DIM, &home_relative(&full))
        )
    }
}

/// The path with the home directory as `~`.
fn home_relative(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rest) = path.strip_prefix(&home)
    {
        if rest.as_os_str().is_empty() {
            return "~".to_owned();
        }
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

fn plural(n: usize, word: &str, words: &str) -> String {
    format!("{n} {}", if n == 1 { word } else { words })
}

/// " in 2 of 5 repositories", or nothing if trim checked one repository.
fn where_(some: usize, all: usize) -> String {
    if all == 1 {
        String::new()
    } else {
        format!(" in {some} of {all} repositories")
    }
}

fn width<'a>(cells: impl Iterator<Item = &'a str>) -> usize {
    cells.map(|c| c.chars().count()).max().unwrap_or(0)
}

/// The branches that trim found, in groups by repository.
pub fn list(
    out: &mut impl Write,
    p: &Painter,
    results: &[(PathBuf, RepoResult)],
    now: i64,
) -> io::Result<()> {
    let mut stale = 0;
    let mut repos_with_stale = 0;
    for (path, r) in results.iter().filter(|(_, r)| !r.found.is_empty()) {
        let wb = width(r.found.iter().map(|f| f.branch.as_str()));
        let wh = width(r.found.iter().map(|f| f.how.as_str()));
        writeln!(out, "{}", p.heading(path))?;
        for f in &r.found {
            writeln!(
                out,
                "  {}  {}  {}",
                p.branch(&f.branch, wb),
                p.how(f.how, wh),
                p.paint(DIM, &crate::age(now - f.time))
            )?;
        }
        writeln!(out)?;
        stale += r.found.len();
        repos_with_stale += 1;
    }

    let scanned = plural(results.len(), "repository", "repositories");
    if stale == 0 {
        writeln!(out, "No stale branches in {scanned}.")
    } else {
        writeln!(
            out,
            "{}{}. Run with --delete to remove {}.",
            plural(stale, "stale branch", "stale branches"),
            where_(repos_with_stale, results.len()),
            if stale == 1 { "it" } else { "them" }
        )
    }
}

/// The branches that trim deleted or kept, in groups by repository.
pub fn deleted(
    out: &mut impl Write,
    p: &Painter,
    results: &[(PathBuf, RepoResult)],
) -> io::Result<()> {
    let mut deleted = 0;
    let mut kept = 0;
    // Kept branches that a run with `--switch` can delete.
    let mut switchable = 0;
    let mut repos_with_deleted = 0;
    for (path, r) in results
        .iter()
        .filter(|(_, r)| !r.deleted.is_empty() || !r.kept.is_empty() || r.switched.is_some())
    {
        let all = || {
            r.deleted
                .iter()
                .map(|d| &d.found)
                .chain(r.kept.iter().map(|k| &k.found))
        };
        let wb = width(all().map(|f| f.branch.as_str()));
        let wh = width(all().map(|f| f.how.as_str()));
        writeln!(out, "{}", p.heading(path))?;
        if let Some(to) = &r.switched {
            writeln!(out, "  {} to {to}", p.paint(SWITCHED, "switched"))?;
        }
        for d in &r.deleted {
            writeln!(
                out,
                "  {}  {}  {}  {}",
                p.paint(DELETED, "deleted"),
                p.branch(&d.found.branch, wb),
                p.how(d.found.how, wh),
                p.paint(DIM, &format!("was {}", d.tip))
            )?;
        }
        for k in &r.kept {
            let note = match &k.why {
                Some(why) => format!("checked out; not switched: {why}"),
                None => "checked out".to_owned(),
            };
            writeln!(
                out,
                "  {}  {}  {}  {}",
                p.paint(KEPT, "kept   "),
                p.branch(&k.found.branch, wb),
                p.how(k.found.how, wh),
                p.paint(DIM, &note)
            )?;
        }
        writeln!(out)?;
        deleted += r.deleted.len();
        kept += r.kept.len();
        switchable += r.kept.iter().filter(|k| k.here && k.why.is_none()).count();
        repos_with_deleted += usize::from(!r.deleted.is_empty());
    }

    let scanned = plural(results.len(), "repository", "repositories");
    if deleted == 0 && kept == 0 {
        return writeln!(out, "No stale branches in {scanned}.");
    }
    write!(
        out,
        "Deleted {}{}.",
        plural(deleted, "branch", "branches"),
        where_(repos_with_deleted, results.len())
    )?;
    if kept > 0 {
        write!(
            out,
            " Kept {}.",
            plural(kept, "checked-out branch", "checked-out branches")
        )?;
    }
    if switchable > 0 {
        write!(
            out,
            " Run with --switch to switch away and delete {}.",
            if switchable == 1 { "it" } else { "them" }
        )?;
    }
    writeln!(out)
}
