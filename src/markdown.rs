//! Markdown normalizer that preserves proper formatting while normalizing spacing.

#[derive(Debug, PartialEq, Clone)]
enum BlockType {
    Header,
    ListItem { depth: usize, marker: ListMarker },
    CodeBlockStart,
    CodeBlockEnd,
    Normal,
}

#[derive(Debug, PartialEq, Clone)]
enum ListMarker {
    Number, // 1. 2. 3.
    Letter, // a. b. c.
    Bullet, // * or -
    None,
}

fn normalize_for_comparison(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_content_boundaries(markdown: &str, plain: &str) -> (usize, usize) {
    let first_plain_para = plain.split("\n\n").next().unwrap_or("").trim();
    let markdown_lines: Vec<&str> = markdown.lines().collect();
    let mut start_idx = 0;
    for (i, window) in markdown_lines.windows(3).enumerate() {
        let combined = window.join(" ");
        if normalize_for_comparison(&combined).contains(&normalize_for_comparison(first_plain_para))
        {
            start_idx = i;
            break;
        }
    }
    let mut end_idx = markdown_lines.len();
    for (i, line) in markdown_lines.iter().enumerate().rev() {
        if i <= start_idx {
            break;
        }
        if line.contains("## Related posts")
            || line.contains("Blog Comments")
            || line.contains("Contents")
            || (line.starts_with("##") && !line.contains("Summary"))
        {
            end_idx = i;
            break;
        }
    }
    (start_idx, end_idx)
}

fn get_list_marker(line: &str) -> ListMarker {
    let trimmed = line.trim_start();
    if let Some(first_token) = trimmed.split_whitespace().next() {
        // Try to parse the first part as a number
        if first_token.chars().next().unwrap_or(' ').is_ascii_digit() {
            // Handle cases like "1", "1.", "1.1", "1.1.", "1.a", "1.a."
            return ListMarker::Number;
        }

        // Handle cases like "a", "a."
        if first_token
            .chars()
            .next()
            .unwrap_or(' ')
            .is_ascii_lowercase()
        {
            return ListMarker::Letter;
        }

        // Handle bullet points
        if first_token.starts_with(['*', '-']) {
            return ListMarker::Bullet;
        }
    }
    ListMarker::None
}

fn get_list_depth(line: &str) -> usize {
    let spaces = line.chars().take_while(|c| c.is_whitespace()).count();
    let trimmed = line.trim_start();

    // Count dots in first token
    if let Some(first_token) = trimmed.split_whitespace().next() {
        let mut found_number = false;
        let mut dots = 0;

        for c in first_token.chars() {
            if c == '`' || c == '[' || c == '<' || c == '\'' || c == '\"' || c == '(' {
                break;
            }
            if c.is_ascii_digit() {
                found_number = true;
            } else if c == '.' && found_number {
                dots += 1;
                found_number = false;
            }
        }

        if dots > 0 {
            if found_number {
                // dot wasn't trailing one
                return dots;
            } else {
                return dots - 1;
            }
        }

        // Return indentation level if no dots found
        return spaces / 4;
    }

    0
}

/// Modified to accept an extra flag indicating if we’re in an active list.
/// If so, and if the marker is composite (e.g. "2.1"), we use the composite indent.
fn get_block_type(line: &str, is_in_code_block: bool, in_list: bool) -> BlockType {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return BlockType::Normal;
    }
    if trimmed.starts_with('#') {
        return BlockType::Header;
    }
    let marker = get_list_marker(trimmed);
    if marker != ListMarker::None {
        let depth = get_list_depth(line);
        return BlockType::ListItem { depth, marker };
    } else if trimmed.starts_with("```") {
        return if is_in_code_block {
            BlockType::CodeBlockEnd
        } else {
            BlockType::CodeBlockStart
        };
    }
    BlockType::Normal
}

fn indent_line(line: &str, depth: usize) -> String {
    let spaces = "    ".repeat(depth);
    let trimmed = line.trim_start();
    format!("{}{}", spaces, trimmed)
}

// Simplified normalization that indents using the provided depth.
fn normalize_list_item(line: &str, depth: usize) -> String {
    let trimmed = line.trim_start();
    if depth > 0 {
        indent_line(trimmed, depth)
    } else {
        trimmed.to_string()
    }
}

fn is_list_continuation(line: &str, prev_block_type: &BlockType) -> bool {
    match prev_block_type {
        BlockType::ListItem { depth, .. } => {
            let line_depth = get_list_depth(line);
            let trimmed = line.trim_start();
            if get_list_marker(trimmed) != ListMarker::None {
                return false;
            }
            line_depth > *depth || (line_depth == *depth && !trimmed.starts_with('#'))
        }
        _ => false,
    }
}

fn needs_spacing_before(block_type: &BlockType, prev_block_type: &BlockType) -> bool {
    match block_type {
        BlockType::Header => true,
        BlockType::ListItem { .. } => match prev_block_type {
            BlockType::ListItem { .. } => false,
            _ => true,
        },
        BlockType::CodeBlockStart => true,
        _ => false,
    }
}

fn needs_spacing_after(block_type: &BlockType, next_block_type: &BlockType) -> bool {
    match block_type {
        BlockType::Header => true,
        BlockType::ListItem { .. } => match next_block_type {
            BlockType::ListItem { .. } => false,
            _ => true,
        },
        BlockType::CodeBlockEnd => true,
        BlockType::Normal => match next_block_type {
            BlockType::Header | BlockType::ListItem { .. } => true,
            BlockType::Normal => true,
            _ => false,
        },
        _ => false,
    }
}
fn is_in_code_or_link(text: &str, pos: usize) -> bool {
    let before = &text[..pos];
    let backticks = before.matches('`').count();
    if backticks % 2 != 0 {
        return true;
    }
    let mut html_link_depth = 0;
    for c in before.chars() {
        match c {
            '<' => html_link_depth += 1,
            '>' => html_link_depth -= 1,
            _ => {}
        }
    }
    if html_link_depth > 0 {
        return true;
    }
    let mut in_brackets = 0;
    let mut in_parens = 0;
    let chars: Vec<char> = before.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '[' => in_brackets += 1,
            ']' => {
                in_brackets -= 1;
                if in_brackets == 0 && i + 1 < chars.len() && chars[i + 1] == '(' {
                    in_parens += 1;
                }
            }
            '(' => {
                if in_brackets == 0 {
                    in_parens += 1;
                }
            }
            ')' => {
                if in_brackets == 0 {
                    in_parens -= 1;
                }
            }
            _ => {}
        }
    }
    in_brackets > 0 || in_parens > 0
}

// Sometimes markdown generators incorrectly merge header with text.
fn split_header_content(line: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().enumerate();
    while let Some((pos, c)) = chars.next() {
        if c == '#' && !is_in_code_or_link(line, pos) {
            let rest: String = line[pos..].chars().take_while(|&c| c == '#').collect();
            let after_hash = pos + rest.len();
            if after_hash < line.len()
                && (line
                    .chars()
                    .nth(after_hash)
                    .map_or(false, |c| c.is_whitespace())
                    || after_hash == line.len())
            {
                if !current.trim().is_empty() {
                    result.push(current.trim().to_string());
                    current.clear();
                }
                current.push_str(&line[pos..]);
                break;
            }
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        result.push(current.trim().to_string());
    }
    result
}

pub fn normalize_markdown(markdown: &str, plain: &str) -> String {
    let markdown_lines: Vec<&str> = markdown.lines().collect();
    let (start_idx, end_idx) = find_content_boundaries(markdown, plain);
    let mut result = Vec::new();
    let mut current_block: Vec<String> = Vec::new();
    let mut in_code_block = false;
    let content_lines = &markdown_lines[start_idx..end_idx];
    let mut prev_block_type = BlockType::Normal;
    let mut in_list = false;

    for (i, &line) in content_lines.iter().enumerate() {
        let trimmed = line.trim_end();
        let split_lines = split_header_content(trimmed);
        for (j, split_line) in split_lines.iter().enumerate() {
            if split_line.is_empty() {
                continue;
            }
            if split_line.starts_with("```") {
                in_code_block = !in_code_block;
            }

            let is_continuation =
                !split_line.is_empty() && is_list_continuation(split_line, &prev_block_type);
            let current_type = if is_continuation {
                prev_block_type.clone()
            } else {
                get_block_type(split_line, in_code_block, in_list)
            };

            // Update in_list status based on current block type
            match &current_type {
                BlockType::ListItem { .. } => {
                    if !in_list {
                        in_list = true;
                        if !current_block.is_empty() {
                            result.push(current_block.join("\n"));
                            current_block.clear();
                        }
                    }
                }
                _ => {
                    if !is_continuation
                        && !matches!(current_type, BlockType::ListItem { .. })
                        && in_list
                    {
                        in_list = false;
                        if !current_block.is_empty() {
                            result.push(current_block.join("\n"));
                            current_block.clear();
                        }
                    }
                }
            }

            if j > 0 && matches!(current_type, BlockType::Header) {
                if !current_block.is_empty() {
                    result.push(current_block.join("\n"));
                    current_block.clear();
                }
            }

            let next_type = if i < content_lines.len() - 1 {
                get_block_type(content_lines[i + 1], in_code_block, in_list)
            } else {
                BlockType::Normal
            };

            if needs_spacing_before(&current_type, &prev_block_type) && !current_block.is_empty() {
                result.push(current_block.join("\n*"));
                current_block.clear();
            }

            let normalized_line = match &current_type {
                BlockType::ListItem { depth, marker } => {
                    let actual_depth = match prev_block_type {
                        BlockType::ListItem {
                            marker: prev_marker,
                            depth: prev_depth,
                            ..
                        } => match (prev_marker, marker) {
                            (ListMarker::Number, ListMarker::Letter) => prev_depth + 1,
                            (ListMarker::Letter, ListMarker::Number) => {
                                0.max(prev_depth.saturating_sub(1))
                            }
                            _ => *depth,
                        },
                        _ => 0,
                    };
                    normalize_list_item(split_line, actual_depth)
                }
                BlockType::Normal => {
                    if let BlockType::ListItem { depth, .. } = prev_block_type {
                        if is_list_continuation(split_line, &prev_block_type) {
                            indent_line(split_line, depth + 1)
                        } else {
                            split_line.to_string()
                        }
                    } else {
                        split_line.to_string()
                    }
                }
                _ => split_line.to_string(),
            };
            current_block.push(normalized_line.clone()); //todo remove clone
            if j == split_lines.len() - 1
                && needs_spacing_after(&current_type, &next_type)
                && !matches!(&next_type, BlockType::ListItem { .. } if in_list)
            {
                result.push(current_block.join("\n"));
                current_block.clear();
            }
            prev_block_type = current_type.clone();
        }
    }

    if !current_block.is_empty() {
        result.push(current_block.join("\n"));
    }

    result
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_with_paragraphs_mixing_numbers_and_chars() {
        let input = r#"Text before
1. First item with continuation
2. Second item
a. Sub item A
b. Sub item B
3. Third item
Some text after the list."#;
        let normalized = normalize_markdown(input, input);
        assert_eq!(
            normalized.trim(),
            r#"Text before

1. First item with continuation
2. Second item
    a. Sub item A
    b. Sub item B
3. Third item

Some text after the list."#
                .trim()
        );
    }

    #[test]
    fn test_list_with_paragraphs_sublists() {
        let input = r#"Text before
1. First item with continuation
2. Second item
2.1 Sub item A
2.2. Sub item B
3. Third item
Some text after the list."#;
        let normalized = normalize_markdown(input, input);
        assert_eq!(
            normalized.trim(),
            r#"Text before

1. First item with continuation
2. Second item
    2.1 Sub item A
    2.2. Sub item B
3. Third item

Some text after the list."#
                .trim()
        );
    }

    #[test]
    fn test_splitting_incorrectly_merged_titles() {
        let input = r#"### Some title

4.4.`Sender`sends all the schedules from the two regions.### Region recovery example (failover is switched off)

1. SSM regional parameters are changed. Life goes back to normal.[in link ##test](#url)"#;
        let normalized = normalize_markdown(input.trim(), input);
        assert_eq!(
            normalized.trim(),
            r#"### Some title

4.4.`Sender`sends all the schedules from the two regions.

### Region recovery example (failover is switched off)

1. SSM regional parameters are changed. Life goes back to normal.[in link ##test](#url)"#
                .trim()
        );
    }

    #[test]
    fn normalize_markdown_should_add_newlines_between_paragraphs() {
        let input = r#"
As mentioned in the beginning of this article, this is good.
It is important to emphasise that this is architecture.
                "#;
        let normalized = normalize_markdown(input.trim(), input);
        assert_eq!(
            normalized.trim(),
            r#"
As mentioned in the beginning of this article, this is good.

It is important to emphasise that this is architecture.
"#
            .trim()
        );
    }

    #[test]
    fn test_list_depth_trailing_dots() {
        assert_eq!(get_list_depth("1.`Ololoev` is the best"), 0);
        assert_eq!(get_list_depth("1. `Ololoev` is the best"), 0);
        assert_eq!(get_list_depth("1 `Ololoev` is the best"), 0);
        assert_eq!(get_list_depth("1.2.`Ololoev` is the best"), 1);
        assert_eq!(get_list_depth("1.2`Ololoev` is the best"), 1);
        assert_eq!(get_list_depth("1.2 `Ololoev` is the best"), 1);
        assert_eq!(get_list_depth("1.2.3.1`Ololoev` is the best"), 3);
    }

    #[test]
    fn test_weird_cases() {
        assert_eq!(
            get_list_depth("4.2`ReaderTaskProducers`generate the`ReaderTasks`"),
            1
        );
        assert_eq!(
            get_list_depth("4.3.`ReaderTaskConsumers`fetch buckets from`us-east-1`"),
            1
        );

        let str = r#"
4. `us-east-2`
4.2`ReaderTaskProducers`generate the`ReaderTasks`
4.3.`ReaderTaskConsumers`fetch buckets from`us-east-1`"#
            .trim();
        let normalized = normalize_markdown(str, str);
        assert_eq!(
            normalized.trim(),
            r#"
4. `us-east-2`
    4.2`ReaderTaskProducers`generate the`ReaderTasks`
    4.3.`ReaderTaskConsumers`fetch buckets from`us-east-1`"#
                .trim()
        );
    }
}
