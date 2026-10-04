//! The markdown pass, lightly: a PR, issue, comment or release body as
//! lines to draw. Markdown keeps its line structure; the HTML that GitHub
//! bodies carry (Dependabot's release notes, `<details>` blocks) is read by
//! a small tolerant tag reader. Not an HTML engine: the fifteen-odd tags
//! such bodies use, each mapped to what the markdown already looks like.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

// --- cleaning the text ---

/// Makes untrusted text safe and faithful to draw. ratatui already drops
/// every control character (so no escape sequence reaches the terminal),
/// but silently: a tab-indented code block would lose its indentation and
/// a lone `\r` would glue two lines. So line ends are normalized and tabs
/// expanded first. Bidirectional overrides are not control characters and
/// would get through, reordering what a line shows ("trojan source"):
/// they go too, and so do the characters that take no room, which a
/// terminal may draw as a box.
fn sanitize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
        .chars()
        .filter(|&c| c == '\n' || !is_unsafe(c))
        .collect()
}

/// What never reaches the screen: control characters, bidi overrides,
/// invisible characters.
fn is_unsafe(c: char) -> bool {
    c.is_control() || is_bidi_override(c) || is_invisible(c)
}

/// Characters of no width that only steer layout: the soft hyphen, the
/// zero-width space and non-joiner, the direction marks, the word joiner
/// and the invisible operators, the byte order mark. Not the zero-width
/// joiner: it glues an emoji family (👩‍💻) into one picture.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{AD}' | '\u{180E}' | '\u{200B}' | '\u{200C}' | '\u{200E}' | '\u{200F}' | '\u{2060}'
            ..='\u{2064}' | '\u{FEFF}'
    )
}

/// The embeddings, overrides and isolates of Unicode's bidi algorithm.
fn is_bidi_override(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A line that opens or closes a fenced code block.
fn is_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// Removes every `<!-- … -->`, even across lines, as GitHub hides them. A
/// PR template is mostly such comments: without this pass the screen shows
/// the template's instructions instead of the PR. An unterminated comment
/// hides the rest, as on GitHub. Fenced code is left alone: a comment
/// there is code.
fn strip_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_comment = false;
    let mut in_fence = false;
    for (i, line) in text.split('\n').enumerate() {
        // Inside a comment, the line break goes with the hidden text: the
        // text before the comment and the text after it join on one line.
        if i > 0 && !in_comment {
            out.push('\n');
        }
        // A fence line, and everything between two of them, is kept as is.
        if !in_comment && is_fence(line) {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        let mut rest = line;
        loop {
            if in_comment {
                let Some(end) = rest.find("-->") else {
                    break; // the whole rest of the line is hidden
                };
                rest = &rest[end + "-->".len()..];
                in_comment = false;
            } else if let Some(start) = rest.find("<!--") {
                out.push_str(&rest[..start]);
                rest = &rest[start + "<!--".len()..];
                in_comment = true;
            } else {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

/// `text` with its character references decoded (`&amp;`, `&#39;`,
/// `&#x27;`…), as GitHub shows them; an unknown one stays as typed. Done
/// on text already cut into pieces, so a decoded `&lt;b&gt;` is never read
/// as a tag; and cleaned again after, so `&#27;` cannot bring back the
/// escape character `sanitize` removed.
fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match entity(rest) {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out.retain(|c| !is_unsafe(c));
    out
}

/// The character reference at the start of `s` and its length: the few
/// names bodies use, and any number (`&#8212;`, `&#x2014;`).
fn entity(s: &str) -> Option<(char, usize)> {
    // The longest, `&#x10FFFF;`, ends at byte 9.
    let end = s.find(';').filter(|&end| end <= 10)?;
    let c = match &s[1..end] {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        name => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((c, end + 1))
}

// --- the source lines ---

/// A body as lines to draw, lightly styled, one source line at a time:
/// headings, list items, quotes, rules, tables and fenced code read at a
/// glance, in markdown or in HTML; bold, italic, struck text and code
/// too, emoji codes as emoji, task boxes as boxes. A link shows its
/// URL, whole, for the terminal to open. A `<details>` block is its
/// summary on one `▸` line, or with `show_folded` (`space`) a `▾` line over
/// its content. Line breaks are kept, as GitHub shows them in a PR body:
/// wrapping is the `Paragraph`'s job. `width` is the room the lines get:
/// a table wider than that reads row by row.
pub fn markdown_lines(body: &str, width: u16, show_folded: bool) -> Vec<Line<'static>> {
    let text = strip_html_comments(&sanitize(body));
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Writer::new(show_folded, width);
    let mut in_fence = false;
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        at += 1;
        if is_fence(line) {
            in_fence = !in_fence;
        } else if in_fence {
            out.code_line(line);
        } else if line.trim().is_empty() {
            out.blank();
        } else if let Some((table, len)) = table_at(&lines[at - 1..]) {
            out.table(&table);
            at += len - 1;
        } else if is_link_definition(line) {
            // Not shown on GitHub either: Dependabot's `[//]: # (…)` markers.
        } else if let Some(title) = heading(line) {
            out.heading(title);
        } else {
            out.prose(line);
        }
    }
    out.finish()
}

/// The text of a `#` … `######` heading.
fn heading(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let level = line.chars().take_while(|&c| c == '#').count();
    let title = line[level..].strip_prefix(' ')?;
    (1..=6).contains(&level).then(|| title.trim())
}

/// `---`, `***` or `___` (three or more): a thematic break.
fn is_rule(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&mark| line.chars().all(|c| c == mark))
}

/// `[label]: url`, a link reference definition: what a `[text][label]`
/// link points to, never shown itself.
fn is_link_definition(line: &str) -> bool {
    line.trim_start()
        .strip_prefix('[')
        .and_then(|rest| rest.find("]:").map(|end| &rest[..end]))
        .is_some_and(|label| !label.is_empty() && !label.contains(['[', ']']))
}

// --- tables ---

/// How a column lines its cells up, from the colons of its delimiter cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// A table as typed: its cells' markdown, not read yet.
struct Table {
    header: Vec<String>,
    aligns: Vec<Align>,
    rows: Vec<Vec<String>>,
}

/// The table that starts at `lines[0]`, as GitHub reads one: a header row,
/// a delimiter row with as many cells (`| --- | :---: |`), then every row
/// up to a blank line or one without a `|`. The table, and how many lines
/// it takes.
fn table_at(lines: &[&str]) -> Option<(Table, usize)> {
    let header = cells(lines.first().filter(|line| line.contains('|'))?);
    let aligns = alignments(lines.get(1)?)?;
    if aligns.len() != header.len() {
        return None;
    }
    let rows: Vec<Vec<String>> = lines[2..]
        .iter()
        .take_while(|line| line.contains('|') && !line.trim().is_empty())
        .map(|line| cells(line))
        .collect();
    let len = 2 + rows.len();
    Some((
        Table {
            header,
            aligns,
            rows,
        },
        len,
    ))
}

/// A row's cells, trimmed: split on its `|`, the outer ones optional, an
/// escaped `\|` being a pipe in the cell (even in code, as on GitHub).
fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = match line.strip_suffix('|') {
        Some(inner) if !inner.ends_with('\\') => inner,
        _ => line,
    };
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cell)),
            c => cell.push(c),
        }
    }
    cells.push(cell);
    cells.iter().map(|cell| cell.trim().to_string()).collect()
}

/// The alignments a delimiter row sets (`---`, `:---`, `:---:`, `---:`),
/// or `None` when `line` is no delimiter row.
fn alignments(line: &str) -> Option<Vec<Align>> {
    if !line.contains('|') {
        return None;
    }
    cells(line)
        .iter()
        .map(|cell| {
            let left = cell.starts_with(':');
            let right = cell.len() > 1 && cell.ends_with(':');
            let dashes = cell.trim_start_matches(':').trim_end_matches(':');
            if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
                return None;
            }
            Some(match (left, right) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

// --- the inline reader ---

/// What may start at a `` ` ``, `!`, `[`, `<`, `*`, `_`, `~` or `:` of a
/// line of prose.
enum Inline<'a> {
    /// `` `code` `` (or ``` ``co`de`` ```), the code inside its backticks:
    /// shown as typed, nothing read inside.
    Code(&'a str),
    /// `![alt](url)`: an image's URL means nothing in a terminal.
    Image(&'a str),
    /// `[label](url)`.
    Link {
        label: &'a str,
        url: &'a str,
    },
    Tag(Tag<'a>),
    /// `**text**` or `__text__`: the text inside, bold.
    Strong(&'a str),
    /// `*text*` or `_text_`: the text inside, italic.
    Emphasis(&'a str),
    /// `~~text~~`: the text inside, struck through.
    Strike(&'a str),
    /// `<https://…>`: the URL, without its angle brackets.
    Autolink(&'a str),
    /// `:rocket:`: its emoji.
    Emoji(&'static str),
}

/// The inline piece at the start of `s`, and its length; `None` when what
/// starts there is plain text after all. `after_word`: a letter or a digit
/// comes just before, which an `_` cannot open emphasis after.
fn inline_at(s: &str, after_word: bool) -> Option<(Inline<'_>, usize)> {
    match s.as_bytes().first()? {
        b'`' => code_span(s).map(|len| (Inline::Code(code_text(&s[..len])), len)),
        b'!' => markdown_image(s).map(|(alt, len)| (Inline::Image(alt), len)),
        b'[' => markdown_link(s),
        b'<' => Tag::read(s)
            .map(|(tag, len)| (Inline::Tag(tag), len))
            .or_else(|| autolink(s)),
        b'*' | b'~' => emphasis(s),
        b'_' | b':' if after_word => None,
        b'_' => emphasis(s),
        b':' => emoji_at(s),
        _ => None,
    }
}

/// The code of a code span, backticks included: what is inside them, less
/// the one space each side that lets code start or end with a backtick
/// (``` `` `a` `` ```), as GitHub reads it.
fn code_text(span: &str) -> &str {
    let ticks = backtick_run(span);
    let inner = &span[ticks..span.len() - ticks];
    match inner.strip_prefix(' ').and_then(|i| i.strip_suffix(' ')) {
        Some(trimmed) if !inner.trim().is_empty() => trimmed,
        _ => inner,
    }
}

/// `<https://…>` (or `http://`) at the start of `s`, and its length: a
/// link that is its URL. No space inside, as GitHub requires.
fn autolink(s: &str) -> Option<(Inline<'_>, usize)> {
    let end = s.find('>')?;
    let url = &s[1..end];
    let web = url.starts_with("https://") || url.starts_with("http://");
    (web && !url.contains(char::is_whitespace)).then_some((Inline::Autolink(url), end + 1))
}

/// `:name:` at the start of `s`, and its length, when `name` is one of
/// `EMOJI`; anything else stays as typed (`10:30:00`, `std::io`).
fn emoji_at(s: &str) -> Option<(Inline<'_>, usize)> {
    let end = 1 + s[1..].find(':')?;
    let name = &s[1..end];
    let (_, emoji) = EMOJI.iter().find(|(known, _)| *known == name)?;
    Some((Inline::Emoji(emoji), end + 1))
}

/// The shortcodes release notes, changelogs and commit messages use most
/// (gitmoji's among them), and their emoji, as GitHub draws them. Not all
/// of GitHub's 1800: an unknown one stays as typed.
const EMOJI: [(&str, &str); 72] = [
    ("+1", "👍"),
    ("-1", "👎"),
    ("adhesive_bandage", "🩹"),
    ("alembic", "⚗️"),
    ("ambulance", "🚑"),
    ("arrow_down", "⬇️"),
    ("arrow_up", "⬆️"),
    ("art", "🎨"),
    ("beetle", "🐞"),
    ("book", "📖"),
    ("bookmark", "🔖"),
    ("books", "📚"),
    ("boom", "💥"),
    ("broom", "🧹"),
    ("bug", "🐛"),
    ("building_construction", "🏗️"),
    ("bulb", "💡"),
    ("card_file_box", "🗃️"),
    ("chart_with_upwards_trend", "📈"),
    ("checkered_flag", "🏁"),
    ("clap", "👏"),
    ("closed_lock_with_key", "🔐"),
    ("construction", "🚧"),
    ("construction_worker", "👷"),
    ("dizzy", "💫"),
    ("eyes", "👀"),
    ("fire", "🔥"),
    ("gear", "⚙️"),
    ("globe_with_meridians", "🌐"),
    ("green_heart", "💚"),
    ("hammer", "🔨"),
    ("hammer_and_wrench", "🛠️"),
    ("heart", "❤️"),
    ("heavy_check_mark", "✔️"),
    ("heavy_minus_sign", "➖"),
    ("heavy_plus_sign", "➕"),
    ("hourglass", "⌛"),
    ("information_source", "ℹ️"),
    ("label", "🏷️"),
    ("link", "🔗"),
    ("lipstick", "💄"),
    ("lock", "🔒"),
    ("mag", "🔍"),
    ("memo", "📝"),
    ("new", "🆕"),
    ("no_entry_sign", "🚫"),
    ("package", "📦"),
    ("pencil", "📝"),
    ("pencil2", "✏️"),
    ("pushpin", "📌"),
    ("recycle", "♻️"),
    ("rewind", "⏪"),
    ("robot", "🤖"),
    ("rocket", "🚀"),
    ("rotating_light", "🚨"),
    ("seedling", "🌱"),
    ("shield", "🛡️"),
    ("sparkles", "✨"),
    ("speech_balloon", "💬"),
    ("star", "⭐"),
    ("tada", "🎉"),
    ("test_tube", "🧪"),
    ("thumbsdown", "👎"),
    ("thumbsup", "👍"),
    ("triangular_flag_on_post", "🚩"),
    ("truck", "🚚"),
    ("twisted_rightwards_arrows", "🔀"),
    ("warning", "⚠️"),
    ("wastebasket", "🗑️"),
    ("white_check_mark", "✅"),
    ("wrench", "🔧"),
    ("x", "❌"),
];

/// How many of its first byte `s` starts with: a run of backticks, of
/// stars, of underscores.
fn run(s: &str) -> usize {
    let Some(&first) = s.as_bytes().first() else {
        return 0;
    };
    s.bytes().take_while(|&b| b == first).count()
}

/// How many backticks `s` starts with.
fn backtick_run(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b'`').count()
}

/// `**strong**` or `*emphasis*` (`__` and `_` alike), or `~~struck~~`, at
/// the start of `s`, and its length, as GitHub reads them: the text inside
/// neither starts nor ends with a space, and closes with a run just as
/// long — after which an `_` needs no letter, so `snake_case` stays a
/// name. A run of three or more is left as typed, and so is a lone `~`.
fn emphasis(s: &str) -> Option<(Inline<'_>, usize)> {
    let mark = s.as_bytes()[0];
    let open = run(s);
    if open > 2 || (mark == b'~' && open != 2) || s[open..].starts_with(char::is_whitespace) {
        return None;
    }
    let mut at = open;
    while let Some(found) = s[at..].find(char::from(mark)) {
        let start = at + found;
        let close = run(&s[start..]);
        let end = start + close;
        let spaced = s[..start].ends_with(char::is_whitespace);
        let in_word = mark == b'_' && s[end..].starts_with(char::is_alphanumeric);
        if close == open && start > open && !spaced && !in_word {
            let inner = &s[open..start];
            let piece = match (mark, open) {
                (b'~', _) => Inline::Strike(inner),
                (_, 2) => Inline::Strong(inner),
                _ => Inline::Emphasis(inner),
            };
            return Some((piece, end));
        }
        at = end;
    }
    None
}

/// The length of the code span at the start of `s`: its run of backticks,
/// up to the next run just as long. `None` when it is never closed.
fn code_span(s: &str) -> Option<usize> {
    let ticks = backtick_run(s);
    let mut at = ticks;
    while let Some(found) = s[at..].find('`') {
        let start = at + found;
        let run = backtick_run(&s[start..]);
        if run == ticks {
            return Some(start + run);
        }
        at = start + run;
    }
    None
}

/// `![alt](url)` at the start of `s`: its alt text and its length.
fn markdown_image(s: &str) -> Option<(&str, usize)> {
    if !s.starts_with("![") {
        return None;
    }
    let close = s.find("](")?;
    let end = close + 2 + s[close + 2..].find(')')?;
    Some((&s[2..close], end + 1))
}

/// `[label](url)` at the start of `s`, and its length. The brackets may
/// nest — a badge is an image inside a link — and a title may follow the
/// URL: the URL ends at the first space.
fn markdown_link(s: &str) -> Option<(Inline<'_>, usize)> {
    let mut depth = 0;
    let close = s.char_indices().find_map(|(i, c)| {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ => return None,
        }
        (depth == 0).then_some(i)
    })?;
    let target = s[close + 1..].strip_prefix('(')?;
    let end = target.find(')')?;
    let url = target[..end].split_whitespace().next().unwrap_or_default();
    let label = &s[1..close];
    Some((Inline::Link { label, url }, close + 2 + end + 1))
}

/// `[image: alt]`, or `[image]` without one.
fn image_label(alt: &str) -> String {
    match alt.trim() {
        "" => "[image]".to_string(),
        alt => format!("[image: {alt}]"),
    }
}

/// The HTML tags the reader knows: those it gives a look, and those whose
/// text is enough (`<strong>`, `<kbd>`, table cells…). Lowercase only, as
/// GitHub and Dependabot write them, so `Option<T>` or `Foo<B>` stay text;
/// any `<word>` that is not here is no HTML and stays as typed too.
const HTML_TAGS: [&str; 53] = [
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "center",
    "cite",
    "code",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "g-emoji",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "input",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "picture",
    "pre",
    "s",
    "samp",
    "small",
    "source",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "ul",
];

/// An HTML tag as typed: `<a href="…">`, `</a>`, `<br/>`.
struct Tag<'a> {
    name: &'static str,
    closing: bool,
    /// What follows the name, up to the `>`: the attributes.
    attrs: &'a str,
}

impl<'a> Tag<'a> {
    /// The tag at the start of `s`, and its length. `None` when that is no
    /// HTML tag: not one of `HTML_TAGS`, or its `>` is not on the line.
    fn read(s: &'a str) -> Option<(Tag<'a>, usize)> {
        let rest = s.strip_prefix('<')?;
        let (closing, rest) = match rest.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let name_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(rest.len());
        let name = *HTML_TAGS.iter().find(|&&tag| tag == &rest[..name_len])?;
        let rest = &rest[name_len..];
        // `<a.b>` is no `<a>`.
        if !rest.starts_with(|c: char| c.is_whitespace() || c == '/' || c == '>') {
            return None;
        }
        // Up to the `>`, which may sit in a quoted value (`alt="a > b"`).
        let mut quote = None;
        for (i, c) in rest.char_indices() {
            match quote {
                Some(open) if c == open => quote = None,
                Some(_) => {}
                None if c == '"' || c == '\'' => quote = Some(c),
                None if c == '>' => {
                    let tag = Tag {
                        name,
                        closing,
                        attrs: &rest[..i],
                    };
                    return Some((tag, s.len() - rest.len() + i + 1));
                }
                None => {}
            }
        }
        None
    }

    /// The value of the attribute `wanted` (`href="…"`, `'…'` or bare), as
    /// typed: its entities are not decoded yet.
    fn attr(&self, wanted: &str) -> Option<&'a str> {
        let mut rest = self.attrs;
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
            if rest.is_empty() {
                return None;
            }
            let name_end = rest
                .find(|c: char| c.is_whitespace() || c == '=' || c == '/')
                .unwrap_or(rest.len());
            let name = &rest[..name_end];
            rest = rest[name_end..].trim_start();
            let mut value = "";
            if let Some(after) = rest.strip_prefix('=') {
                let after = after.trim_start();
                let (found, len) = match after.chars().next() {
                    Some(quote @ ('"' | '\'')) => match after[1..].find(quote) {
                        Some(end) => (&after[1..1 + end], end + 2),
                        None => (&after[1..], after.len()),
                    },
                    _ => {
                        let end = after.find(char::is_whitespace).unwrap_or(after.len());
                        (&after[..end], end)
                    }
                };
                value = found;
                rest = &after[len..];
            }
            if name == wanted {
                return Some(value);
            }
        }
    }
}

// --- the writer ---

/// A table's cell, read like any prose, as one line of spans; its links'
/// text and URL added to `links`.
fn cell_spans(markdown: &str, links: &mut Vec<(String, String)>) -> Vec<Span<'static>> {
    let mut cell = Writer::new(false, u16::MAX);
    cell.in_cell = true;
    cell.inline(markdown);
    cell.end_line();
    links.append(&mut cell.cell_links);
    // A `<br>` in a cell broke it into lines: they read as one.
    let mut spans = Vec::new();
    for (i, line) in cell.lines.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        spans.extend(line.spans);
    }
    spans
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// A row's cells side by side, two columns apart, each padded to its
/// column's width on the side its alignment leaves; the last cell of a
/// left-aligned column gets no trailing spaces.
fn table_row(
    cells: Vec<Vec<Span<'static>>>,
    aligns: &[Align],
    widths: &[usize],
) -> Vec<Span<'static>> {
    let last = cells.len() - 1;
    let mut spans = Vec::new();
    for (col, cell) in cells.into_iter().enumerate() {
        if col > 0 {
            spans.push(Span::raw("  "));
        }
        let pad = widths[col] - spans_width(&cell);
        let (before, after) = match aligns[col] {
            Align::Left => (0, pad),
            Align::Right => (pad, 0),
            Align::Center => (pad / 2, pad - pad / 2),
        };
        if before > 0 {
            spans.push(Span::raw(" ".repeat(before)));
        }
        spans.extend(cell);
        if after > 0 && col < last {
            spans.push(Span::raw(" ".repeat(after)));
        }
    }
    spans
}

/// A block the lines being written sit in. Each indents them by two
/// columns, under what opened it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    /// An unfolded `<details>`, under its `▾` line.
    Details,
    /// `<ul>` or `<ol>`: a nested list indents further.
    List,
    /// `<blockquote>`, behind a gray `│ ` bar.
    Quote,
}

/// A link being written: where it goes, and its text so far.
struct Link {
    url: String,
    text: String,
}

/// Builds the lines piece by piece. The source lines' markdown and the
/// HTML tags in them both write here, so a `<details>` or a `<ul>` can
/// span many source lines.
struct Writer {
    show_folded: bool,
    /// The room the lines get: a wider table reads row by row.
    width: u16,
    lines: Vec<Line<'static>>,
    /// The line being written, without the indentation of its blocks:
    /// that is added when it ends.
    spans: Vec<Span<'static>>,
    /// Its own style, if it has one: a heading's, a quote's.
    style: Option<Style>,
    /// It holds only its `• ` so far: a `<p>` or a line break right after
    /// an `<li>` must not leave the bullet alone on its line.
    only_bullet: bool,
    /// The next text starts a line an HTML tag opened: its leading spaces
    /// are the source's indentation, not text.
    trim: bool,
    /// The last line written is a blank one.
    blank: bool,
    /// The open blocks, outermost first.
    blocks: Vec<Block>,
    /// A `<details>` was opened and its header is not written yet: it
    /// waits for its `<summary>`.
    pending: bool,
    /// The text of the `<summary>` being read.
    summary: Option<String>,
    /// The open links, innermost last.
    links: Vec<Link>,
    /// How many `<details>` deep the reader is inside a folded one; 0 when
    /// what comes shows.
    hidden: usize,
    /// How many `**`, `<strong>`, `<b>` the text is in: bold when any.
    strong: usize,
    /// How many `*`, `<em>`, `<i>` the text is in: italic when any.
    emphasis: usize,
    /// How many `~~`, `<del>`, `<s>` the text is in: struck when any.
    strike: usize,
    /// How many `` ` `` or `<code>` the text is in: yellow when any.
    code: usize,
    /// This writer writes a table's cell: a link is its text, underlined,
    /// its URL kept in `cell_links` for the list under the table.
    in_cell: bool,
    /// The links of a cell: their text, their URL.
    cell_links: Vec<(String, String)>,
}

impl Writer {
    fn new(show_folded: bool, width: u16) -> Self {
        Self {
            show_folded,
            width,
            lines: Vec::new(),
            spans: Vec::new(),
            style: None,
            only_bullet: false,
            trim: false,
            blank: false,
            blocks: Vec::new(),
            pending: false,
            summary: None,
            links: Vec::new(),
            hidden: 0,
            strong: 0,
            emphasis: 0,
            strike: 0,
            code: 0,
            in_cell: false,
            cell_links: Vec::new(),
        }
    }

    /// The lines written; a trailing blank one dropped.
    fn finish(mut self) -> Vec<Line<'static>> {
        self.end_line();
        // A body that ends in a `<details>` header still shows it.
        self.reveal();
        if self.blank {
            self.lines.pop();
        }
        self.lines
    }

    // The source lines' own markdown.

    /// A line of fenced code: as typed, in yellow.
    fn code_line(&mut self, line: &str) {
        if !self.reveal() {
            return;
        }
        self.end_line();
        self.push_line(
            vec![Span::raw(format!("    {line}"))],
            Style::new().fg(Color::Yellow),
        );
    }

    /// A `#` heading.
    fn heading(&mut self, title: &str) {
        self.reveal();
        self.start_heading();
        self.inline(title);
        self.end_heading();
    }

    /// Any line outside a fence that is not blank nor a heading: a rule, a
    /// list item, a quote or plain text, any of them holding HTML.
    fn prose(&mut self, line: &str) {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if is_rule(trimmed) {
            self.rule();
            return;
        }
        if let Some(item) = ["- ", "* ", "+ "]
            .iter()
            .find_map(|bullet| trimmed.strip_prefix(bullet))
        {
            self.reveal();
            // Two columns per nesting level, as the source indents them.
            let pad = "  ".repeat(indent / 2 + 1);
            // A task's box in place of the bullet, as GitHub draws it.
            let (bullet, item) = if let Some(task) = item.strip_prefix("[ ] ") {
                ("☐ ", task)
            } else if let Some(task) = item.strip_prefix("[x] ").or(item.strip_prefix("[X] ")) {
                ("☑ ", task)
            } else {
                ("• ", item)
            };
            self.write(&format!("{pad}{bullet}"));
            self.inline(item);
        } else if let Some(quoted) = trimmed.strip_prefix('>') {
            self.reveal();
            self.style = Some(Style::new().fg(Color::DarkGray));
            self.write("│ ");
            self.inline(quoted.trim_start());
        } else {
            self.inline(line);
        }
        // The source's line break, kept.
        self.soft_break();
    }

    /// A gray rule.
    fn rule(&mut self) {
        if !self.reveal() {
            return;
        }
        self.end_line();
        self.push_line(
            vec![Span::raw("─".repeat(40))],
            Style::new().fg(Color::DarkGray),
        );
    }

    /// A table: its columns lined up under a bold header and a rule when
    /// they fit in the width, each row as `header: cell` pairs when they do
    /// not. Its links show their text only; their URLs follow the table,
    /// one a line, whole for the terminal to open.
    fn table(&mut self, table: &Table) {
        if !self.reveal() {
            return;
        }
        self.end_line();
        let mut links = Vec::new();
        let mut cell = |markdown: &str| cell_spans(markdown, &mut links);
        let header: Vec<_> = table.header.iter().map(|c| cell(c)).collect();
        // As many cells as the header: one missing is empty, one too many
        // is dropped, as on GitHub.
        let rows: Vec<Vec<_>> = table
            .rows
            .iter()
            .map(|row| {
                (0..header.len())
                    .map(|col| row.get(col).map(|c| cell(c)).unwrap_or_default())
                    .collect()
            })
            .collect();

        let widths: Vec<usize> = (0..header.len())
            .map(|col| {
                std::iter::once(&header)
                    .chain(&rows)
                    .map(|row| spans_width(&row[col]))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let total = widths.iter().sum::<usize>() + 2 * (widths.len() - 1);
        let room = usize::from(self.width).saturating_sub(self.indent().chars().count());
        let gray = Style::new().fg(Color::DarkGray);
        if total <= room {
            let header = table_row(header, &table.aligns, &widths);
            self.push_line(header, Style::new().bold());
            self.push_line(vec![Span::styled("─".repeat(total), gray)], Style::new());
            for row in rows {
                let row = table_row(row, &table.aligns, &widths);
                self.push_line(row, Style::new());
            }
        } else {
            for row in rows {
                let mut spans = Vec::new();
                for (col, cell) in row.into_iter().enumerate() {
                    if col > 0 {
                        spans.push(Span::styled(" · ", gray));
                    }
                    spans.extend(
                        header[col]
                            .iter()
                            .map(|span| span.clone().patch_style(gray)),
                    );
                    spans.push(Span::styled(": ", gray));
                    spans.extend(cell);
                }
                self.push_line(spans, Style::new());
            }
        }

        // Each link once, its text lined up: `eslint  https://…`.
        let mut unique: Vec<(String, String)> = Vec::new();
        for link in links {
            if !unique.contains(&link) {
                unique.push(link);
            }
        }
        let links = unique;
        // Apart from the rows, not to read as more of them.
        if !links.is_empty() {
            self.blank();
        }
        let text_width = links
            .iter()
            .map(|(text, _)| Span::raw(text.as_str()).width())
            .max()
            .unwrap_or(0);
        for (text, url) in links {
            let pad = text_width - Span::raw(text.as_str()).width();
            let spans = vec![
                Span::styled(format!("{text}{}  ", " ".repeat(pad)), gray),
                Span::styled(url, gray.underlined()),
            ];
            self.push_line(spans, Style::new());
        }
    }

    /// Air around a heading, as GitHub gives it.
    fn start_heading(&mut self) {
        self.blank();
        self.style = Some(Style::new().bold().fg(Color::Cyan));
        self.write("▍");
        self.trim = true;
    }

    fn end_heading(&mut self) {
        self.end_line();
        self.blank();
    }

    /// Writes a line of prose (or a heading's title, a link's text): its
    /// plain text, and what starts at a `` ` ``, `!`, `[`, `<`, `*`, `_`,
    /// `~` or `:`.
    fn inline(&mut self, line: &str) {
        // Where the plain text not written yet starts, and where to look
        // for the next piece.
        let mut plain = 0;
        let mut at = 0;
        while let Some(found) = line[at..].find(['`', '!', '[', '<', '*', '_', '~', ':']) {
            let start = at + found;
            let after_word = line[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
            let Some((piece, len)) = inline_at(&line[start..], after_word) else {
                // Plain text after all. An unclosed run of backticks (or
                // stars) stays one, not to be read again one shorter.
                at = start
                    + match line.as_bytes()[start] {
                        b'`' | b'*' | b'_' | b'~' => run(&line[start..]),
                        _ => 1,
                    };
                continue;
            };
            self.text(&line[plain..start]);
            match piece {
                Inline::Code(code) => {
                    self.code += 1;
                    self.write(code);
                    self.code -= 1;
                }
                Inline::Strike(inner) => {
                    self.strike += 1;
                    self.inline(inner);
                    self.strike -= 1;
                }
                Inline::Autolink(url) => {
                    self.open_link(url.to_string());
                    self.write(url);
                    self.close_link();
                }
                Inline::Emoji(emoji) => self.write(emoji),
                Inline::Image(alt) => self.write(&image_label(&decode(alt))),
                Inline::Link { label, url } => {
                    self.open_link(decode(url));
                    self.inline(label);
                    self.close_link();
                }
                Inline::Tag(tag) => self.tag(&tag),
                Inline::Strong(inner) => {
                    self.strong += 1;
                    self.inline(inner);
                    self.strong -= 1;
                }
                Inline::Emphasis(inner) => {
                    self.emphasis += 1;
                    self.inline(inner);
                    self.emphasis -= 1;
                }
            }
            at = start + len;
            plain = at;
        }
        self.text(&line[plain..]);
    }

    // HTML.

    /// What a tag does: most end the line or open a block, the inline ones
    /// write a mark or nothing at all.
    fn tag(&mut self, tag: &Tag) {
        // Inside a folded `<details>`, only the depth of the nested ones
        // matters: it tells which `</details>` ends the folded one.
        if self.hidden > 0 {
            if tag.name == "details" {
                if tag.closing {
                    self.hidden -= 1;
                } else {
                    self.hidden += 1;
                }
            }
            return;
        }
        // Inside `<summary>`, only the text counts: the header is one line.
        if self.summary.is_some() {
            match (tag.name, tag.closing) {
                ("code", _) => self.write("`"),
                ("img", false) => self.image(tag),
                ("summary", true) => {
                    self.reveal();
                }
                // Never closed: the summary ends with its `<details>`.
                ("details", _) => {
                    self.reveal();
                    self.tag(tag);
                }
                _ => {}
            }
            return;
        }
        if tag.name == "summary" {
            // Only a `<details>`' first child names it; any other is dropped.
            if self.pending && !tag.closing {
                self.pending = false;
                self.summary = Some(String::new());
            }
            return;
        }
        // A `<details>` without `<summary>` gets its header now, and when
        // it is folded the tag counts as folded content.
        if !self.reveal() {
            self.tag(tag);
            return;
        }
        match (tag.name, tag.closing) {
            ("details", false) => {
                self.end_line();
                self.pending = true;
            }
            ("details", true) => {
                self.end_line();
                self.close(Block::Details);
                self.blank();
            }
            ("ul" | "ol", false) => self.open(Block::List),
            ("ul" | "ol", true) => {
                self.end_line();
                self.close(Block::List);
            }
            ("blockquote", false) => self.open(Block::Quote),
            ("blockquote", true) => {
                self.end_line();
                self.close(Block::Quote);
            }
            ("li", false) => {
                self.end_line();
                self.write("• ");
                self.only_bullet = true;
                self.trim = true;
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => self.start_heading(),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => self.end_heading(),
            ("hr", _) => self.rule(),
            ("li" | "p" | "br" | "div" | "pre" | "table" | "tr" | "dl" | "dt" | "dd", _) => {
                self.soft_break();
                self.trim = true;
            }
            ("code", false) => self.code += 1,
            ("code", true) => self.code = self.code.saturating_sub(1),
            ("del" | "s" | "strike", false) => self.strike += 1,
            ("del" | "s" | "strike", true) => self.strike = self.strike.saturating_sub(1),
            ("a", false) => self.open_link(tag.attr("href").map(decode).unwrap_or_default()),
            ("a", true) => self.close_link(),
            ("img", false) => self.image(tag),
            ("strong" | "b", false) => self.strong += 1,
            ("strong" | "b", true) => self.strong = self.strong.saturating_sub(1),
            ("em" | "i", false) => self.emphasis += 1,
            ("em" | "i", true) => self.emphasis = self.emphasis.saturating_sub(1),
            // Cells side by side, not glued.
            ("td" | "th", true) => self.write(" "),
            // `<kbd>`, `<span>`…: their text is enough.
            _ => {}
        }
    }

    /// `<img>`: its alt text, as a markdown image.
    fn image(&mut self, tag: &Tag) {
        let alt = decode(tag.attr("alt").unwrap_or_default());
        self.write(&image_label(&alt));
    }

    fn open(&mut self, block: Block) {
        self.end_line();
        self.blocks.push(block);
    }

    /// Closes the last `block` open, and whatever was opened inside it and
    /// left open: an HTML parser's tolerance.
    fn close(&mut self, block: Block) {
        if let Some(at) = self.blocks.iter().rposition(|&open| open == block) {
            self.blocks.truncate(at);
        }
    }

    fn open_link(&mut self, url: String) {
        self.links.push(Link {
            url,
            text: String::new(),
        });
    }

    /// Ends a link: its URL after its text, ` (https://…)`, whole and in
    /// one piece so the terminal can open it — once when the text is
    /// already the URL. A summary shows the text only.
    fn close_link(&mut self) {
        let Some(link) = self.links.pop() else {
            return;
        };
        if self.hidden > 0 || self.summary.is_some() {
            return;
        }
        let text = link.text.trim();
        if link.url.is_empty() || text == link.url {
            return;
        }
        if self.in_cell {
            self.cell_links.push((text.to_string(), link.url));
            return;
        }
        let gray = Style::new().fg(Color::DarkGray);
        let url = Span::styled(link.url, gray.underlined());
        if text.is_empty() {
            self.spans.push(url);
        } else {
            self.spans.push(Span::styled(" (", gray));
            self.spans.push(url);
            self.spans.push(Span::styled(")", gray));
        }
    }

    /// Opens a `<details>` whose header is still to write — the summary's
    /// text, or "Details" without one, as GitHub — `▸` folded, its content
    /// then skipped, or `▾` open, its content indented under it.
    fn open_details(&mut self, title: Option<String>) {
        self.pending = false;
        self.summary = None;
        let title = title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("Details")
            .to_string();
        self.end_line();
        let gray = Style::new().fg(Color::DarkGray);
        if self.show_folded {
            self.push_line(
                vec![
                    Span::styled("▾ ", gray),
                    Span::styled(title, Style::new().bold()),
                ],
                Style::new(),
            );
            self.blocks.push(Block::Details);
        } else {
            self.push_line(vec![Span::raw(format!("▸ {title}"))], gray);
            self.hidden = 1;
        }
    }

    /// Called before anything that shows: a `<details>` still waiting for
    /// its `<summary>` (or inside it) gets its header first. False when
    /// what comes is folded away.
    fn reveal(&mut self) -> bool {
        if let Some(title) = self.summary.take() {
            self.open_details(Some(title));
        } else if self.pending {
            self.open_details(None);
        }
        self.hidden == 0
    }

    // Writing.

    /// Plain text as typed in the source: its entities decoded.
    fn text(&mut self, raw: &str) {
        if !raw.is_empty() {
            self.write(&decode(raw));
        }
    }

    /// Writes `text`, final, on the line being written — or into the
    /// summary being read, or nowhere when it is folded away.
    fn write(&mut self, text: &str) {
        if self.hidden > 0 || text.is_empty() {
            return;
        }
        if let Some(summary) = &mut self.summary {
            summary.push_str(text);
            return;
        }
        // Spaces between `<details>` and its `<summary>` are no content.
        if self.pending && (text.trim().is_empty() || !self.reveal()) {
            return;
        }
        let text = if self.trim { text.trim_start() } else { text };
        if text.is_empty() {
            return;
        }
        self.trim = false;
        self.only_bullet = false;
        if let Some(link) = self.links.last_mut() {
            link.text.push_str(text);
        }
        self.spans
            .push(Span::styled(text.to_string(), self.text_style()));
    }

    /// The style of the text written now: bold, italic, struck, code, or
    /// underlined for a link in a table's cell, its URL being under the
    /// table.
    fn text_style(&self) -> Style {
        let mut style = Style::new();
        if self.strong > 0 {
            style = style.bold();
        }
        if self.emphasis > 0 {
            style = style.italic();
        }
        if self.strike > 0 {
            style = style.crossed_out();
        }
        // Yellow, as fenced code: GitHub gives code a background instead
        // of its backticks.
        if self.code > 0 {
            style = style.fg(Color::Yellow);
        }
        if self.in_cell && !self.links.is_empty() {
            style = style.underlined();
        }
        style
    }

    /// A line break that leaves an `<li>`'s lone bullet where it is: the
    /// end of a source line, `<p>`, `<br>`.
    fn soft_break(&mut self) {
        if !self.only_bullet {
            self.end_line();
        }
    }

    /// Ends the line being written; a line of spaces only is dropped (the
    /// indentation of HTML tags in the source).
    fn end_line(&mut self) {
        let spans = std::mem::take(&mut self.spans);
        let style = self.style.take();
        self.trim = false;
        self.only_bullet = false;
        // A summary on several source lines reads as one.
        if let Some(summary) = &mut self.summary
            && !summary.ends_with(' ')
        {
            summary.push(' ');
        }
        if spans.iter().all(|span| span.content.trim().is_empty()) {
            return;
        }
        let style = style.unwrap_or(if self.quoted() {
            Style::new().fg(Color::DarkGray)
        } else {
            Style::new()
        });
        self.push_line(spans, style);
    }

    /// A blank line, unless the text starts here or one is already there:
    /// runs of blank lines collapse to one. In a quote, its bar runs on.
    fn blank(&mut self) {
        self.end_line();
        if self.hidden > 0
            || self.pending
            || self.summary.is_some()
            || self.lines.is_empty()
            || self.blank
        {
            return;
        }
        let line = if self.quoted() {
            Line::styled(
                self.indent().trim_end().to_string(),
                Style::new().fg(Color::DarkGray),
            )
        } else {
            Line::default()
        };
        self.lines.push(line);
        self.blank = true;
    }

    /// Adds a finished line, behind the indentation of its blocks.
    fn push_line(&mut self, spans: Vec<Span<'static>>, style: Style) {
        let indent = self.indent();
        let mut line = Vec::with_capacity(spans.len() + 1);
        if !indent.is_empty() {
            // A quote's bar stays gray, whatever the line's own style.
            line.push(Span::styled(indent, Style::new().fg(Color::DarkGray)));
        }
        line.extend(spans);
        self.lines.push(Line::from(line).style(style));
        self.blank = false;
    }

    /// Two columns per open block, a quote's with its bar.
    fn indent(&self) -> String {
        self.blocks
            .iter()
            .map(|block| match block {
                Block::Quote => "│ ",
                Block::Details | Block::List => "  ",
            })
            .collect()
    }

    fn quoted(&self) -> bool {
        self.blocks.contains(&Block::Quote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    /// Room enough for every line the tests write.
    const WIDTH: u16 = 120;

    /// `body` as the view shows it with its `<details>` folded.
    fn folded(body: &str) -> Vec<String> {
        texts(&markdown_lines(body, WIDTH, false))
    }

    /// `body` once `space` unfolded its `<details>`.
    fn unfolded(body: &str) -> Vec<String> {
        texts(&markdown_lines(body, WIDTH, true))
    }

    /// One line of prose, as the view shows it.
    fn one_line(line: &str) -> String {
        let lines = folded(line);
        assert_eq!(lines.len(), 1, "{lines:?}");
        lines[0].clone()
    }

    #[test]
    fn sanitize_normalizes_line_ends_expands_tabs_and_drops_bidi_overrides() {
        // ratatui drops \t and \r itself (control characters), silently: a
        // tab-indented code block would lose its indentation.
        assert_eq!(sanitize("a\r\n\tb\rc"), "a\n    b\nc");
        // U+202E reorders what a line shows ("trojan source"): gone.
        assert_eq!(sanitize("x\u{202e}y\u{2066}z"), "xyz");
        // Other control characters (an ESC sequence) go too.
        assert_eq!(sanitize("\u{1b}[31mred"), "[31mred");
    }

    /// Characters that take no room, which a terminal may draw as a box:
    /// gone, except the joiner that glues an emoji family together.
    #[test]
    fn invisible_characters_are_dropped_but_the_emoji_joiner() {
        assert_eq!(
            sanitize("|\u{feff}ts\u{200b}-proto\u{2060}\u{ad}\u{200e}"),
            "|ts-proto"
        );
        assert_eq!(sanitize("👩\u{200d}💻"), "👩\u{200d}💻");
        assert_eq!(one_line("a&#8203;b&#xFEFF;c"), "abc");
    }

    #[test]
    fn html_comments_vanish_even_across_lines() {
        assert_eq!(strip_html_comments("a<!-- x -->b"), "ab");
        assert_eq!(strip_html_comments("a\n<!--\nfill\nthis\n-->\nb"), "a\n\nb");
        assert_eq!(strip_html_comments("a<!-- 1 -->b<!-- 2 -->c"), "abc");
        // Unterminated: GitHub hides the rest, so do we.
        assert_eq!(strip_html_comments("a<!-- never closed\nb"), "a");
    }

    #[test]
    fn a_comment_inside_a_fence_is_code_not_a_comment() {
        assert_eq!(
            strip_html_comments("```\n<!-- x -->\n```"),
            "```\n<!-- x -->\n```"
        );
    }

    #[test]
    fn images_become_their_alt_text() {
        assert_eq!(
            one_line("see ![the dashboard](https://x/y.png)."),
            "see [image: the dashboard]."
        );
        assert_eq!(one_line("![](https://x/y.png)"), "[image]");
        assert_eq!(
            one_line(r#"<img src="https://x" alt="logo" width="40">"#),
            "[image: logo]"
        );
        assert_eq!(one_line(r#"<img src="https://x">"#), "[image]");
        // Not an image after all: left as typed.
        assert_eq!(one_line("a ![b] c"), "a ![b] c");
    }

    /// A PR template is mostly comments: what is left is the PR.
    #[test]
    fn a_template_body_reads_as_its_headings_and_text() {
        let body = "<!--\r\nThanks! Fill every section.\r\n-->\r\n\r\n## Summary\r\n\r\nAdds a limiter.\r\n\r\n\r\n\r\n## How to test\r\n<!-- paste commands -->\r\n```sh\r\ncargo test\r\n```\r\n";
        assert_eq!(
            folded(body),
            // The comment-only line leaves a blank, kept (runs collapse to one).
            [
                "▍Summary",
                "",
                "Adds a limiter.",
                "",
                "▍How to test",
                "",
                "    cargo test"
            ]
        );
    }

    #[test]
    fn lists_quotes_and_rules_get_a_light_touch() {
        let md = "- one\n  * nested\n- [x] done\n> quoted\n---";
        assert_eq!(
            folded(md),
            [
                "  • one",
                "    • nested",
                "  ☑ done",
                "│ quoted",
                &"─".repeat(40)
            ]
        );
    }

    #[test]
    fn nothing_inside_a_fence_is_interpreted() {
        let md = "```\n# not a heading\n<!-- kept -->\n\tindented\n<details>\n&amp; <b>x</b>\n```";
        assert_eq!(
            folded(md),
            [
                "    # not a heading",
                "    <!-- kept -->",
                "        indented",
                "    <details>",
                "    &amp; <b>x</b>"
            ]
        );
    }

    // --- <details> ---

    /// Folded, a `<details>` block is its summary on one gray `▸` line, as
    /// a folded review; consecutive ones stay together.
    #[test]
    fn a_details_block_folds_to_its_summary() {
        let body = "Bumps x.\n<details>\n<summary>Release notes</summary>\n<p>hidden</p>\n</details>\n<details>\n<summary>Commits</summary>\n<ul><li>c</li></ul>\n</details>\nAfter.";
        assert_eq!(
            folded(body),
            ["Bumps x.", "▸ Release notes", "▸ Commits", "After."]
        );
        let lines = markdown_lines(body, WIDTH, false);
        assert_eq!(lines[1].style.fg, Some(Color::DarkGray), "gray, as folded");
    }

    /// `space` unfolds every block, its content indented under its `▾`.
    #[test]
    fn space_unfolds_a_details_block_under_its_summary() {
        let body = "Bumps x.\n<details>\n<summary>Release notes</summary>\n<p>shown</p>\n</details>\n<details>\n<summary>Commits</summary>\n<ul><li>c</li></ul>\n</details>\nAfter.";
        assert_eq!(
            unfolded(body),
            [
                "Bumps x.",
                "▾ Release notes",
                "  shown",
                "",
                "▾ Commits",
                "    • c",
                "",
                "After."
            ]
        );
    }

    /// A block inside a folded block stays hidden with it: its own
    /// `</details>` does not end the outer one.
    #[test]
    fn nested_details_fold_and_unfold_together() {
        let body = "<details><summary>Outer</summary>\n<details><summary>Inner</summary>\nx\n</details>\nz\n</details>\ny";
        assert_eq!(folded(body), ["▸ Outer", "y"]);
        assert_eq!(
            unfolded(body),
            ["▾ Outer", "  ▾ Inner", "    x", "", "  z", "", "y"]
        );
    }

    /// GitHub names a block without a `<summary>` "Details".
    #[test]
    fn a_details_block_without_summary_is_called_details() {
        assert_eq!(folded("<details>\nx\n</details>"), ["▸ Details"]);
        assert_eq!(unfolded("<details>\nx\n</details>"), ["▾ Details", "  x"]);
    }

    /// Markdown inside an open block keeps its markdown look, indented.
    #[test]
    fn markdown_inside_an_unfolded_block_is_still_markdown() {
        let body = "<details>\n<summary>Commands</summary>\n\n- `@dependabot rebase` will rebase\n</details>";
        assert_eq!(
            unfolded(body),
            ["▾ Commands", "", "    • @dependabot rebase will rebase"]
        );
    }

    // --- block and inline tags ---

    #[test]
    fn html_headings_lists_quotes_and_paragraphs_look_like_their_markdown() {
        let body = "<h2>v18.4.0</h2>\n<h3>Features</h3>\n<ul>\n<li><strong>x:</strong> a</li>\n<li>b<br>c<br/>d</li>\n</ul>\n<blockquote><p>q</p></blockquote>\n<p>one</p><p>two</p>";
        assert_eq!(
            folded(body),
            [
                "▍v18.4.0",
                "",
                "▍Features",
                "",
                "  • x: a",
                "  • b",
                "  c",
                "  d",
                "│ q",
                "one",
                "two"
            ]
        );
    }

    #[test]
    fn nested_html_lists_indent() {
        assert_eq!(
            folded("<ul><li>a<ul><li>b</li></ul></li><li>c</li></ul>"),
            ["  • a", "    • b", "  • c"]
        );
        // A loose item, its text in a paragraph on the next source line:
        // the bullet stays with it.
        assert_eq!(
            folded("<ul>\n  <li>\n    <p>loose</p>\n  </li>\n</ul>"),
            ["  • loose"]
        );
    }

    /// A `>` inside a quoted attribute does not end the tag; single
    /// quotes and bare values are attributes too.
    #[test]
    fn a_tag_reads_its_attributes_whatever_their_quotes() {
        assert_eq!(one_line("<img src=x.png alt='a > b'>"), "[image: a > b]");
        assert_eq!(one_line("<a href=https://x.y/z>z</a>"), "z (https://x.y/z)");
        // No `>` on the line: no tag.
        assert_eq!(
            one_line("<a href=\"x\" never closed"),
            "<a href=\"x\" never closed"
        );
    }

    /// A summary typed over several lines, with markup, is one header.
    #[test]
    fn a_summary_reads_as_one_line_of_text() {
        assert_eq!(
            folded(
                "<details>\n<summary>\n<b>Bump</b> <code>vite</code>\nto 5.4.11\n</summary>\nx\n</details>"
            ),
            ["▸ Bump `vite` to 5.4.11"]
        );
    }

    /// Inside a quote the bar runs on, blank lines included.
    #[test]
    fn a_quote_keeps_its_bar_across_a_heading() {
        assert_eq!(
            folded("<blockquote>\n<h2>v1</h2>\n<ul><li>x</li></ul>\n</blockquote>"),
            ["│ ▍v1", "│", "│   • x"]
        );
    }

    #[test]
    fn inline_tags_keep_their_text() {
        assert_eq!(
            one_line(
                r#"<code>3c257fd</code> chore: <em>bump</em> <b>x</b> <i>y</i> <img src="u" alt="logo"> <kbd>q</kbd>"#
            ),
            "3c257fd chore: bump x y [image: logo] q"
        );
    }

    /// What only looks like a tag is text: generics, comparisons.
    #[test]
    fn angle_brackets_that_are_no_html_stay() {
        assert_eq!(
            one_line("returns Vec<String> or Option<T>, a < b > c"),
            "returns Vec<String> or Option<T>, a < b > c"
        );
    }

    // --- links ---

    /// The URL stays visible, whole, so the terminal can open it.
    #[test]
    fn a_link_shows_its_text_then_its_url() {
        let line = &markdown_lines(
            r#"fix (<a href="https://github.com/acme/api/pull/569">#569</a>)"#,
            WIDTH,
            false,
        )[0];
        assert_eq!(
            line.to_string(),
            "fix (#569 (https://github.com/acme/api/pull/569))"
        );
        let url = line
            .spans
            .iter()
            .find(|span| span.content == "https://github.com/acme/api/pull/569")
            .expect("the URL in one piece");
        assert!(url.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn a_link_whose_text_is_its_url_shows_it_once() {
        assert_eq!(
            one_line(r#"<a href="https://x.y/z">https://x.y/z</a>"#),
            "https://x.y/z"
        );
        assert_eq!(one_line("[https://x.y/z](https://x.y/z)"), "https://x.y/z");
    }

    #[test]
    fn a_markdown_link_reads_like_an_html_one() {
        assert_eq!(
            one_line(
                "Bumps [eslint-plugin-n](https://github.com/acme/eslint-plugin-n) from 1 to 2."
            ),
            "Bumps eslint-plugin-n (https://github.com/acme/eslint-plugin-n) from 1 to 2."
        );
        // Code in the text, an image as the text (a badge).
        assert_eq!(
            one_line("[`3c257fd`](https://x.y/c/3c257fd) fix"),
            "3c257fd (https://x.y/c/3c257fd) fix"
        );
        assert_eq!(
            one_line("[![score](https://x.y/badge.svg)](https://x.y/docs)"),
            "[image: score] (https://x.y/docs)"
        );
        // A task box is no link.
        assert_eq!(one_line("[x] done [y]"), "[x] done [y]");
    }

    #[test]
    fn a_link_url_is_decoded_and_a_link_without_url_is_its_text() {
        assert_eq!(
            one_line(r#"<a href="https://x.y/?a=1&amp;b=2">q</a> <a name="top">top</a>"#),
            "q (https://x.y/?a=1&b=2) top"
        );
        assert_eq!(
            one_line(r#"<a href="https://x.y/c"><code>3c257fd</code></a> chore"#),
            "3c257fd (https://x.y/c) chore"
        );
    }

    /// GitHub hides a link reference definition: Dependabot's markers.
    #[test]
    fn link_definitions_are_hidden() {
        assert_eq!(
            folded("a\n[//]: # (dependabot-automerge-start)\nb"),
            ["a", "b"]
        );
    }

    // --- entities ---

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            one_line("a &amp; b &lt;c&gt; &quot;q&quot; &#39;s&#x27; x&nbsp;y &#8212; &copy;"),
            "a & b <c> \"q\" 's' x y — &copy;"
        );
    }

    /// A decoded `<b>` is text, never a tag to read again.
    #[test]
    fn a_decoded_tag_is_text() {
        assert_eq!(
            one_line("&lt;b&gt;bold&lt;/b&gt; &lt;details&gt;"),
            "<b>bold</b> <details>"
        );
    }

    /// An entity cannot smuggle an escape sequence nor a bidi override.
    #[test]
    fn a_decoded_control_character_is_dropped() {
        let line = one_line("&#27;[31mred &#x1B;x &#8238;y &#0;z &#13;w");
        assert_eq!(line, "[31mred x y z w");
        let line = one_line(r#"<a href="https://x.y/&#27;[2J">t</a>"#);
        assert!(!line.contains('\u{1b}'), "{line:?}");
    }

    /// Code is code: no tag, no entity read inside backticks.
    #[test]
    fn inline_code_is_left_as_typed() {
        assert_eq!(
            one_line("use `<details>` and `a &amp; b` or ``x ` <b>y</b>``"),
            "use <details> and a &amp; b or x ` <b>y</b>"
        );
    }

    // --- a whole Dependabot body ---

    const DEPENDABOT: &str = r#"Bumps [eslint-plugin-n](https://github.com/acme/eslint-plugin-n) from 17.10.3 to 17.15.0.
<details>
<summary>Release notes</summary>
<p><em>Sourced from <a href="https://github.com/acme/eslint-plugin-n/releases">eslint-plugin-n's releases</a>.</em></p>
<blockquote>
<h2>v17.15.0</h2>
<h3>Features</h3>
<ul>
<li><strong>no-unsupported:</strong> Node 23.3.0 &amp; 22.12.0 (<a href="https://github.com/acme/eslint-plugin-n/issues/393">#393</a>)</li>
</ul>
</blockquote>
</details>
<details>
<summary>Commits</summary>
<ul>
<li><a href="https://github.com/acme/eslint-plugin-n/commit/3c257fd"><code>3c257fd</code></a> chore: release 17.15.0</li>
<li>See full diff in <a href="https://github.com/acme/eslint-plugin-n/compare/v17.10.3...v17.15.0">compare view</a></li>
</ul>
</details>
<br />


[![Dependabot compatibility score](https://x.y/badge.svg)](https://x.y/docs)

Dependabot will resolve any conflicts with this PR.

[//]: # (dependabot-automerge-start)
[//]: # (dependabot-automerge-end)

---

<details>
<summary>Dependabot commands and options</summary>
<br />

You can trigger Dependabot actions by commenting on this PR:
- `@dependabot rebase` will rebase this PR
</details>"#;

    #[test]
    fn a_dependabot_body_folds_to_its_summaries() {
        assert_eq!(
            folded(DEPENDABOT),
            [
                "Bumps eslint-plugin-n (https://github.com/acme/eslint-plugin-n) from 17.10.3 to 17.15.0.",
                "▸ Release notes",
                "▸ Commits",
                "",
                "[image: Dependabot compatibility score] (https://x.y/docs)",
                "",
                "Dependabot will resolve any conflicts with this PR.",
                "",
                &"─".repeat(40),
                "",
                "▸ Dependabot commands and options",
            ]
        );
    }

    #[test]
    fn a_dependabot_body_unfolds_readable() {
        assert_eq!(
            unfolded(DEPENDABOT),
            [
                "Bumps eslint-plugin-n (https://github.com/acme/eslint-plugin-n) from 17.10.3 to 17.15.0.",
                "▾ Release notes",
                "  Sourced from eslint-plugin-n's releases (https://github.com/acme/eslint-plugin-n/releases).",
                "  │",
                "  │ ▍v17.15.0",
                "  │",
                "  │ ▍Features",
                "  │",
                "  │   • no-unsupported: Node 23.3.0 & 22.12.0 (#393 (https://github.com/acme/eslint-plugin-n/issues/393))",
                "",
                "▾ Commits",
                "    • 3c257fd (https://github.com/acme/eslint-plugin-n/commit/3c257fd) chore: release 17.15.0",
                "    • See full diff in compare view (https://github.com/acme/eslint-plugin-n/compare/v17.10.3...v17.15.0)",
                "",
                "[image: Dependabot compatibility score] (https://x.y/docs)",
                "",
                "Dependabot will resolve any conflicts with this PR.",
                "",
                &"─".repeat(40),
                "",
                "▾ Dependabot commands and options",
                "",
                "  You can trigger Dependabot actions by commenting on this PR:",
                "    • @dependabot rebase will rebase this PR",
            ]
        );
    }

    // --- emphasis ---

    /// The span that reads `text` in `line`.
    fn span<'a>(line: &'a Line, text: &str) -> &'a Span<'a> {
        line.spans
            .iter()
            .find(|span| span.content == text)
            .unwrap_or_else(|| panic!("no span {text:?} in {line:?}"))
    }

    /// The modifiers of the span that reads `text` in `line`.
    fn modifiers(line: &Line, text: &str) -> Modifier {
        span(line, text).style.add_modifier
    }

    #[test]
    fn stars_and_underscores_make_bold_and_italic() {
        let line = &markdown_lines(
            "**For reviewers**: a *small* and __bold__ _one_",
            WIDTH,
            false,
        )[0];
        assert_eq!(line.to_string(), "For reviewers: a small and bold one");
        assert_eq!(modifiers(line, "For reviewers"), Modifier::BOLD);
        assert_eq!(modifiers(line, ": a "), Modifier::empty());
        assert_eq!(modifiers(line, "small"), Modifier::ITALIC);
        assert_eq!(modifiers(line, "bold"), Modifier::BOLD);
        assert_eq!(modifiers(line, "one"), Modifier::ITALIC);
    }

    #[test]
    fn emphasis_nests_and_reads_in_html_too() {
        let line = &markdown_lines(
            "**a *b* c** <strong>d</strong> <em>e</em> <b>f</b> <i>g</i>",
            WIDTH,
            false,
        )[0];
        assert_eq!(line.to_string(), "a b c d e f g");
        assert_eq!(modifiers(line, "b"), Modifier::BOLD | Modifier::ITALIC);
        assert_eq!(modifiers(line, " c"), Modifier::BOLD);
        assert_eq!(modifiers(line, "d"), Modifier::BOLD);
        assert_eq!(modifiers(line, "e"), Modifier::ITALIC);
        assert_eq!(modifiers(line, "f"), Modifier::BOLD);
        assert_eq!(modifiers(line, "g"), Modifier::ITALIC);
    }

    /// What only looks like emphasis stays as typed: an underscore inside a
    /// word, a star with spaces around it, one never closed, code.
    #[test]
    fn stars_and_underscores_that_are_no_emphasis_stay() {
        for typed in [
            "snake_case_name and file_v2_final.rs",
            "a trailing_underscore_ then text",
            "2 * 3 * 4 and a ** b",
            "an *unclosed star",
        ] {
            assert_eq!(one_line(typed), typed);
        }
        assert_eq!(one_line("`**not bold**`"), "**not bold**");
    }

    // --- tables ---

    const TABLE: &str = "| Package | From | To |\n| --- | :--- | ---: |\n| [eslint](https://github.com/acme/eslint) | `10.11.0` | `10.12.0` |\n|ts-proto | `2.12.0` | `2.13.0` |";

    /// Columns lined up under a bold header; a link shows its text only,
    /// underlined, its URL listed under the table, whole and clickable.
    #[test]
    fn a_table_lines_its_columns_up_and_lists_its_links_under_it() {
        let lines = markdown_lines(TABLE, WIDTH, false);
        let row = |a: &str, b: &str, c: &str| format!("{a:<8}  {b:<7}  {c:>7}");
        assert_eq!(
            texts(&lines),
            [
                row("Package", "From", "To"),
                "─".repeat(26),
                row("eslint", "10.11.0", "10.12.0"),
                row("ts-proto", "2.12.0", "2.13.0"),
                String::new(),
                "eslint  https://github.com/acme/eslint".to_string(),
            ]
        );
        // A link twice in the table is listed once.
        let twice = format!("{TABLE}\n| [eslint](https://github.com/acme/eslint) | 1 | 2 |");
        assert_eq!(markdown_lines(&twice, WIDTH, false).len(), 7);
        assert!(lines[0].style.add_modifier.contains(Modifier::BOLD));
        assert!(modifiers(&lines[2], "eslint").contains(Modifier::UNDERLINED));
        assert!(
            modifiers(&lines[5], "https://github.com/acme/eslint").contains(Modifier::UNDERLINED)
        );
    }

    /// Too wide for the view, each row reads as `header: cell` pairs.
    #[test]
    fn a_table_too_wide_reads_row_by_row() {
        assert_eq!(
            texts(&markdown_lines(TABLE, 20, false)),
            [
                "Package: eslint · From: 10.11.0 · To: 10.12.0",
                "Package: ts-proto · From: 2.12.0 · To: 2.13.0",
                "",
                "eslint  https://github.com/acme/eslint",
            ]
        );
    }

    #[test]
    fn a_table_cell_may_hold_an_escaped_pipe() {
        assert_eq!(
            folded("a | b\n---|---\n`x\\|y` | z\nalone\n\nafter"),
            ["a    b", "──────", "x|y  z", "alone", "", "after"]
        );
    }

    /// No delimiter row, or one that does not match the header: no table.
    #[test]
    fn pipes_without_a_delimiter_row_are_text() {
        assert_eq!(folded("a | b\nc | d"), ["a | b", "c | d"]);
        assert_eq!(
            folded("| a | b |\n| --- |\n| c | d |"),
            ["| a | b |", "| --- |", "| c | d |"]
        );
    }

    #[test]
    fn a_table_folds_with_its_details_block() {
        let body =
            "<details>\n<summary>Versions</summary>\n\n| a | b |\n|---|---|\n| 1 | 2 |\n</details>";
        assert_eq!(folded(body), ["▸ Versions"]);
        assert_eq!(
            unfolded(body),
            ["▾ Versions", "", "  a  b", "  ────", "  1  2"]
        );
    }

    // --- code, emoji, strikethrough, task boxes, autolinks ---

    /// Inline code drops its backticks for a color, where GitHub gives it
    /// a background.
    #[test]
    fn inline_code_shows_in_yellow_without_its_backticks() {
        let line = &markdown_lines(
            "Updates `eslint` from <code>1.0</code> to `` a`b ``",
            WIDTH,
            false,
        )[0];
        assert_eq!(line.to_string(), "Updates eslint from 1.0 to a`b");
        for code in ["eslint", "1.0", "a`b"] {
            assert_eq!(span(line, code).style.fg, Some(Color::Yellow), "{code}");
        }
        assert_eq!(span(line, "Updates ").style.fg, None);
    }

    #[test]
    fn emoji_shortcodes_become_emoji() {
        assert_eq!(
            one_line(":bug: Bug Fixes :rocket: :+1: :white_check_mark:"),
            "🐛 Bug Fixes 🚀 👍 ✅"
        );
        assert_eq!(folded("### :sparkles: Features"), ["▍✨ Features"]);
        // Unknown, glued to a word, a clock, code: as typed.
        assert_eq!(
            one_line(":not_an_emoji: a:bug: 10:30:00 `:bug:`"),
            ":not_an_emoji: a:bug: 10:30:00 :bug:"
        );
    }

    #[test]
    fn tildes_and_html_strike_through() {
        let line = &markdown_lines("~~old~~ <del>gone</del> <s>x</s> a ~ b ~~ c", WIDTH, false)[0];
        assert_eq!(line.to_string(), "old gone x a ~ b ~~ c");
        for text in ["old", "gone", "x"] {
            assert!(
                modifiers(line, text).contains(Modifier::CROSSED_OUT),
                "{text}"
            );
        }
    }

    #[test]
    fn task_boxes_read_as_boxes() {
        assert_eq!(
            folded("- [x] Tests added\n- [ ] Load-tested\n  * [X] nested"),
            ["  ☑ Tests added", "  ☐ Load-tested", "    ☑ nested"]
        );
    }

    #[test]
    fn an_autolink_drops_its_angle_brackets() {
        assert_eq!(
            one_line("see <https://x.y/z> or <http://a.b>, not <ftp://c.d>"),
            "see https://x.y/z or http://a.b, not <ftp://c.d>"
        );
    }
}
