use std::io::{self, Stdout, Write};
use std::path::Path;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use vrac::{NodeId, Placement};

use super::{
    App, BacklinkView, EditTarget, Editor, ReferencePrompt, Search, TagPrompt, VisibleNode,
};

#[derive(Clone)]
pub(super) struct DisplayLine {
    pub(super) selected: bool,
    pub(super) text: String,
    pub(super) cursor: Option<usize>,
}

pub(super) fn draw(stdout: &mut Stdout, app: &mut App, path: &Path) -> io::Result<()> {
    let (width, height) = terminal::size()?;
    let width = usize::from(width);
    let height = usize::from(height);
    let body_height = height.saturating_sub(4);
    let lines = match (&app.backlinks, &app.tag_prompt, &app.search) {
        (Some(view), _, _) => backlink_lines(view, width),
        (None, Some(prompt), _) => tag_lines(prompt, width),
        (None, None, Some(search)) => search_lines(search, width),
        (None, None, None) => display_lines(app, width),
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

    queue!(stdout, Hide, MoveTo(0, 0), Clear(ClearType::All))?;
    queue!(
        stdout,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        Print(fit(
            &format!(
                "Vrac TUI  {}",
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
        queue!(stdout, Print(fit(&app.focus_label(), width)), ResetColor)?;
    }

    let mut inline_cursor = None;
    for (offset, line) in lines.iter().skip(app.scroll).take(body_height).enumerate() {
        let row = offset + 2;
        queue!(stdout, MoveTo(0, u16::try_from(row).unwrap_or(u16::MAX)))?;
        if line.selected {
            queue!(
                stdout,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold)
            )?;
        }
        queue!(stdout, Print(fit(&line.text, width)))?;
        if line.selected {
            queue!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
        }
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
    stdout.flush()
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
            || format!("Create [[{}]] on save", prompt.query),
            |node| format!("[[{}]]", node.text.replace('\n', " ")),
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
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(fit(status, width)),
            ResetColor
        )?;
    }
    if height >= 1 {
        let help = "j/k h/l  Enter/-  / search  # tag  b backlinks  i/o/c  Tab  yy/dd/p  u/^R  q";
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
            "SEARCH  ↑/↓ select · Enter open · Esc cancel"
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
            Print("/ "),
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
            "EDIT  Enter save · Esc cancel"
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
                "←/→ move caret  Home/End  Enter save  Esc cancel",
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
            .map(|(index, depth)| (index, depth, editor)),
        EditTarget::Existing(_) => None,
    });
    let mut lines = Vec::new();
    for index in 0..=visible.len() {
        if let Some((draft_index, depth, editor)) = draft
            && draft_index == index
        {
            lines.extend(editor_lines(editor, depth, width));
        }
        let Some(item) = visible.get(index) else {
            continue;
        };
        let selected = app.selected == Some(item.node.id);
        let selector = if selected { "› " } else { "  " };
        let indent = "  ".repeat(item.depth);
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
        let continuation = " ".repeat(UnicodeWidthStr::width(prefix.as_str()));
        let available = width
            .saturating_sub(UnicodeWidthStr::width(prefix.as_str()))
            .max(1);
        let editing = app.editor.as_ref().filter(
            |editor| matches!(editor.target, EditTarget::Existing(id) if id == item.node.id),
        );
        if let Some(editor) = editing {
            for (line_index, (content, cursor)) in
                wrap_editor_text(&editor.text, available, editor.cursor)
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
    lines
}

fn draft_position(
    visible: &[VisibleNode],
    focus: Option<NodeId>,
    parent_id: Option<NodeId>,
    placement: Placement,
) -> Option<(usize, usize)> {
    match placement {
        Placement::After(reference) => {
            let index = visible.iter().position(|item| item.node.id == reference)?;
            let depth = visible[index].depth;
            let mut insertion = index + 1;
            while insertion < visible.len() && visible[insertion].depth > depth {
                insertion += 1;
            }
            Some((insertion, depth))
        }
        Placement::Last if parent_id == focus => Some((visible.len(), 0)),
        Placement::Last => {
            let parent = parent_id?;
            let index = visible.iter().position(|item| item.node.id == parent)?;
            let depth = visible[index].depth;
            let mut insertion = index + 1;
            while insertion < visible.len() && visible[insertion].depth > depth {
                insertion += 1;
            }
            Some((insertion, depth + 1))
        }
        Placement::First | Placement::Before(_) => None,
    }
}

fn editor_lines(editor: &Editor, depth: usize, width: usize) -> Vec<DisplayLine> {
    let prefix = format!("› {}• ", "  ".repeat(depth));
    let continuation = " ".repeat(UnicodeWidthStr::width(prefix.as_str()));
    let available = width
        .saturating_sub(UnicodeWidthStr::width(prefix.as_str()))
        .max(1);
    wrap_editor_text(&editor.text, available, editor.cursor)
        .into_iter()
        .enumerate()
        .map(|(index, (content, cursor))| {
            let line_prefix = if index == 0 { &prefix } else { &continuation };
            DisplayLine {
                selected: true,
                cursor: cursor.map(|column| UnicodeWidthStr::width(line_prefix.as_str()) + column),
                text: format!("{line_prefix}{content}"),
            }
        })
        .collect()
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
    if search.results.is_empty() {
        return vec![DisplayLine {
            selected: false,
            cursor: None,
            text: if search.text.trim().chars().count() < 2 {
                "  Type at least two characters".into()
            } else {
                "  No results".into()
            },
        }];
    }
    search
        .results
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let selected = index == search.selected;
            let tags = node
                .tags
                .iter()
                .map(|tag| format!("#{tag}"))
                .collect::<Vec<_>>()
                .join(" ");
            let text = if tags.is_empty() {
                node.text.replace('\n', " ↵ ")
            } else {
                format!("{}  {tags}", node.text.replace('\n', " ↵ "))
            };
            DisplayLine {
                selected,
                cursor: None,
                text: fit(
                    &format!("{}• {text}", if selected { "› " } else { "  " }),
                    width,
                ),
            }
        })
        .collect()
}

fn tag_lines(prompt: &TagPrompt, width: usize) -> Vec<DisplayLine> {
    if prompt.results.is_empty() {
        return vec![DisplayLine {
            selected: false,
            text: "  Type a tag".into(),
            cursor: None,
        }];
    }
    prompt
        .results
        .iter()
        .enumerate()
        .map(|(index, tag)| {
            let selected = index == prompt.selected;
            DisplayLine {
                selected,
                text: fit(
                    &format!("{}#{}", if selected { "› " } else { "  " }, tag),
                    width,
                ),
                cursor: None,
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
