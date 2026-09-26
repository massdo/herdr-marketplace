//! The little HTML a README carries, read as tags and text. Nothing is run:
//! comments and unknown tags disappear, entities become characters.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Text(String),
    /// A start tag, lowercase, and its raw attributes.
    Open {
        name: String,
        attributes: String,
    },
    Close {
        name: String,
    },
}

pub fn tokens(html: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        if open > 0 {
            tokens.push(Token::Text(decode_entities(&rest[..open])));
        }
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        let Some(close) = rest.find('>') else {
            // An unclosed `<` is text.
            tokens.push(Token::Text(decode_entities(rest)));
            return tokens;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        let (closing, tag) = match tag.strip_prefix('/') {
            Some(tag) => (true, tag),
            None => (false, tag),
        };
        let name: String = tag
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        if closing {
            tokens.push(Token::Close { name });
        } else {
            let attributes = tag[name.len()..].trim_end_matches('/').to_string();
            tokens.push(Token::Open { name, attributes });
        }
    }
    if !rest.is_empty() {
        tokens.push(Token::Text(decode_entities(rest)));
    }
    tokens
}

/// Value of `name="…"`, `name='…'` or `name=…` among raw attributes.
pub fn attribute(attributes: &str, name: &str) -> Option<String> {
    let lower = attributes.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let start = from + found;
        from = start + name.len();
        let before = lower[..start].chars().last();
        if !before.is_none_or(char::is_whitespace) {
            continue;
        }
        let Some(rest) = attributes[from..].trim_start().strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => rest[1..].split(quote).next().unwrap_or(""),
            _ => rest.split_whitespace().next().unwrap_or(""),
        };
        return Some(decode_entities(value));
    }
    None
}

/// Named entities a README uses, and numeric ones.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(';').filter(|&end| end <= 10);
        let decoded = end.and_then(|end| entity(&rest[1..end]));
        match (end, decoded) {
            (Some(end), Some(ch)) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        // Control characters stay out, whatever their spelling.
        return char::from_u32(code).filter(|ch| !ch.is_control());
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "middot" => '·',
        "bull" => '•',
        "rarr" => '→',
        "larr" => '←',
        "times" => '×',
        _ => return None,
    })
}
