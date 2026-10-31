//! Help menu: community and project links (the frontend opens `Event::OpenUrl`).

use serde_json::{Value, json};

use super::{CommandSpec, always, str_p};
use crate::links;
use crate::{Event, Result, Session, cmd};

fn open(s: &mut Session, url: String) -> Result<Value> {
    s.events.push(Event::OpenUrl(url.clone()));
    Ok(json!({"url": url}))
}

fn sibling(s: &mut Session, p: &Value) -> Result<Value> {
    let slug = str_p(p, "app").unwrap_or("photocraft").to_ascii_lowercase();
    if str_p(p, "kind") == Some("github") { open(s, links::github_of(&slug)) } else { open(s, links::page_of(&slug)) }
}

/// Help ▸ EffectCraft Help / Scripting Help / Expression Reference / Effect Reference.
fn docs(s: &mut Session, p: &Value) -> Result<Value> {
    let page = match str_p(p, "page").unwrap_or("help") {
        "scripting" => "docs/control-protocol.md",
        "expressions" => "docs/expressions.md",
        "effects" => "docs/effects.md",
        "agents" => "docs/agents.md",
        _ => "README.md",
    };
    open(s, format!("{}/blob/main/{page}", links::GITHUB))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("help.docs", "EffectCraft Help...", [], Some("F1"), "{page?: help|scripting|expressions|effects|agents}", always, docs),
        cmd!("help.discord", "Join the ArtCraft Discord...", ["Help"], None, "{}", always, |s, _| open(s, links::DISCORD.into())),
        cmd!("help.website", "ArtCraft Website", ["Help"], None, "{}", always, |s, _| open(s, links::WEBSITE.into())),
        cmd!("help.appPage", "EffectCraft Home Page", ["Help"], None, "{}", always, |s, _| open(s, links::APP_PAGE.into())),
        cmd!("help.github", "EffectCraft on GitHub", ["Help"], None, "{}", always, |s, _| open(s, links::GITHUB.into())),
        cmd!("help.onlineTutorials", "Online Tutorials...", ["Help"], None, "{}", always, |s, _| open(s, links::APP_PAGE.into())),
        cmd!("help.inAppTutorials", "In-App Tutorials...", ["Help"], None, "{} → the Home screen's Learn tab", always, |s, p| super::frontend(
            s,
            "help.inAppTutorials",
            p
        )),
        cmd!("help.reportIssue", "Provide Feedback...", ["Help"], None, "{}", always, |s, _| open(s, links::ISSUES.into())),
        cmd!(
            "help.sibling",
            "Other ArtCraft Apps",
            [],
            None,
            "{app: photocraft|vectorcraft|filmcraft|lightcraft|pdfcraft|designcraft, kind?: page|github}",
            always,
            sibling
        ),
    ]
}
