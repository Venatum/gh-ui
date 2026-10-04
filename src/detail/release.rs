//! A release's detail (`enter` on the Releases tab): what `gh release view`
//! returns, and the lines of its two sections, Notes and Assets.

use super::{cut_end, fact_label, facts_then_body, local_time};
use crate::model::Author;
use chrono::FixedOffset;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;

/// One release as `gh release view --json …` returns it: only the fields
/// the view shows (see `gh::RELEASE_DETAIL_FIELDS`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDetail {
    pub tag_name: String,
    /// Its title; GitHub shows the tag when it is empty.
    #[serde(default)]
    pub name: String,
    pub url: String,
    /// The release notes, in markdown.
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub is_prerelease: bool,
    /// `None` for a release published by an account since deleted.
    #[serde(default)]
    pub author: Option<Author>,
    /// `None` for a draft.
    #[serde(default)]
    pub published_at: Option<String>,
    /// The branch or commit the tag was made from.
    #[serde(default)]
    pub target_commitish: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// A file attached to a release.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub name: String,
    /// In bytes.
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub download_count: u64,
}

impl ReleaseDetail {
    /// What the view's border shows: the title, or the tag without one.
    pub fn title(&self) -> &str {
        if self.name.is_empty() {
            &self.tag_name
        } else {
            &self.name
        }
    }
}

/// The Notes section: the facts, a rule `width` wide, the release notes.
/// `offset` is the user's time zone, for the publication date.
pub fn notes_lines(d: &ReleaseDetail, width: u16, offset: FixedOffset) -> Vec<Line<'static>> {
    facts_then_body(facts_lines(d, offset), &d.body, width, "No release notes.")
}

/// One line per question: what kind of release, which tag, by whom and
/// when; made from what; what it ships and how much it was fetched.
fn facts_lines(d: &ReleaseDetail, offset: FixedOffset) -> Vec<Line<'static>> {
    let gray = Style::new().fg(Color::DarkGray);
    let (kind, color) = if d.is_draft {
        ("draft", Color::DarkGray)
    } else if d.is_prerelease {
        ("pre-release", Color::Yellow)
    } else {
        ("release", Color::Green)
    };
    let mut headline = format!(" · {}", d.tag_name);
    if let Some(author) = &d.author {
        headline.push_str(&format!(" · @{}", author.login));
    }
    if let Some(at) = &d.published_at {
        headline.push_str(&format!(" · {}", local_time(at, offset)));
    }

    let target = if d.target_commitish.is_empty() {
        Span::styled("-", gray)
    } else {
        Span::raw(d.target_commitish.clone())
    };
    let assets = if d.assets.is_empty() {
        Span::styled("none", gray)
    } else {
        let files = if d.assets.len() == 1 { "file" } else { "files" };
        let downloads: u64 = d.assets.iter().map(|a| a.download_count).sum();
        Span::raw(format!(
            "{} {files} · {downloads} downloads",
            d.assets.len()
        ))
    };
    vec![
        Line::from(vec![
            Span::styled(format!("● {kind}"), Style::new().fg(color)),
            Span::raw(headline),
        ]),
        Line::from(vec![fact_label("target"), target]),
        Line::from(vec![fact_label("assets"), assets]),
    ]
}

/// The Assets section: one file a line, its size and its downloads, the
/// columns lined up within `width`.
pub fn assets_lines(d: &ReleaseDetail, width: u16) -> Vec<Line<'static>> {
    if d.assets.is_empty() {
        return vec![Line::styled("No assets.", Style::new().fg(Color::DarkGray))];
    }
    // The name as wide as the longest, leaving the size and the count
    // their room: `  999.9 MB  99999 ↓` is 20 columns.
    let longest = d
        .assets
        .iter()
        .map(|a| a.name.chars().count())
        .max()
        .unwrap_or(0);
    let name_width = longest.min(usize::from(width).saturating_sub(20).max(10));
    let mut lines: Vec<Line<'static>> = d
        .assets
        .iter()
        .map(|asset| {
            Line::from(vec![
                Span::raw(format!(
                    "{:<name_width$}  ",
                    cut_end(&asset.name, name_width)
                )),
                Span::styled(
                    format!("{:>7}", human_size(asset.size)),
                    Style::new().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("  {:>5} ↓", asset.download_count),
                    Style::new().fg(Color::DarkGray),
                ),
            ])
        })
        .collect();
    let assets = if d.assets.len() == 1 {
        "asset"
    } else {
        "assets"
    };
    lines.push(Line::default());
    lines.push(Line::styled(
        format!(
            "{} {assets} · enter opens the release on GitHub",
            d.assets.len()
        ),
        Style::new().fg(Color::DarkGray),
    ));
    lines
}

/// A size as a person says it: `412 B`, `1.5 KB`, `8.5 MB`, `5.0 GB`
/// (powers of 1024, as file managers count).
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    // `as f64`: a size is far below where f64 loses whole bytes.
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail::{Detail, Section};

    fn sample() -> ReleaseDetail {
        serde_json::from_str(
            r###"{"tagName": "v2.4.0", "name": "Rate limit headers", "isDraft": false,
                "isPrerelease": false, "url": "https://github.com/acme/api/releases/tag/v2.4.0",
                "author": {"login": "alice"}, "publishedAt": "2026-10-02T09:08:00Z",
                "targetCommitish": "main",
                "body": "## What's changed\r\n\r\n- `X-RateLimit-*` headers on every response",
                "assets": [
                    {"name": "api-linux-amd64.tar.gz", "size": 8912345, "downloadCount": 1204},
                    {"name": "checksums.txt", "size": 412, "downloadCount": 97}
                ]}"###,
        )
        .unwrap()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.to_string().trim_end().to_string())
            .collect()
    }

    #[test]
    fn the_notes_open_on_the_facts() {
        let lines = texts(&notes_lines(&sample(), 20, utc()));
        assert_eq!(lines[0], "● release · v2.4.0 · @alice · 2026-10-02 09:08");
        assert_eq!(lines[1], "target   main");
        assert_eq!(lines[2], "assets   2 files · 1301 downloads");
        assert_eq!(lines[3], "─".repeat(20));
        assert_eq!(lines[5], "▍What's changed");
        assert_eq!(lines[7], "  • `X-RateLimit-*` headers on every response");
    }

    #[test]
    fn a_pre_release_and_an_empty_one_say_so() {
        let mut d = sample();
        d.is_prerelease = true;
        d.body.clear();
        d.assets.clear();
        let lines = texts(&notes_lines(&d, 20, utc()));
        assert!(lines[0].starts_with("● pre-release · v2.4.0"));
        assert_eq!(lines[2], "assets   none");
        assert_eq!(lines[5], "No release notes.");
    }

    /// Sizes read as a person would say them; the columns line up.
    #[test]
    fn the_assets_list_name_size_and_downloads() {
        assert_eq!(
            texts(&assets_lines(&sample(), 80)),
            [
                "api-linux-amd64.tar.gz   8.5 MB   1204 ↓",
                "checksums.txt             412 B     97 ↓",
                "",
                "2 assets · enter opens the release on GitHub",
            ]
        );
        let mut d = sample();
        d.assets.clear();
        assert_eq!(texts(&assets_lines(&d, 80)), ["No assets."]);
    }

    #[test]
    fn sizes_read_in_the_right_unit() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    /// The bar counts the assets; `enter` opens the release; a release
    /// without a name is named by its tag, as on GitHub.
    #[test]
    fn the_view_counts_the_assets_and_opens_the_release() {
        let detail = Detail::Release(sample());
        assert_eq!(detail.count(Section::Assets), Some(2));
        assert_eq!(detail.count(Section::Notes), None);
        assert_eq!(
            detail.url(Section::Assets),
            "https://github.com/acme/api/releases/tag/v2.4.0"
        );
        assert_eq!(detail.title(), "Rate limit headers");
        let mut d = sample();
        d.name.clear();
        assert_eq!(Detail::Release(d).title(), "v2.4.0");
    }
}
