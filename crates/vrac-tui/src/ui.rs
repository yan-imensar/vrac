use std::io::{self, Stdout, Write};
use std::path::Path;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use vrac::{NodeId, Placement};

use super::{
    App, BacklinkView, EditTarget, Editor, LauncherItem, PromptKind, ReferencePrompt, Search,
    TagPrompt, VisibleNode,
};

#[derive(Clone)]
pub(super) struct DisplayLine {
    pub(super) selected: bool,
    pub(super) text: String,
    pub(super) cursor: Option<usize>,
    content_start: usize,
}

pub(super) fn draw(stdout: &mut Stdout, app: &mut App, path: &Path) -> io::Result<()> {
    let (width, height) = terminal::size()?;
    let width = usize::from(width);
    let height = usize::from(height);
    app.viewport_width = width;
    let completion_height = completion_height(app, height);
    let body_height = height.saturating_sub(4 + completion_height);
    let lines = match (&app.backlinks, &app.search) {
        (Some(view), _) => backlink_lines(view, width),
        (None, Some(search)) => search_lines(search, width),
        (None, None) => display_lines(app, width),
    };
    let selected_start = lines
        .iter()
        .position(|line| line.selected)
        .unwrap_or_default();
    let selected_end = lines
        .iter()
        .rposition(|line| line.selected)
        .unwrap_or(selected_start);
    if selected_start < app.scroll {
        app.scroll = selected_start;
    } else if body_height > 0 && selected_end >= app.scroll + body_height {
        let selection_height = selected_end + 1 - selected_start;
        app.scroll = if selection_height > body_height {
            selected_start
        } else {
            selected_end + 1 - body_height
        };
    }
    if let Some(cursor_line) = lines.iter().position(|line| line.cursor.is_some()) {
        if cursor_line < app.scroll {
            app.scroll = cursor_line;
        } else if body_height > 0 && cursor_line >= app.scroll + body_height {
            app.scroll = cursor_line + 1 - body_height;
        }
    }
    app.scroll = app.scroll.min(lines.len().saturating_sub(body_height));

    queue!(
        stdout,
        BeginSynchronizedUpdate,
        Hide,
        MoveTo(0, 0),
        Clear(ClearType::All)
    )?;
    queue!(
        stdout,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        Print(fit(
            &format!(
                "VRAC  {}",
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into()
                )
            ),
            width
        )),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    if height > 1 {
        queue!(stdout, MoveTo(0, 1), SetForegroundColor(Color::DarkGrey))?;
        queue!(
            stdout,
            Print(fit(&format!("  {}", app.focus_label()), width)),
            ResetColor
        )?;
    }

    if completion_height > 0 {
        draw_completion_panel(stdout, app, width, 2 + body_height, completion_height)?;
    }

    let mut inline_cursor = None;
    for (offset, line) in lines.iter().skip(app.scroll).take(body_height).enumerate() {
        let row = offset + 2;
        queue!(stdout, MoveTo(0, u16::try_from(row).unwrap_or(u16::MAX)))?;
        draw_display_line(stdout, line, width)?;
        if let Some(column) = line.cursor {
            inline_cursor = Some((column.min(width.saturating_sub(1)), row));
        }
    }

    if app.backlinks.is_some() {
        draw_backlink_footer(stdout, &app.status, width, height)?;
    } else if let Some(prompt) = &app.tag_prompt {
        draw_tag_footer(stdout, prompt, &app.status, width, height)?;
    } else if let Some(search) = &app.search {
        draw_search_footer(stdout, search, &app.status, width, height)?;
    } else if let Some(prompt) = &app.reference_prompt {
        draw_reference_footer(stdout, prompt, width, height)?;
        if let Some((column, row)) = inline_cursor {
            queue!(
                stdout,
                Show,
                MoveTo(
                    u16::try_from(column).unwrap_or(u16::MAX),
                    u16::try_from(row).unwrap_or(u16::MAX)
                )
            )?;
        }
    } else if app.editor.is_some() {
        draw_editor_status(stdout, &app.status, width, height)?;
        if let Some((column, row)) = inline_cursor {
            queue!(
                stdout,
                Show,
                MoveTo(
                    u16::try_from(column).unwrap_or(u16::MAX),
                    u16::try_from(row).unwrap_or(u16::MAX)
                )
            )?;
        }
    } else {
        draw_normal_footer(stdout, &app.status, width, height)?;
    }
    queue!(stdout, EndSynchronizedUpdate)?;
    stdout.flush()
}

fn draw_display_line(stdout: &mut Stdout, line: &DisplayLine, width: usize) -> io::Result<()> {
    let fitted = fit(&line.text, width);
    let (prefix, content) = split_content(&fitted, line.content_start);
    queue!(
        stdout,
        SetForegroundColor(if line.selected {
            Color::Cyan
        } else {
            Color::DarkGrey
        }),
        SetAttribute(if line.selected {
            Attribute::Bold
        } else {
            Attribute::Reset
        }),
        Print(prefix),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    if line.selected {
        queue!(stdout, SetAttribute(Attribute::Bold))?;
    }
    draw_inline_content(stdout, content)?;
    queue!(stdout, SetAttribute(Attribute::Reset), ResetColor)
}

pub(super) fn split_content(text: &str, requested: usize) -> (&str, &str) {
    let mut split = requested.min(text.len());
    while !text.is_char_boundary(split) {
        split -= 1;
    }
    text.split_at(split)
}

fn draw_inline_content(stdout: &mut Stdout, content: &str) -> io::Result<()> {
    let mut offset = 0;
    while offset < content.len() {
        let remaining = &content[offset..];
        if let Some(end) = remaining
            .strip_prefix("[[")
            .and_then(|label| label.find("]]").map(|end| end + 4))
        {
            queue!(
                stdout,
                SetForegroundColor(Color::Cyan),
                Print(&remaining[..end]),
                ResetColor
            )?;
            offset += end;
            continue;
        }
        if remaining.starts_with('#')
            && (offset == 0
                || content[..offset]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace))
        {
            let end = remaining
                .char_indices()
                .skip(1)
                .find(|(_, character)| character.is_whitespace())
                .map_or(remaining.len(), |(index, _)| index);
            queue!(
                stdout,
                SetForegroundColor(Color::Magenta),
                Print(&remaining[..end]),
                ResetColor
            )?;
            offset += end;
            continue;
        }
        let next = remaining
            .char_indices()
            .nth(1)
            .map_or(remaining.len(), |(index, _)| index);
        queue!(stdout, Print(&remaining[..next]))?;
        offset += next;
    }
    Ok(())
}

fn completion_height(app: &App, height: usize) -> usize {
    if app.tag_prompt.is_none() && app.reference_prompt.is_none() {
        return 0;
    }
    6.min(height.saturating_sub(6))
}

fn draw_completion_panel(
    stdout: &mut Stdout,
    app: &App,
    width: usize,
    start: usize,
    height: usize,
) -> io::Result<()> {
    if height == 0 {
        return Ok(());
    }
    let (title, options, selected) = if let Some(prompt) = &app.tag_prompt {
        (
            "TAGS",
            if prompt.results.is_empty() {
                vec!["Type a tag".into()]
            } else {
                prompt.results.iter().map(|tag| format!("#{tag}")).collect()
            },
            prompt.selected,
        )
    } else if let Some(prompt) = &app.reference_prompt {
        (
            "REFERENCES",
            if prompt.results.is_empty() {
                if prompt.query.is_empty() {
                    vec!["Type to search concepts".into()]
                } else {
                    vec![format!("Create [[{}]]", prompt.query)]
                }
            } else {
                prompt
                    .results
                    .iter()
                    .map(|node| node.text.replace('\n', " ↵ "))
                    .collect()
            },
            prompt.selected,
        )
    } else {
        return Ok(());
    };
    queue!(
        stdout,
        MoveTo(0, u16::try_from(start).unwrap_or(u16::MAX)),
        SetForegroundColor(Color::DarkGrey),
        Print(fit(&format!("  ── {title} "), width)),
        ResetColor
    )?;
    let visible_options = height.saturating_sub(1);
    let first = selected
        .saturating_add(1)
        .saturating_sub(visible_options)
        .min(options.len().saturating_sub(visible_options));
    for (offset, option) in options.iter().skip(first).take(visible_options).enumerate() {
        let active = first + offset == selected;
        queue!(
            stdout,
            MoveTo(0, u16::try_from(start + offset + 1).unwrap_or(u16::MAX))
        )?;
        if active {
            queue!(
                stdout,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold)
            )?;
        } else {
            queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
        }
        queue!(
            stdout,
            Print(fit(
                &format!("  {} {option}", if active { "›" } else { " " }),
                width
            )),
            SetAttribute(Attribute::Reset),
            ResetColor
        )?;
    }
    Ok(())
}

fn draw_reference_footer(
    stdout: &mut Stdout,
    prompt: &ReferencePrompt,
    width: usize,
    height: usize,
) -> io::Result<()> {
    if height >= 2 {
        queue!(stdout, MoveTo(0, u16::try_from(height - 2).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(fit(
                "REFERENCE  ↑/↓ select · Enter complete/create · Esc keep literal",
                width
            )),
            ResetColor
        )?;
    }
    if height >= 1 {
        let choice = prompt.results.get(prompt.selected).map_or_else(
            || {
                if prompt.query.is_empty() {
                    "Type after [[ to search".into()
                } else {
                    format!("Enter creates [[{}]]", prompt.query)
                }
            },
            |node| format!("Enter inserts [[{}]]", node.text.replace('\n', " ")),
        );
        queue!(stdout, MoveTo(0, u16::try_from(height - 1).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(fit(&choice, width)),
            ResetColor
        )?;
    }
    Ok(())
}

fn draw_backlink_footer(
    stdout: &mut Stdout,
    status: &str,
    width: usize,
    height: usize,
) -> io::Result<()> {
    if height >= 2 {
        let label = if status.is_empty() {
            "BACKLINKS  j/k select · Enter open · b/Esc close"
        } else {
            status
        };
        queue!(stdout, MoveTo(0, u16::try_from(height - 2).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(fit(label, width)),
            ResetColor
        )?;
    }
    if height >= 1 {
        queue!(stdout, MoveTo(0, u16::try_from(height - 1).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(fit("Context paths are newest Journal day first", width)),
            ResetColor
        )?;
    }
    Ok(())
}

fn draw_tag_footer(
    stdout: &mut Stdout,
    prompt: &TagPrompt,
    status: &str,
    width: usize,
    height: usize,
) -> io::Result<()> {
    if height >= 2 {
        let label = if status.is_empty() {
            "TAG  ↑/↓ select · Enter toggle · Esc cancel"
        } else {
            status
        };
        queue!(stdout, MoveTo(0, u16::try_from(height - 2).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(fit(label, width)),
            ResetColor
        )?;
    }
    if height >= 1 {
        let input_width = width.saturating_sub(2);
        let query = fit(&prompt.query, input_width);
        queue!(stdout, MoveTo(0, u16::try_from(height - 1).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Cyan),
            Print("# "),
            ResetColor,
            Print(&query),
            Show
        )?;
        let column = (2 + UnicodeWidthStr::width(query.as_str())).min(width.saturating_sub(1));
        queue!(
            stdout,
            MoveTo(
                u16::try_from(column).unwrap_or(u16::MAX),
                u16::try_from(height - 1).unwrap_or(0)
            )
        )?;
    }
    Ok(())
}

fn draw_normal_footer(
    stdout: &mut Stdout,
    status: &str,
    width: usize,
    height: usize,
) -> io::Result<()> {
    if height >= 2 {
        queue!(stdout, MoveTo(0, u16::try_from(height - 2).unwrap_or(0)))?;
        if status.is_empty() {
            queue!(
                stdout,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                Print(" NORMAL "),
                SetAttribute(Attribute::Reset),
                ResetColor
            )?;
        } else {
            queue!(
                stdout,
                SetForegroundColor(Color::Yellow),
                Print(fit(status, width)),
                ResetColor
            )?;
        }
    }
    if height >= 1 {
        let help = "j/k move  h/l tree  Enter zoom  i/a edit  o/O new  / search  : commands";
        queue!(stdout, MoveTo(0, u16::try_from(height - 1).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(fit(help, width)),
            ResetColor
        )?;
    }
    Ok(())
}

fn draw_search_footer(
    stdout: &mut Stdout,
    search: &Search,
    status: &str,
    width: usize,
    height: usize,
) -> io::Result<()> {
    if height >= 2 {
        let label = if status.is_empty() {
            match search.kind {
                PromptKind::Search => "SEARCH  ↑/↓ select · Enter open · Esc cancel",
                PromptKind::Commands => "COMMANDS  ↑/↓ select · Enter run · Esc cancel",
            }
        } else {
            status
        };
        queue!(stdout, MoveTo(0, u16::try_from(height - 2).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(fit(label, width)),
            ResetColor
        )?;
    }
    if height >= 1 {
        let input_width = width.saturating_sub(2);
        let (view, cursor_column) = editor_view(&search.text, search.cursor, input_width);
        queue!(stdout, MoveTo(0, u16::try_from(height - 1).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Cyan),
            Print(match search.kind {
                PromptKind::Search => "/ ",
                PromptKind::Commands => ": ",
            }),
            ResetColor
        )?;
        queue!(stdout, Print(fit(&view, input_width)), Show)?;
        let column = (2 + cursor_column).min(width.saturating_sub(1));
        queue!(
            stdout,
            MoveTo(
                u16::try_from(column).unwrap_or(u16::MAX),
                u16::try_from(height - 1).unwrap_or(0)
            )
        )?;
    }
    Ok(())
}

fn draw_editor_status(
    stdout: &mut Stdout,
    status: &str,
    width: usize,
    height: usize,
) -> io::Result<()> {
    if height >= 2 {
        let label = if status.is_empty() {
            " INSERT  Enter sibling · Tab/Shift-Tab depth · Ctrl-Enter zoom · Alt-Bksp word"
        } else {
            status
        };
        queue!(stdout, MoveTo(0, u16::try_from(height - 2).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(fit(label, width)),
            ResetColor
        )?;
    }
    if height >= 1 {
        queue!(stdout, MoveTo(0, u16::try_from(height - 1).unwrap_or(0)))?;
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(fit(
                "arrows move · Alt-←/→ words · Home/End line · # tag · [[ ref · Esc normal",
                width
            )),
            ResetColor
        )?;
    }
    Ok(())
}

pub(super) fn display_lines(app: &App, width: usize) -> Vec<DisplayLine> {
    let visible = app.visible_nodes();
    let draft = app.editor.as_ref().and_then(|editor| match editor.target {
        EditTarget::New {
            parent_id,
            placement,
        } => draft_position(&visible, app.focus, parent_id, placement)
            .map(|(index, guides)| (index, guides, editor)),
        EditTarget::Existing(_) => None,
    });
    let mut lines = Vec::new();
    for index in 0..=visible.len() {
        if let Some((draft_index, guides, editor)) = &draft
            && *draft_index == index
        {
            lines.extend(editor_lines(editor, guides, width));
        }
        let Some(item) = visible.get(index) else {
            continue;
        };
        let selected = app.selected == Some(item.node.id);
        let selector = if selected { "› " } else { "  " };
        let indent = guide_prefix(&item.guides);
        let marker = if item.node.has_children {
            if app.expanded.contains(&item.node.id) {
                "▾"
            } else {
                "▸"
            }
        } else {
            "•"
        };
        let prefix = format!("{selector}{indent}{marker} ");
        let continuation = format!("  {indent}  ");
        let available = width
            .saturating_sub(UnicodeWidthStr::width(prefix.as_str()))
            .max(1);
        let editing = app.editor.as_ref().filter(
            |editor| matches!(editor.target, EditTarget::Existing(id) if id == item.node.id),
        );
        if let Some(editor) = editing {
            let text = editor_text(editor);
            for (line_index, (content, cursor)) in wrap_editor_text(&text, available, editor.cursor)
                .into_iter()
                .enumerate()
            {
                let line_prefix = if line_index == 0 {
                    &prefix
                } else {
                    &continuation
                };
                lines.push(DisplayLine {
                    selected: true,
                    cursor: cursor
                        .map(|column| UnicodeWidthStr::width(line_prefix.as_str()) + column),
                    text: format!("{line_prefix}{content}"),
                    content_start: line_prefix.len(),
                });
            }
        } else {
            let tags = item
                .node
                .tags
                .iter()
                .map(|tag| format!("#{tag}"))
                .collect::<Vec<_>>()
                .join(" ");
            let text = if tags.is_empty() {
                item.node.text.replace('\n', " ↵ ")
            } else {
                format!("{}  {tags}", item.node.text.replace('\n', " ↵ "))
            };
            for (line_index, content) in wrap_text(&text, available).into_iter().enumerate() {
                lines.push(DisplayLine {
                    selected,
                    cursor: None,
                    content_start: if line_index == 0 {
                        prefix.len()
                    } else {
                        continuation.len()
                    },
                    text: format!(
                        "{}{}",
                        if line_index == 0 {
                            &prefix
                        } else {
                            &continuation
                        },
                        content
                    ),
                });
            }
        }
    }
    if lines.is_empty() {
        lines.push(DisplayLine {
            selected: false,
            text: "  No bullets yet".into(),
            cursor: None,
            content_start: 2,
        });
        lines.push(DisplayLine {
            selected: false,
            text: "  Press o to start writing".into(),
            cursor: None,
            content_start: 2,
        });
    }
    lines
}

fn draft_position(
    visible: &[VisibleNode],
    focus: Option<NodeId>,
    parent_id: Option<NodeId>,
    placement: Placement,
) -> Option<(usize, Vec<bool>)> {
    match placement {
        Placement::Before(reference) => {
            let index = visible.iter().position(|item| item.node.id == reference)?;
            Some((index, visible[index].guides.clone()))
        }
        Placement::After(reference) => {
            let index = visible.iter().position(|item| item.node.id == reference)?;
            let depth = visible[index].depth;
            let mut insertion = index + 1;
            while insertion < visible.len() && visible[insertion].depth > depth {
                insertion += 1;
            }
            Some((insertion, visible[index].guides.clone()))
        }
        Placement::First if parent_id == focus => Some((0, Vec::new())),
        Placement::First => {
            let parent = parent_id?;
            let index = visible.iter().position(|item| item.node.id == parent)?;
            let mut guides = visible[index].guides.clone();
            guides.push(visible[index].has_following);
            Some((index + 1, guides))
        }
        Placement::Last if parent_id == focus => Some((visible.len(), Vec::new())),
        Placement::Last => {
            let parent = parent_id?;
            let index = visible.iter().position(|item| item.node.id == parent)?;
            let depth = visible[index].depth;
            let mut insertion = index + 1;
            while insertion < visible.len() && visible[insertion].depth > depth {
                insertion += 1;
            }
            let mut guides = visible[index].guides.clone();
            guides.push(visible[index].has_following);
            Some((insertion, guides))
        }
    }
}

fn editor_lines(editor: &Editor, guides: &[bool], width: usize) -> Vec<DisplayLine> {
    let indent = guide_prefix(guides);
    let prefix = format!("› {indent}• ");
    let continuation = format!("  {indent}  ");
    let available = width
        .saturating_sub(UnicodeWidthStr::width(prefix.as_str()))
        .max(1);
    wrap_editor_text(&editor_text(editor), available, editor.cursor)
        .into_iter()
        .enumerate()
        .map(|(index, (content, cursor))| {
            let line_prefix = if index == 0 { &prefix } else { &continuation };
            DisplayLine {
                selected: true,
                cursor: cursor.map(|column| UnicodeWidthStr::width(line_prefix.as_str()) + column),
                text: format!("{line_prefix}{content}"),
                content_start: line_prefix.len(),
            }
        })
        .collect()
}

fn guide_prefix(guides: &[bool]) -> String {
    guides
        .iter()
        .map(|visible| if *visible { "│ " } else { "  " })
        .collect()
}

fn editor_text(editor: &Editor) -> String {
    if editor.tags.is_empty() {
        return editor.text.clone();
    }
    let tags = editor
        .tags
        .iter()
        .map(|tag| format!("#{tag}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{}  {tags}", editor.text)
}

fn wrap_editor_text(text: &str, width: usize, cursor: usize) -> Vec<(String, Option<usize>)> {
    let characters: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_width = 0;
    let mut line_cursor = None;

    for (index, character) in characters.iter().enumerate() {
        if index == cursor && line_width == width && !line.is_empty() {
            lines.push((line, line_cursor));
            line = String::new();
            line_width = 0;
            line_cursor = None;
        }
        if index == cursor {
            line_cursor = Some(line_width);
        }
        let shown = if character.is_control() {
            '↵'
        } else {
            *character
        };
        let character_width = UnicodeWidthChar::width(shown).unwrap_or(0);
        if !line.is_empty() && line_width + character_width > width {
            lines.push((line, line_cursor));
            line = String::new();
            line_width = 0;
            line_cursor = if index == cursor { Some(0) } else { None };
        }
        line.push(shown);
        line_width += character_width;
    }
    if cursor == characters.len() {
        if line_width == width && !line.is_empty() {
            lines.push((line, line_cursor));
            line = String::new();
            line_cursor = Some(0);
        } else {
            line_cursor = Some(line_width);
        }
    }
    lines.push((line, line_cursor));
    lines
}

fn search_lines(search: &Search, width: usize) -> Vec<DisplayLine> {
    if search.items.is_empty() {
        return vec![DisplayLine {
            selected: false,
            cursor: None,
            content_start: 2,
            text: if search.kind == PromptKind::Search && search.text.trim().chars().count() < 2 {
                "  Type at least two characters".into()
            } else {
                "  No results".into()
            },
        }];
    }
    search
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let selected = index == search.selected;
            let text = match item {
                LauncherItem::Command(entry) => {
                    format!(":{}  — {}", entry.name, entry.hint)
                }
                LauncherItem::Node(node) => {
                    let tags = node
                        .tags
                        .iter()
                        .map(|tag| format!("#{tag}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if tags.is_empty() {
                        format!("• {}", node.text.replace('\n', " ↵ "))
                    } else {
                        format!("• {}  {tags}", node.text.replace('\n', " ↵ "))
                    }
                }
            };
            DisplayLine {
                selected,
                cursor: None,
                content_start: if selected { "› ".len() } else { 2 },
                text: fit(
                    &format!("{}{text}", if selected { "› " } else { "  " }),
                    width,
                ),
            }
        })
        .collect()
}

fn backlink_lines(view: &BacklinkView, width: usize) -> Vec<DisplayLine> {
    if view.contexts.is_empty() {
        return vec![DisplayLine {
            selected: false,
            text: "  No backlinks".into(),
            cursor: None,
            content_start: 2,
        }];
    }
    view.contexts
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let selected = index == view.selected;
            let context = path
                .iter()
                .map(|node| node.text.replace('\n', " "))
                .collect::<Vec<_>>()
                .join(" › ");
            DisplayLine {
                selected,
                text: fit(
                    &format!("{}{}", if selected { "› " } else { "  " }, context),
                    width,
                ),
                cursor: None,
                content_start: if selected { "› ".len() } else { 2 },
            }
        })
        .collect()
}

pub(super) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_width = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if !line.is_empty() && line_width + character_width > width {
            lines.push(line);
            line = String::new();
            line_width = 0;
        }
        line.push(character);
        line_width += character_width;
    }
    lines.push(line);
    lines
}

fn fit(text: &str, width: usize) -> String {
    let mut fitted = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        fitted.push(character);
        used += character_width;
    }
    fitted
}

fn editor_view(text: &str, cursor: usize, width: usize) -> (String, usize) {
    let characters: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < cursor
        && display_width(&characters[start..cursor]) >= width.saturating_sub(1).max(1)
    {
        start += 1;
    }
    let cursor_column = display_width(&characters[start..cursor]);
    let mut view = String::new();
    let mut used = 0;
    for character in &characters[start..] {
        let shown = if character.is_control() {
            '↵'
        } else {
            *character
        };
        let character_width = UnicodeWidthChar::width(shown).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        view.push(shown);
        used += character_width;
    }
    (view, cursor_column)
}

fn display_width(characters: &[char]) -> usize {
    characters
        .iter()
        .map(|character| {
            UnicodeWidthChar::width(if character.is_control() {
                '↵'
            } else {
                *character
            })
            .unwrap_or(0)
        })
        .sum()
}
