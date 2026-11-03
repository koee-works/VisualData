//! Comments on steps, sources and the analysis, kept in the project's
//! `comments.json`, so they travel with the project.
//!
//! - Not part of the analysis definition: undo never removes a comment, and
//!   comments never outdate a result.
//! - A comment on a step that is later deleted is kept (and shown again if
//!   the deletion is undone); a renamed source's comments follow it.
//! - The author is the computer's user name. Anyone can reply to or resolve
//!   a comment; only its author can delete it.

use crate::{fail, App, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommentView {
    pub id: String,
    /// `step:<id>`, `source:<id>` or `analysis`.
    pub target: String,
    /// The comment this replies to, if it is a reply.
    #[serde(default)]
    pub reply_to: Option<String>,
    pub author: String,
    pub time_unix_ms: u64,
    pub text: String,
    #[serde(default)]
    pub resolved: bool,
}

pub const MAX_COMMENT_CHARS: usize = 5_000;

/// The computer's user name, or "you".
pub fn current_author() -> String {
    std::env::var("VDA_AUTHOR")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "you".into())
}

fn path(project: &Path) -> std::path::PathBuf {
    project.join("comments.json")
}

pub fn load(project: &Path) -> Vec<CommentView> {
    std::fs::read_to_string(path(project)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(project: &Path, list: &[CommentView]) -> Result<()> {
    let tmp = project.join("comments.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(list).unwrap())?;
    std::fs::rename(&tmp, path(project))?;
    Ok(())
}

impl App {
    fn project_path(&mut self) -> Result<std::path::PathBuf> {
        Ok(self.session()?.project_dir().to_path_buf())
    }

    pub fn comments(&mut self) -> Result<Vec<CommentView>> {
        Ok(load(&self.project_path()?))
    }

    /// Add a comment on `target`, or a reply to another comment.
    pub fn add_comment(&mut self, target: &str, text: &str, reply_to: Option<String>) -> Result<Vec<CommentView>> {
        let project = self.project_path()?;
        let text = text.trim();
        if text.is_empty() {
            return fail("write a comment first");
        }
        if text.chars().count() > MAX_COMMENT_CHARS {
            return fail(format!("a comment can be at most {MAX_COMMENT_CHARS} characters"));
        }
        let mut list = load(&project);
        // A reply is on its comment's target (even a step deleted since).
        let target = match &reply_to {
            Some(parent) => match list.iter().find(|c| &c.id == parent) {
                Some(p) => p.target.clone(),
                None => return fail("the comment being replied to no longer exists"),
            },
            None => {
                let a = &self.session()?.analysis;
                let known = target == "analysis"
                    || target.strip_prefix("step:").is_some_and(|id| a.step(id).is_some())
                    || target.strip_prefix("source:").is_some_and(|id| a.is_source(id));
                if !known {
                    return fail(format!("cannot comment on `{target}`: no such step or source"));
                }
                target.to_string()
            }
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let n = list.iter().filter_map(|c| c.id.strip_prefix('c')?.parse::<u64>().ok()).max().unwrap_or(0) + 1;
        list.push(CommentView { id: format!("c{n}"), target, reply_to, author: self.author.clone(), time_unix_ms: now, text: text.into(), resolved: false });
        save(&project, &list)?;
        Ok(list)
    }

    /// Mark a comment (and its replies) resolved, or open again.
    pub fn resolve_comment(&mut self, id: &str, resolved: bool) -> Result<Vec<CommentView>> {
        let project = self.project_path()?;
        let mut list = load(&project);
        if !list.iter().any(|c| c.id == id) {
            return fail("that comment no longer exists");
        }
        for c in list.iter_mut().filter(|c| c.id == id || c.reply_to.as_deref() == Some(id)) {
            c.resolved = resolved;
        }
        save(&project, &list)?;
        Ok(list)
    }

    /// Delete your own comment, with its replies.
    pub fn delete_comment(&mut self, id: &str) -> Result<Vec<CommentView>> {
        let project = self.project_path()?;
        let mut list = load(&project);
        let Some(c) = list.iter().find(|c| c.id == id) else { return fail("that comment no longer exists") };
        if c.author != self.author {
            return fail(format!("only {} can delete this comment; you can resolve it instead", c.author));
        }
        list.retain(|c| c.id != id && c.reply_to.as_deref() != Some(id));
        save(&project, &list)?;
        Ok(list)
    }

    /// A source was renamed: its comments follow it.
    pub(crate) fn retarget_comments(&mut self, old: &str, new: &str) -> Result<()> {
        let project = self.project_path()?;
        let mut list = load(&project);
        let (from, to) = (format!("source:{old}"), format!("source:{new}"));
        let mut changed = false;
        for c in list.iter_mut().filter(|c| c.target == from) {
            c.target = to.clone();
            changed = true;
        }
        if changed {
            save(&project, &list)?;
        }
        Ok(())
    }
}
