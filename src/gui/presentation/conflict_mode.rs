use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::config::Theme;
use crate::git::merge_conflict::ResolveChoice;
use crate::gui::modes::conflict_mode::{ConflictBlockState, ConflictModeState};

pub fn render(frame: &mut Frame, state: &mut ConflictModeState, theme: &Theme) {
    let area = frame.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    render_main(frame, outer[0], state, theme);
    render_status(frame, outer[1], state, theme);
}

fn render_main(frame: &mut Frame, area: Rect, state: &mut ConflictModeState, theme: &Theme) {
    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "Merge Conflict Resolver",
                Style::default()
                    .fg(theme.text_strong)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ~ ", Style::default().fg(theme.text_dimmed)),
            Span::styled(&state.path, Style::default().fg(theme.accent_secondary)),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_style(theme.active_border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 20 || inner.height < 4 {
        frame.render_widget(Paragraph::new("terminal too small"), inner);
        return;
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((inner.width / 4).clamp(18, 32)),
            Constraint::Min(20),
        ])
        .split(inner);
    render_block_list(frame, columns[0], state, theme);
    render_three_way(frame, columns[1], state, theme);
}

fn render_block_list(frame: &mut Frame, area: Rect, state: &mut ConflictModeState, theme: &Theme) {
    let list_block = Block::default()
        .title(" Conflicts ")
        .borders(Borders::RIGHT)
        .border_style(theme.inactive_border);
    let inner = list_block.inner(area);
    frame.render_widget(list_block, area);

    state.visible_height = inner.height as usize;
    state.ensure_visible(state.visible_height);
    let items: Vec<ListItem> = state
        .blocks
        .iter()
        .enumerate()
        .skip(state.scroll)
        .take(state.visible_height.max(1))
        .map(|(idx, block)| {
            let selected = idx == state.selected;
            let badge = choice_badge(block.choice);
            let prefix = if selected { "▶" } else { " " };
            let style = if selected {
                Style::default().fg(theme.text_strong).bg(theme.selected_bg)
            } else {
                Style::default().fg(theme.text)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{prefix} "), style),
                Span::styled(format!("{badge} "), badge_style(block.choice, theme)),
                Span::styled(format!("Block {}", idx + 1), style),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items), inner);
}

fn render_three_way(frame: &mut Frame, area: Rect, state: &ConflictModeState, theme: &Theme) {
    let Some(block) = state.blocks.get(state.selected) else {
        frame.render_widget(Paragraph::new("No conflict blocks"), area);
        return;
    };

    let top = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    let header = Line::from(vec![
        Span::styled(
            format!(" Block {} / {} ", state.selected + 1, state.blocks.len()),
            Style::default().fg(theme.accent_secondary),
        ),
        Span::styled(
            format!("Result: {}  ", choice_label(block.choice)),
            Style::default().fg(theme.text_strong),
        ),
        Span::styled("Ours ", Style::default().fg(Color::Green)),
        Span::styled("──▶ ", Style::default().fg(theme.text_dimmed)),
        Span::styled("Result", Style::default().fg(theme.accent_secondary)),
        Span::styled(" ◀── ", Style::default().fg(theme.text_dimmed)),
        Span::styled("Theirs", Style::default().fg(Color::Blue)),
    ]);
    frame.render_widget(Paragraph::new(header), top[0]);

    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(top[1]);
    let triptych = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(body[0]);

    render_text_panel(
        frame,
        triptych[0],
        "→ Ours / Current  [o]",
        source_panel_lines(block, &block.block.ours, "→ ", Color::Green, theme),
        theme,
        block.choice == Some(ResolveChoice::Ours),
    );
    render_text_panel(
        frame,
        triptych[1],
        "Result Preview",
        result_panel_lines(block, theme),
        theme,
        block.choice.is_some(),
    );
    render_text_panel(
        frame,
        triptych[2],
        "← Theirs / Incoming  [t]",
        source_panel_lines(block, &block.block.theirs, "← ", Color::Blue, theme),
        theme,
        block.choice == Some(ResolveChoice::Theirs),
    );

    let lower = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body[1]);
    render_text_panel(
        frame,
        lower[0],
        "Base",
        source_panel_lines(
            block,
            block.block.base.as_deref().unwrap_or(""),
            "• ",
            theme.text_dimmed,
            theme,
        ),
        theme,
        false,
    );
    render_text_panel(
        frame,
        lower[1],
        "⇄ Both Preview  [b]",
        both_panel_lines(block, theme),
        theme,
        block.choice == Some(ResolveChoice::Both),
    );
}

fn render_text_panel(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    theme: &Theme,
    highlighted: bool,
) {
    let border_style = if highlighted {
        Style::default().fg(theme.accent)
    } else {
        theme.inactive_border
    };
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(border_style);
    let content = if lines.is_empty() {
        vec![Line::from(Span::styled(
            "∅",
            Style::default().fg(theme.text_dimmed),
        ))]
    } else {
        lines
    };
    frame.render_widget(
        Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_status(frame: &mut Frame, area: Rect, state: &ConflictModeState, theme: &Theme) {
    let unresolved = state.unresolved_count();
    let line = Line::from(vec![
        Span::styled(" j/k", Style::default().fg(theme.accent_secondary)),
        Span::styled(": block  ", Style::default().fg(theme.text_dimmed)),
        Span::styled("o", Style::default().fg(theme.accent_secondary)),
        Span::styled(": ours  ", Style::default().fg(theme.text_dimmed)),
        Span::styled("t", Style::default().fg(theme.accent_secondary)),
        Span::styled(": theirs  ", Style::default().fg(theme.text_dimmed)),
        Span::styled("b", Style::default().fg(theme.accent_secondary)),
        Span::styled(": both  ", Style::default().fg(theme.text_dimmed)),
        Span::styled("n/p", Style::default().fg(theme.accent_secondary)),
        Span::styled(": unresolved  ", Style::default().fg(theme.text_dimmed)),
        Span::styled("s/enter", Style::default().fg(theme.accent_secondary)),
        Span::styled(": save+stage  ", Style::default().fg(theme.text_dimmed)),
        Span::styled("esc/q", Style::default().fg(theme.accent_secondary)),
        Span::styled(": cancel  ", Style::default().fg(theme.text_dimmed)),
        Span::styled(
            format!("unresolved: {unresolved}"),
            Style::default().fg(if unresolved == 0 {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn choice_badge(choice: Option<ResolveChoice>) -> &'static str {
    match choice {
        Some(ResolveChoice::Ours) => "O",
        Some(ResolveChoice::Theirs) => "T",
        Some(ResolveChoice::Both) => "B",
        None => "?",
    }
}

fn choice_label(choice: Option<ResolveChoice>) -> &'static str {
    match choice {
        Some(ResolveChoice::Ours) => "ours",
        Some(ResolveChoice::Theirs) => "theirs",
        Some(ResolveChoice::Both) => "both",
        None => "unresolved",
    }
}

fn badge_style(choice: Option<ResolveChoice>, theme: &Theme) -> Style {
    match choice {
        Some(ResolveChoice::Ours) => Style::default().fg(Color::Green),
        Some(ResolveChoice::Theirs) => Style::default().fg(Color::Blue),
        Some(ResolveChoice::Both) => Style::default().fg(theme.accent_secondary),
        None => Style::default().fg(Color::Yellow),
    }
}

fn result_panel_lines(block: &ConflictBlockState, theme: &Theme) -> Vec<Line<'static>> {
    match block.choice {
        Some(ResolveChoice::Ours) => {
            source_panel_lines(block, &block.block.ours, "→ ", Color::Green, theme)
        }
        Some(ResolveChoice::Theirs) => {
            source_panel_lines(block, &block.block.theirs, "← ", Color::Blue, theme)
        }
        Some(ResolveChoice::Both) => both_panel_lines(block, theme),
        None => source_panel_lines(
            block,
            "Choose ours (o), theirs (t), or both (b).\n",
            "  ",
            Color::Yellow,
            theme,
        ),
    }
}

fn both_panel_lines(block: &ConflictBlockState, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = context_lines(&block.block.context_before, theme);
    push_body_lines(
        &mut lines,
        &block.block.ours,
        "→ ",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    push_body_lines(
        &mut lines,
        &block.block.theirs,
        "← ",
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
    );
    lines.extend(context_lines(&block.block.context_after, theme));
    lines
}

fn source_panel_lines(
    block: &ConflictBlockState,
    body: &str,
    marker: &str,
    color: Color,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = context_lines(&block.block.context_before, theme);
    push_body_lines(
        &mut lines,
        body,
        marker,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );
    lines.extend(context_lines(&block.block.context_after, theme));
    lines
}

fn context_lines(text: &str, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    push_body_lines(
        &mut lines,
        text,
        "  ",
        Style::default().fg(theme.text_dimmed),
    );
    lines
}

fn push_body_lines(lines: &mut Vec<Line<'static>>, text: &str, marker: &str, style: Style) {
    for line in display_lines(text) {
        lines.push(Line::from(Span::styled(format!("{marker}{line}"), style)));
    }
}

fn display_lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n')
        .map(|line| line.trim_end_matches(['\r', '\n']).to_string())
        .collect()
}

fn result_preview(block: &ConflictBlockState) -> String {
    let body = match block.choice {
        Some(ResolveChoice::Ours) => block.block.ours.clone(),
        Some(ResolveChoice::Theirs) => block.block.theirs.clone(),
        Some(ResolveChoice::Both) => format!("{}{}", block.block.ours, block.block.theirs),
        None => "Choose ours (o), theirs (t), or both (b).\n".to_string(),
    };
    text_with_context(block, &body)
}

fn text_with_context(block: &ConflictBlockState, body: &str) -> String {
    format!(
        "{}{}{}",
        block.block.context_before, body, block.block.context_after
    )
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::config::Theme;
    use crate::git::merge_conflict::TextConflictBlock;

    #[test]
    fn result_panel_lines_dim_context_and_color_actual_changes() {
        let block = ConflictBlockState {
            block: TextConflictBlock {
                index: 0,
                context_before: "before\n".to_string(),
                base: Some("base\n".to_string()),
                ours: "ours\n".to_string(),
                theirs: "theirs\n".to_string(),
                context_after: "after\n".to_string(),
            },
            choice: Some(ResolveChoice::Both),
        };

        let lines = result_panel_lines(&block, &Theme::default());

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].spans[0].content, "  before");
        assert_eq!(lines[1].spans[0].content, "→ ours");
        assert_eq!(lines[2].spans[0].content, "← theirs");
        assert_eq!(lines[3].spans[0].content, "  after");
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(Theme::default().text_dimmed)
        );
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Green));
        assert_eq!(lines[2].spans[0].style.fg, Some(Color::Blue));
    }

    #[test]
    fn result_preview_includes_context_around_selected_change() {
        let block = ConflictBlockState {
            block: TextConflictBlock {
                index: 0,
                context_before: "before\n".to_string(),
                base: Some("base\n".to_string()),
                ours: "ours\n".to_string(),
                theirs: "theirs\n".to_string(),
                context_after: "after\n".to_string(),
            },
            choice: Some(ResolveChoice::Both),
        };

        assert_eq!(result_preview(&block), "before\nours\ntheirs\nafter\n");
    }

    #[test]
    fn conflict_mode_render_does_not_panic_on_compact_terminal() {
        let mut state = ConflictModeState::new();
        state.enter(
            "file.txt".to_string(),
            vec![TextConflictBlock {
                index: 0,
                context_before: "before\n".to_string(),
                base: Some("base\n".to_string()),
                ours: "ours\n".to_string(),
                theirs: "theirs\n".to_string(),
                context_after: "after\n".to_string(),
            }],
        );
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render(frame, &mut state, &Theme::default()))
            .unwrap();
    }
}
