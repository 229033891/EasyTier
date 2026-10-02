use tabled::settings::{Modify, Remove, Style, Width, location::ByColumnName, object::Columns};
use terminal_size::{Width as TerminalWidth, terminal_size};
use unicode_width::UnicodeWidthStr;

#[derive(clap::ValueEnum, Debug, Clone, PartialEq)]
pub enum OutputFormat {
    Table,
    Json,
}

pub fn print_output<T>(
    items: &[T],
    format: &OutputFormat,
    optional_columns: &[&str],
    drop_columns: &[&str],
    no_trunc: bool,
) -> Result<(), anyhow::Error>
where
    T: tabled::Tabled + serde::Serialize,
{
    match format {
        OutputFormat::Table => {
            let mut table = tabled::Table::new(items);
            table.with(Style::markdown());
            if no_trunc {
                println!("{}", table);
                return Ok(());
            }
            let headers = T::headers()
                .iter()
                .map(|header| header.as_ref().to_string())
                .collect::<Vec<_>>();
            let col_widths = compute_column_widths(items);
            let terminal_width = terminal_table_width();
            let drop_indices = header_indices(&headers, drop_columns);
            let optional_indices = header_indices(&headers, optional_columns);
            let (active, drop_indices, total_width) =
                select_columns_to_drop(terminal_width, &drop_indices, &col_widths);
            apply_column_drops(&mut table, &drop_indices);
            apply_optional_column_truncation(
                &mut table,
                terminal_width,
                &headers,
                &optional_indices,
                &col_widths,
                &active,
                total_width,
            );
            println!("{}", table);
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(items)?);
        }
    }
    Ok(())
}

fn terminal_table_width() -> Option<usize> {
    let (TerminalWidth(width), _) = terminal_size()?;
    let width = width as usize;
    // Avoid wrapping at the last column which can still trigger a hard line break.
    width.checked_sub(1)
}

fn apply_optional_column_truncation(
    table: &mut tabled::Table,
    terminal_width: Option<usize>,
    headers: &[String],
    optional_indices: &[usize],
    col_widths: &[usize],
    active: &[bool],
    total_width: usize,
) {
    let Some(terminal_width) = terminal_width else {
        return;
    };
    if optional_indices.is_empty() || total_width <= terminal_width {
        return;
    }

    let targets = optional_column_targets(terminal_width, optional_indices, col_widths, active);
    for (index, width) in targets {
        if let Some(name) = headers.get(index) {
            table.with(
                Modify::new(ByColumnName::new(name)).with(Width::truncate(width).suffix("...")),
            );
        }
    }
}

fn apply_column_drops(table: &mut tabled::Table, drop_indices: &[usize]) {
    let mut indices = drop_indices.to_vec();
    indices.sort_unstable_by(|a, b| b.cmp(a));
    for index in indices {
        table.with(Remove::column(Columns::one(index)));
    }
}

fn compute_column_widths<T>(items: &[T]) -> Vec<usize>
where
    T: tabled::Tabled,
{
    let mut widths = vec![0usize; T::LENGTH];
    for (idx, header) in T::headers().iter().enumerate() {
        widths[idx] = widths[idx].max(text_width(header.as_ref()));
    }
    for item in items {
        for (idx, field) in item.fields().iter().enumerate() {
            widths[idx] = widths[idx].max(text_width(field.as_ref()));
        }
    }
    widths
}

fn text_width(text: &str) -> usize {
    text.split('\n')
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
}

fn header_indices(headers: &[String], names: &[&str]) -> Vec<usize> {
    let mut indices = Vec::new();
    for name in names {
        if let Some(index) = headers
            .iter()
            .position(|header| header.eq_ignore_ascii_case(name))
            && !indices.contains(&index)
        {
            indices.push(index);
        }
    }
    indices
}

fn select_columns_to_drop(
    terminal_width: Option<usize>,
    drop_indices: &[usize],
    col_widths: &[usize],
) -> (Vec<bool>, Vec<usize>, usize) {
    let mut active = vec![true; col_widths.len()];
    let Some(terminal_width) = terminal_width else {
        let total = table_total_width(col_widths, &active);
        return (active, vec![], total);
    };

    let mut total = table_total_width(col_widths, &active);
    if total <= terminal_width {
        return (active, vec![], total);
    }

    let mut dropped = vec![];
    for &index in drop_indices {
        if total <= terminal_width {
            break;
        }
        if active[index] {
            active[index] = false;
            dropped.push(index);
            total = table_total_width(col_widths, &active);
        }
    }

    (active, dropped, total)
}

fn table_total_width(col_widths: &[usize], active: &[bool]) -> usize {
    let col_count = active.iter().filter(|value| **value).count();
    if col_count == 0 {
        return 0;
    }
    let content_width = col_widths
        .iter()
        .zip(active.iter())
        .filter_map(|(width, keep)| keep.then_some(*width))
        .sum::<usize>();
    content_width + 3 * col_count + 1
}

fn optional_column_targets(
    terminal_width: usize,
    optional_indices: &[usize],
    col_widths: &[usize],
    active: &[bool],
) -> Vec<(usize, usize)> {
    if optional_indices.is_empty() {
        return vec![];
    }

    let mut is_optional = vec![false; col_widths.len()];
    for &index in optional_indices {
        if let Some(flag) = is_optional.get_mut(index) {
            *flag = true;
        }
    }

    let optional_indices = optional_indices
        .iter()
        .copied()
        .filter(|idx| active.get(*idx).copied().unwrap_or(false))
        .collect::<Vec<_>>();
    if optional_indices.is_empty() {
        return vec![];
    }

    let col_count = active.iter().filter(|value| **value).count();
    let overhead = 3 * col_count + 1;
    let mut required_width = overhead;
    for (idx, width) in col_widths.iter().enumerate() {
        if active.get(idx).copied().unwrap_or(false) && !is_optional[idx] {
            required_width += *width;
        }
    }

    let remaining = terminal_width.saturating_sub(required_width);
    let min_width = 6usize;
    let per_column = if remaining == 0 {
        min_width
    } else {
        (remaining / optional_indices.len()).clamp(min_width, 24)
    };

    optional_indices
        .into_iter()
        .map(|idx| (idx, col_widths[idx].min(per_column)))
        .collect()
}
