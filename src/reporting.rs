pub fn print_pairs(pairs: &[(&str, String)]) {
    let rendered = pairs
        .iter()
        .map(|(key, value)| format!("{key}={}", escape_value(value)))
        .collect::<Vec<_>>()
        .join(" ");
    println!("{rendered}");
}

pub fn print_progress(message: &str) {
    eprintln!("{message}");
}

pub fn escape_value(value: &str) -> String {
    let sanitized = value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    let needs_quotes = sanitized.chars().any(|ch| ch.is_whitespace()) || sanitized.contains('"');
    if !needs_quotes {
        return sanitized;
    }
    format!("\"{}\"", sanitized.replace('"', "\\\""))
}
