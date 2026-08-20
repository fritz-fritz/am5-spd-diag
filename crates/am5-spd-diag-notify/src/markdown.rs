//! Markdown for the GTK results window.
//!
//! [`pulldown-cmark`] parses the ticket subset (headings, emphasis, lists,
//! fenced/indented code, GFM tables). Blocks become GTK labels and grids so
//! tables get a real allocated width. Firmware e820 lines get type colors.

use gtk4::gdk::Display;
use gtk4::prelude::{BoxExt, GridExt, WidgetExt};
use gtk4::{
    glib, pango, Align, Box as GtkBox, CssProvider, Grid, Label, NaturalWrapMode, Orientation,
    STYLE_PROVIDER_PRIORITY_USER,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::sync::OnceLock;

const CSS: &str = r#"
label.am5-md-h1 {
  font-weight: 700;
  font-size: 18pt;
  margin-top: 8px;
}
label.am5-md-h2 {
  font-weight: 700;
  font-size: 15pt;
  margin-top: 10px;
}
label.am5-md-h3 {
  font-weight: 600;
  font-size: 13pt;
  margin-top: 8px;
}
label.am5-md-li {
  margin-left: 8px;
}
label.am5-md-pre {
  font-family: monospace;
}
.am5-md-table {
  margin: 8px 0 12px 0;
}
.am5-md-th, .am5-md-td {
  padding: 5px 10px;
  border: 1px solid alpha(currentColor, 0.18);
}
.am5-md-th {
  font-weight: 700;
  background-color: alpha(currentColor, 0.10);
}
.am5-md-td-alt {
  background-color: alpha(currentColor, 0.04);
}
label.e820-ram {
  font-family: monospace;
  color: #8ff0a4;
}
label.e820-reserved {
  font-family: monospace;
  color: #ffbe6f;
}
label.e820-acpi {
  font-family: monospace;
  color: #99c1f1;
}
label.e820-nvs {
  font-family: monospace;
  color: #62a0ea;
}
label.e820-pmem {
  font-family: monospace;
  color: #dc8add;
}
label.e820-other {
  font-family: monospace;
  color: #c0bfbc;
}
scrollbar slider {
  min-width: 8px;
  min-height: 8px;
}
"#;

#[derive(Clone, Copy)]
enum BlockKind {
    None,
    Heading(&'static str),
    Paragraph,
    Item,
}

pub fn fill_box(parent: &GtkBox, markdown: bool, text: &str) {
    ensure_css();
    clear_box(parent);
    if !markdown {
        append_preformatted(parent, text);
        return;
    }
    render_markdown(parent, text);
}

fn clear_box(parent: &GtkBox) {
    while let Some(child) = parent.first_child() {
        parent.remove(&child);
    }
}

fn ensure_css() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let Some(display) = Display::default() else {
            return;
        };
        let provider = CssProvider::new();
        provider.load_from_data(CSS);
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            STYLE_PROVIDER_PRIORITY_USER,
        );
    });
}

fn esc(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

const COLOR_OK: &str = "#8ff0a4";
const COLOR_BAD: &str = "#f66151";
const COLOR_WARN: &str = "#ffbe6f";
const COLOR_ACPI: &str = "#99c1f1";
const COLOR_NVS: &str = "#62a0ea";
const COLOR_PMEM: &str = "#dc8add";
const COLOR_MUTED: &str = "#c0bfbc";

fn token_at(rest: &str, tok: &str) -> bool {
    if !rest.starts_with(tok) {
        return false;
    }
    match rest.as_bytes().get(tok.len()) {
        None => true,
        Some(b) => !b.is_ascii_alphanumeric(),
    }
}

fn colorize_esc(text: &str) -> String {
    let tokens: &[(&str, &str)] = &[
        ("system ram", COLOR_OK),
        ("acpi nvs", COLOR_NVS),
        ("acpi data", COLOR_ACPI),
        ("bios-e820", COLOR_MUTED),
        ("corrupted", COLOR_BAD),
        ("healthy", COLOR_OK),
        ("reserved", COLOR_WARN),
        ("unknown", COLOR_BAD),
        ("corrupt", COLOR_BAD),
    ];
    let lower = text.to_ascii_lowercase();
    let mut out = String::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &lower[i..];
        let mut hit = None;
        for (tok, color) in tokens {
            if token_at(rest, tok) {
                hit = Some((tok.len(), *color));
                break;
            }
        }
        if let Some((n, color)) = hit {
            if !buf.is_empty() {
                out.push_str(&esc(&buf));
                buf.clear();
            }
            out.push_str("<span foreground=\"");
            out.push_str(color);
            out.push_str("\">");
            out.push_str(&esc(&text[i..i + n]));
            out.push_str("</span>");
            i += n;
        } else {
            let ch = text[i..].chars().next().unwrap();
            buf.push(ch);
            i += ch.len_utf8();
        }
    }
    if !buf.is_empty() {
        out.push_str(&esc(&buf));
    }
    out
}

fn e820_color(tag: &str) -> &'static str {
    match tag {
        "e820-ram" => COLOR_OK,
        "e820-reserved" => COLOR_WARN,
        "e820-acpi" => COLOR_ACPI,
        "e820-nvs" => COLOR_NVS,
        "e820-pmem" => COLOR_PMEM,
        _ => COLOR_MUTED,
    }
}

fn split_e820_text(text: &str) -> Option<(String, Vec<String>)> {
    let pos = text.find("BIOS-e820:")?;
    let intro = text[..pos].trim_end().to_string();
    let mut lines = Vec::new();
    for part in text[pos..].split("BIOS-e820:") {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        lines.push(format!("BIOS-e820: {part}"));
    }
    if lines.is_empty() {
        None
    } else {
        Some((intro, lines))
    }
}

fn e820_tag(line: &str) -> Option<&'static str> {
    if !looks_like_e820(line) {
        return None;
    }
    let l = line.to_ascii_lowercase();
    if l.contains("system ram") || l.contains("usable") {
        Some("e820-ram")
    } else if l.contains("acpi nvs") {
        Some("e820-nvs")
    } else if l.contains("acpi") {
        Some("e820-acpi")
    } else if l.contains("reserved") {
        Some("e820-reserved")
    } else if l.contains("persistent") || l.contains("pmem") {
        Some("e820-pmem")
    } else {
        Some("e820-other")
    }
}

fn looks_like_e820(line: &str) -> bool {
    let t = line.trim();
    t.contains("BIOS-e820") || t.contains("[mem 0x") || t.contains("[mem 0X")
}

fn heading_class(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "am5-md-h1",
        HeadingLevel::H2 => "am5-md-h2",
        _ => "am5-md-h3",
    }
}

fn apply_wrap(lab: &Label, wrap: bool) {
    lab.set_wrap(wrap);
    if wrap {
        lab.set_wrap_mode(pango::WrapMode::WordChar);
        lab.set_natural_wrap_mode(NaturalWrapMode::Word);
        lab.set_hexpand(true);
        lab.set_halign(Align::Fill);
    }
}

fn append_markup_label(parent: &GtkBox, markup: &str, class: &str, wrap: bool) {
    if markup.trim().is_empty() {
        return;
    }
    let lab = Label::new(None);
    lab.set_markup(markup);
    apply_wrap(&lab, wrap);
    lab.set_xalign(0.0);
    lab.set_selectable(true);
    if !class.is_empty() {
        lab.add_css_class(class);
    }
    parent.append(&lab);
}

fn append_plain_line(parent: &GtkBox, text: &str, class: &str, wrap: bool) {
    let lab = Label::new(Some(text));
    apply_wrap(&lab, wrap);
    lab.set_xalign(0.0);
    lab.set_selectable(true);
    lab.add_css_class(class);
    parent.append(&lab);
}

fn append_code_lines(parent: &GtkBox, text: &str) {
    let wrap = GtkBox::new(Orientation::Vertical, 0);
    wrap.add_css_class("am5-md-code");
    wrap.set_halign(Align::Fill);
    wrap.set_hexpand(true);
    let mut any = false;
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        any = true;
        let class = e820_tag(line).unwrap_or("am5-md-pre");
        let lab = Label::new(None);
        let fg = e820_tag(line).map(e820_color);
        let inner = esc(line);
        let markup = if let Some(fg) = fg {
            format!("<span font_family=\"monospace\" foreground=\"{fg}\">{inner}</span>")
        } else {
            format!("<span font_family=\"monospace\">{inner}</span>")
        };
        lab.set_markup(&markup);
        apply_wrap(&lab, true);
        lab.set_xalign(0.0);
        lab.set_selectable(true);
        lab.add_css_class(class);
        wrap.append(&lab);
    }
    if any {
        parent.append(&wrap);
    }
}

fn append_preformatted(parent: &GtkBox, text: &str) {
    for line in text.split('\n') {
        if line.is_empty() {
            let sp = Label::new(None);
            sp.set_height_request(8);
            parent.append(&sp);
            continue;
        }
        if e820_tag(line).is_some() {
            append_code_lines(parent, line);
        } else {
            let markup = format!(
                "<span font_family=\"monospace\">{}</span>",
                colorize_esc(line)
            );
            append_markup_label(parent, &markup, "am5-md-pre", true);
        }
    }
}

fn flush_markup(parent: &GtkBox, markup: &mut String, kind: &mut BlockKind) {
    let class = match *kind {
        BlockKind::None => {
            markup.clear();
            return;
        }
        BlockKind::Heading(class) => class,
        BlockKind::Paragraph => "am5-md-p",
        BlockKind::Item => "am5-md-li",
    };
    append_markup_label(parent, markup, class, true);
    markup.clear();
    *kind = BlockKind::None;
}

fn start_block(parent: &GtkBox, markup: &mut String, kind: &mut BlockKind, next: BlockKind) {
    if !matches!(*kind, BlockKind::None) {
        flush_markup(parent, markup, kind);
    }
    *kind = next;
}

struct TableBuild {
    rows: Vec<(bool, Vec<String>)>,
    row: Vec<String>,
    cell: String,
    in_head: bool,
}

impl TableBuild {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            row: Vec::new(),
            cell: String::new(),
            in_head: false,
        }
    }
}

fn emit_table(parent: &GtkBox, table: &TableBuild) {
    if table.rows.is_empty() {
        return;
    }
    let grid = Grid::new();
    grid.set_column_spacing(0);
    grid.set_row_spacing(0);
    grid.set_hexpand(true);
    grid.set_halign(Align::Fill);
    grid.add_css_class("am5-md-table");
    let cols = table
        .rows
        .iter()
        .map(|(_, cells)| cells.len())
        .max()
        .unwrap_or(1);
    for (r, (is_head, cells)) in table.rows.iter().enumerate() {
        for (c, text) in cells.iter().enumerate() {
            let lab = Label::new(None);
            lab.set_markup(&colorize_esc(text));
            // hexpand on every cell makes GtkGrid report a multi-thousand-px
            // minimum width inside a ScrolledWindow.
            apply_wrap(&lab, true);
            lab.set_hexpand(c + 1 == cols);
            lab.set_max_width_chars(if cols <= 2 { 72 } else { 18 });
            lab.set_xalign(0.0);
            lab.set_selectable(true);
            lab.add_css_class(if *is_head { "am5-md-th" } else { "am5-md-td" });
            if !*is_head && r % 2 == 1 {
                lab.add_css_class("am5-md-td-alt");
            }
            grid.attach(&lab, c as i32, r as i32, 1, 1);
        }
    }
    parent.append(&grid);
}

fn render_markdown(parent: &GtkBox, text: &str) {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let mut kind = BlockKind::None;
    let mut markup = String::new();
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut table: Option<TableBuild> = None;
    let mut in_cell = false;
    let mut code: Option<String> = None;

    for event in Parser::new_ext(text, opts) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                start_block(
                    parent,
                    &mut markup,
                    &mut kind,
                    BlockKind::Heading(heading_class(level)),
                );
            }
            Event::End(TagEnd::Heading(_)) => flush_markup(parent, &mut markup, &mut kind),
            Event::Start(Tag::Paragraph) => {
                if matches!(kind, BlockKind::None) {
                    start_block(parent, &mut markup, &mut kind, BlockKind::Paragraph);
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if matches!(kind, BlockKind::Paragraph) {
                    flush_markup(parent, &mut markup, &mut kind);
                }
            }
            Event::Start(Tag::Emphasis) => markup.push_str("<i>"),
            Event::End(TagEnd::Emphasis) => markup.push_str("</i>"),
            Event::Start(Tag::Strong) => markup.push_str("<b>"),
            Event::End(TagEnd::Strong) => markup.push_str("</b>"),
            Event::Start(Tag::Strikethrough) => markup.push_str("<s>"),
            Event::End(TagEnd::Strikethrough) => markup.push_str("</s>"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                markup.push_str("<a href=\"");
                markup.push_str(&esc(&dest_url));
                markup.push_str("\">");
            }
            Event::End(TagEnd::Link) => markup.push_str("</a>"),
            Event::Start(Tag::BlockQuote(_)) => {}
            Event::End(TagEnd::BlockQuote(_)) => {
                if matches!(kind, BlockKind::Paragraph) {
                    flush_markup(parent, &mut markup, &mut kind);
                }
            }
            Event::Start(Tag::List(start)) => {
                if matches!(kind, BlockKind::Item | BlockKind::Paragraph) {
                    flush_markup(parent, &mut markup, &mut kind);
                }
                list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                let bullet = match list_stack.last_mut() {
                    Some(Some(n)) => {
                        let label = format!("{n}. ");
                        *n += 1;
                        label
                    }
                    _ => "• ".into(),
                };
                start_block(parent, &mut markup, &mut kind, BlockKind::Item);
                markup.push_str(&esc(&bullet));
            }
            Event::End(TagEnd::Item) => flush_markup(parent, &mut markup, &mut kind),
            Event::Start(Tag::CodeBlock(_)) => {
                flush_markup(parent, &mut markup, &mut kind);
                code = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(block) = code.take() {
                    append_code_lines(parent, &block);
                }
            }
            Event::Start(Tag::Table(_)) => {
                flush_markup(parent, &mut markup, &mut kind);
                table = Some(TableBuild::new());
            }
            Event::End(TagEnd::Table) => {
                if let Some(built) = table.take() {
                    emit_table(parent, &built);
                }
            }
            Event::Start(Tag::TableHead) => {
                if let Some(t) = table.as_mut() {
                    t.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(t) = table.as_mut() {
                    if !t.row.is_empty() {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push((true, row));
                    }
                    t.in_head = false;
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(t) = table.as_mut() {
                    t.row.clear();
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(t) = table.as_mut() {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push((t.in_head, row));
                }
            }
            Event::Start(Tag::TableCell) => {
                in_cell = true;
                if let Some(t) = table.as_mut() {
                    t.cell.clear();
                }
            }
            Event::End(TagEnd::TableCell) => {
                in_cell = false;
                if let Some(t) = table.as_mut() {
                    let cell = std::mem::take(&mut t.cell);
                    t.row.push(cell.trim().to_string());
                }
            }
            Event::Code(s) => {
                if let Some(t) = table.as_mut().filter(|_| in_cell) {
                    t.cell.push_str(&s);
                } else if let Some(block) = code.as_mut() {
                    block.push_str(&s);
                } else {
                    markup.push_str("<span font_family=\"monospace\">");
                    markup.push_str(&esc(&s));
                    markup.push_str("</span>");
                }
            }
            Event::Text(s) => {
                if let Some(t) = table.as_mut().filter(|_| in_cell) {
                    t.cell.push_str(&s);
                } else if let Some(block) = code.as_mut() {
                    block.push_str(&s);
                } else if let Some((intro, lines)) = split_e820_text(&s) {
                    if !intro.is_empty() {
                        if matches!(kind, BlockKind::None) {
                            start_block(parent, &mut markup, &mut kind, BlockKind::Paragraph);
                        }
                        markup.push_str(&colorize_esc(&intro));
                    }
                    flush_markup(parent, &mut markup, &mut kind);
                    for line in lines {
                        append_code_lines(parent, &line);
                    }
                } else if looks_like_e820(&s) {
                    flush_markup(parent, &mut markup, &mut kind);
                    append_code_lines(parent, s.trim());
                } else {
                    markup.push_str(&colorize_esc(&s));
                }
            }
            Event::SoftBreak => {
                if let Some(t) = table.as_mut().filter(|_| in_cell) {
                    t.cell.push(' ');
                } else if let Some(block) = code.as_mut() {
                    block.push('\n');
                } else if matches!(kind, BlockKind::None) {
                } else {
                    markup.push(' ');
                }
            }
            Event::HardBreak => {
                if let Some(t) = table.as_mut().filter(|_| in_cell) {
                    t.cell.push(' ');
                } else if let Some(block) = code.as_mut() {
                    block.push('\n');
                } else {
                    markup.push('\n');
                }
            }
            Event::Rule => {
                flush_markup(parent, &mut markup, &mut kind);
                append_plain_line(parent, "────────────────", "am5-md-p", false);
            }
            Event::Html(s) | Event::InlineHtml(s) => {
                if let Some(t) = table.as_mut().filter(|_| in_cell) {
                    t.cell.push_str(&s);
                } else if let Some(block) = code.as_mut() {
                    block.push_str(&s);
                } else {
                    markup.push_str(&esc(&s));
                }
            }
            _ => {}
        }
    }
    flush_markup(parent, &mut markup, &mut kind);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e820_colors() {
        assert_eq!(
            e820_tag("BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM"),
            Some("e820-ram")
        );
        assert_eq!(
            e820_tag("BIOS-e820: [mem 0x000000000009fc00-0x000000000009ffff] reserved"),
            Some("e820-reserved")
        );
        assert_eq!(
            e820_tag("BIOS-e820: [mem 0x00000000000a0000-0x00000000000bffff] ACPI NVS"),
            Some("e820-nvs")
        );
        assert_eq!(
            e820_tag("BIOS-e820: [mem 0x00000000000c0000-0x00000000000cffff] ACPI data"),
            Some("e820-acpi")
        );
        assert_eq!(e820_tag("not a map"), None);
    }

    #[test]
    fn colorize_marks_health_words() {
        let markup = colorize_esc("SPD now: healthy, previously corrupted");
        assert!(markup.contains("foreground=\"#8ff0a4\""));
        assert!(markup.contains("foreground=\"#f66151\""));
        assert!(markup.contains("healthy"));
        assert!(markup.contains("corrupted"));
    }

    #[test]
    fn split_folded_e820_paragraph() {
        let blob =
            "Healthy ts: BIOS-e820: [mem 0x0-0x1] System RAM BIOS-e820: [mem 0x2-0x3] reserved";
        let (intro, lines) = split_e820_text(blob).expect("e820 lines");
        assert!(intro.contains("Healthy"));
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("System RAM"));
        assert!(lines[1].contains("reserved"));
        assert_eq!(e820_tag(&lines[0]), Some("e820-ram"));
        assert_eq!(e820_tag(&lines[1]), Some("e820-reserved"));
    }

    #[test]
    fn fixture_parses_gfm_tables() {
        let md = include_str!("../../../tests/fixture/report-out.md");
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        let mut tables = 0usize;
        let mut first_header = Vec::new();
        let mut in_head = false;
        let mut in_cell = false;
        let mut cell = String::new();
        for ev in Parser::new_ext(md, opts) {
            match ev {
                Event::Start(Tag::Table(_)) => tables += 1,
                Event::Start(Tag::TableHead) => in_head = true,
                Event::End(TagEnd::TableHead) => in_head = false,
                Event::Start(Tag::TableCell) => {
                    in_cell = true;
                    cell.clear();
                }
                Event::End(TagEnd::TableCell) => {
                    in_cell = false;
                    if in_head && tables == 1 {
                        first_header.push(cell.trim().to_string());
                    }
                }
                Event::Text(s) if in_cell => cell.push_str(&s),
                Event::Code(s) if in_cell => cell.push_str(&s),
                _ => {}
            }
        }
        assert!(tables >= 3, "expected GFM tables, got {tables}");
        assert_eq!(first_header, ["Item", "Details"]);
    }
}
