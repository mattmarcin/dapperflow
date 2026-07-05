//! A small, strict, indentation-based YAML subset for recipe front matter.
//!
//! Recipe front matter (`recipes.md` / Format) is intentionally simple: scalars,
//! nested block maps, inline flow lists (`[a, b]`), and one block list of maps (the
//! `mcp:` mounts). A full YAML engine is more surface than the format needs, and its
//! generic type errors read worse than a schema-aware parser's. This module parses the
//! subset into a [`Node`] tree with line-numbered errors; the schema interpretation
//! (typed blocks, allowed enum values) lives in `recipe/mod.rs`.
//!
//! Deliberately unsupported (rejected with a clear error, never mis-parsed): anchors,
//! multi-document streams, block scalars (`|`, `>`), and tab indentation.

/// A parsed YAML-subset value.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A scalar value (quotes stripped).
    Scalar(String),
    /// A sequence.
    List(Vec<Node>),
    /// A mapping, preserving source order (validation reports first offender first).
    Map(Vec<(String, Node)>),
}

impl Node {
    /// Borrow the scalar text, if this node is a scalar.
    pub fn as_scalar(&self) -> Option<&str> {
        match self {
            Node::Scalar(s) => Some(s),
            _ => None,
        }
    }

    /// Borrow the list items, if this node is a list.
    pub fn as_list(&self) -> Option<&[Node]> {
        match self {
            Node::List(v) => Some(v),
            _ => None,
        }
    }

    /// Look up a key in a mapping (first match wins), if this node is a map.
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The kind name, for error messages ("scalar" | "list" | "map").
    pub fn kind(&self) -> &'static str {
        match self {
            Node::Scalar(_) => "scalar",
            Node::List(_) => "list",
            Node::Map(_) => "map",
        }
    }
}

/// A line-numbered YAML-subset parse error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlError {
    pub message: String,
    /// 1-based source line, or 0 when not tied to a specific line.
    pub line: usize,
}

impl std::fmt::Display for YamlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "line {}: {}", self.line, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

/// A significant source line: its indentation, its trimmed text, and its 1-based number.
#[derive(Debug, Clone)]
struct Line {
    indent: usize,
    text: String,
    no: usize,
}

/// Parse a YAML-subset document into a [`Node`]. An empty document is an empty map.
pub fn parse(text: &str) -> Result<Node, YamlError> {
    let lines = significant_lines(text)?;
    if lines.is_empty() {
        return Ok(Node::Map(Vec::new()));
    }
    let base = lines[0].indent;
    let mut cursor = 0;
    let node = parse_block(&lines, &mut cursor, base)?;
    if cursor != lines.len() {
        return Err(YamlError {
            message: "inconsistent indentation or unexpected content".to_string(),
            line: lines[cursor].no,
        });
    }
    Ok(node)
}

/// Strip blank and comment lines, reject tab indentation and block-scalar markers, and
/// carry the 1-based line number so errors point at the real source line.
fn significant_lines(text: &str) -> Result<Vec<Line>, YamlError> {
    let mut out = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let no = idx + 1;
        // Indentation must be spaces; a tab in the indent is a common, silent YAML trap.
        let indent = raw.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        if raw[..indent].contains('\t') {
            return Err(YamlError { message: "tab indentation is not allowed".to_string(), line: no });
        }
        let content = raw[indent..].trim_end();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        out.push(Line { indent, text: content.to_string(), no });
    }
    Ok(out)
}

/// Parse the block beginning at `*cursor`, whose sibling indentation is `indent`.
fn parse_block(lines: &[Line], cursor: &mut usize, indent: usize) -> Result<Node, YamlError> {
    if lines[*cursor].text.starts_with('-')
        && (lines[*cursor].text == "-" || lines[*cursor].text.starts_with("- "))
    {
        parse_list(lines, cursor, indent)
    } else {
        parse_map(lines, cursor, indent)
    }
}

/// Parse a mapping: `key: value` lines, and `key:` lines whose value is a deeper block.
fn parse_map(lines: &[Line], cursor: &mut usize, indent: usize) -> Result<Node, YamlError> {
    let mut entries: Vec<(String, Node)> = Vec::new();
    while *cursor < lines.len() && lines[*cursor].indent == indent {
        let line = lines[*cursor].clone();
        if line.text.starts_with('-') && (line.text == "-" || line.text.starts_with("- ")) {
            return Err(YamlError {
                message: "expected 'key: value', found a list item".to_string(),
                line: line.no,
            });
        }
        let (key, rest) = split_key(&line)?;
        *cursor += 1;
        let value = if rest.is_empty() {
            // A bare `key:` introduces either a deeper block or an empty (null) value.
            if *cursor < lines.len() && lines[*cursor].indent > indent {
                let child_indent = lines[*cursor].indent;
                parse_block(lines, cursor, child_indent)?
            } else {
                Node::Scalar(String::new())
            }
        } else {
            parse_scalar(&rest)
        };
        entries.push((key, value));
    }
    Ok(Node::Map(entries))
}

/// Parse a block sequence: `- item` lines, where an item is a scalar, a nested block,
/// or an inline-first map (`- key: value` with aligned continuation lines).
fn parse_list(lines: &[Line], cursor: &mut usize, indent: usize) -> Result<Node, YamlError> {
    let mut items: Vec<Node> = Vec::new();
    while *cursor < lines.len()
        && lines[*cursor].indent == indent
        && (lines[*cursor].text == "-" || lines[*cursor].text.starts_with("- "))
    {
        let line = lines[*cursor].clone();
        let rest = line.text[1..].trim_start().to_string();
        *cursor += 1;
        let has_continuation = *cursor < lines.len() && lines[*cursor].indent > indent;
        let value = if rest.is_empty() {
            // A bare `-` introduces a nested block, or an empty item.
            if has_continuation {
                let child_indent = lines[*cursor].indent;
                parse_block(lines, cursor, child_indent)?
            } else {
                Node::Scalar(String::new())
            }
        } else if is_map_entry(&rest) || has_continuation {
            // `- key: value`: the item is a map whose first entry is on the dash line and
            // whose remaining entries are the deeper-indented continuation lines. Rebuild
            // it as a sub-block aligned at the content column and parse that.
            let item_indent = indent + 2;
            let mut sub = vec![Line { indent: item_indent, text: rest, no: line.no }];
            while *cursor < lines.len() && lines[*cursor].indent > indent {
                sub.push(lines[*cursor].clone());
                *cursor += 1;
            }
            let mut sub_cursor = 0;
            let node = parse_block(&sub, &mut sub_cursor, sub[0].indent)?;
            if sub_cursor != sub.len() {
                return Err(YamlError {
                    message: "inconsistent indentation inside a list item".to_string(),
                    line: sub[sub_cursor].no,
                });
            }
            node
        } else {
            parse_scalar(&rest)
        };
        items.push(value);
    }
    Ok(Node::List(items))
}

/// Split a `key: value` line into its key and the trimmed remainder.
fn split_key(line: &Line) -> Result<(String, String), YamlError> {
    let (key, rest) = line
        .text
        .split_once(':')
        .ok_or_else(|| YamlError { message: format!("expected 'key: value', found '{}'", line.text), line: line.no })?;
    let key = key.trim();
    if key.is_empty() {
        return Err(YamlError { message: "empty mapping key".to_string(), line: line.no });
    }
    Ok((key.to_string(), rest.trim().to_string()))
}

/// Whether `s` begins a `key: value` mapping entry (a bare key followed by `: `).
fn is_map_entry(s: &str) -> bool {
    match s.split_once(':') {
        Some((k, v)) => {
            let k = k.trim();
            !k.is_empty()
                && !k.contains(char::is_whitespace)
                && (v.is_empty() || v.starts_with([' ', '\t']))
        }
        None => false,
    }
}

/// Parse a scalar value or an inline flow list (`[a, b, c]`).
fn parse_scalar(s: &str) -> Node {
    let s = s.trim();
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        let items: Vec<Node> = inner
            .split(',')
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .map(|t| Node::Scalar(unquote(t)))
            .collect();
        return Node::List(items);
    }
    Node::Scalar(unquote(s))
}

/// Strip a single layer of matching single or double quotes.
fn unquote(value: &str) -> String {
    let v = value.trim();
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        v[1..v.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_and_inline_lists() {
        let node = parse("name: standard\nversion: 1\nstages: [plan, implement, verify, ship]").unwrap();
        assert_eq!(node.get("name").unwrap().as_scalar(), Some("standard"));
        assert_eq!(node.get("version").unwrap().as_scalar(), Some("1"));
        let stages = node.get("stages").unwrap().as_list().unwrap();
        assert_eq!(stages.len(), 4);
        assert_eq!(stages[0].as_scalar(), Some("plan"));
        assert_eq!(stages[3].as_scalar(), Some("ship"));
    }

    #[test]
    fn nested_block_map() {
        let text = "plan:\n  mode: artifact\n  approval: required\n  playbooks: [plan]";
        let node = parse(text).unwrap();
        let plan = node.get("plan").unwrap();
        assert_eq!(plan.get("mode").unwrap().as_scalar(), Some("artifact"));
        assert_eq!(plan.get("approval").unwrap().as_scalar(), Some("required"));
        assert_eq!(plan.get("playbooks").unwrap().as_list().unwrap().len(), 1);
    }

    #[test]
    fn block_list_of_maps() {
        let text = "mcp:\n  - name: context7\n    command: \"npx -y @upstash/context7-mcp\"\n    stages: [plan, implement]";
        let node = parse(text).unwrap();
        let mounts = node.get("mcp").unwrap().as_list().unwrap();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].get("name").unwrap().as_scalar(), Some("context7"));
        assert_eq!(
            mounts[0].get("command").unwrap().as_scalar(),
            Some("npx -y @upstash/context7-mcp")
        );
        assert_eq!(mounts[0].get("stages").unwrap().as_list().unwrap().len(), 2);
    }

    #[test]
    fn multiple_block_list_items() {
        let text = "mcp:\n  - name: a\n    command: cmd-a\n  - name: b\n    command: cmd-b\n    stages: [implement]";
        let node = parse(text).unwrap();
        let mounts = node.get("mcp").unwrap().as_list().unwrap();
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].get("name").unwrap().as_scalar(), Some("a"));
        assert_eq!(mounts[1].get("name").unwrap().as_scalar(), Some("b"));
        assert!(mounts[0].get("stages").is_none());
        assert_eq!(mounts[1].get("stages").unwrap().as_list().unwrap().len(), 1);
    }

    #[test]
    fn comments_and_blanks_ignored() {
        let text = "# a comment\nname: presto\n\n  # indented comment\nversion: 1\n";
        let node = parse(text).unwrap();
        assert_eq!(node.get("name").unwrap().as_scalar(), Some("presto"));
        assert_eq!(node.get("version").unwrap().as_scalar(), Some("1"));
    }

    #[test]
    fn quoted_values_unquoted() {
        let node = parse("description: \"One screen, single review.\"").unwrap();
        assert_eq!(node.get("description").unwrap().as_scalar(), Some("One screen, single review."));
    }

    #[test]
    fn empty_document_is_empty_map() {
        assert_eq!(parse("").unwrap(), Node::Map(Vec::new()));
        assert_eq!(parse("\n\n# only a comment\n").unwrap(), Node::Map(Vec::new()));
    }

    #[test]
    fn tab_indentation_rejected_with_line() {
        let err = parse("plan:\n\tmode: artifact").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("tab"));
    }

    #[test]
    fn missing_colon_reports_line() {
        let err = parse("name: ok\nthis is not a mapping").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("key: value"));
    }

    #[test]
    fn inconsistent_indent_reports_line() {
        // `approval` is indented one space shallower than its sibling `mode`.
        let err = parse("plan:\n  mode: artifact\n approval: required").unwrap_err();
        assert!(err.line >= 3, "error should point near the misindented line: {err:?}");
    }
}
