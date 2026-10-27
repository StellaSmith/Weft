//! The engine's `key = value` text files (settings, world and player files, themes, configs, GUI
//! sidecars, animations, connected textures, translations): one reader, so all treat comments
//! alike. A line starting with `#` is a comment; a value is taken whole, `#` included (theme
//! colors, text color tags).

/// (line number, key, value) of every `key = value` line, both trimmed.
pub fn pairs(src: &str) -> impl Iterator<Item = (usize, &str, &str)> {
    src.lines().enumerate().filter_map(|(i, l)| {
        let l = l.trim_start_matches('\u{feff}').trim();
        if l.is_empty() || l.starts_with('#') {
            return None;
        }
        let (k, v) = l.split_once('=')?;
        Some((i + 1, k.trim(), v.trim()))
    })
}

/// `pairs` whose value may go on in indented lines below it, joined with line breaks
/// (translations).
pub fn long_pairs(src: &str) -> Vec<(usize, &str, String)> {
    let mut out: Vec<(usize, &str, String)> = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}');
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.2.push('\n');
                last.2.push_str(t);
            }
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.push((i + 1, k.trim(), v.trim().to_string()));
        }
    }
    out
}

/// The pairs of each `[section]` (connected texture rules), the lines before the first header
/// being a section too.
pub fn sections(src: &str) -> Vec<Vec<(usize, &str, &str)>> {
    let mut out = vec![Vec::new()];
    for (n, k, v) in pairs_and_headers(src) {
        match (k, v) {
            (_, None) => out.push(Vec::new()),
            (k, Some(v)) => out.last_mut().unwrap().push((n, k, v)),
        }
    }
    out
}

fn pairs_and_headers(src: &str) -> impl Iterator<Item = (usize, &str, Option<&str>)> {
    src.lines().enumerate().filter_map(|(i, l)| {
        let l = l.trim_start_matches('\u{feff}').trim();
        if l.starts_with('[') {
            return Some((i + 1, l, None));
        }
        if l.is_empty() || l.starts_with('#') {
            return None;
        }
        let (k, v) = l.split_once('=')?;
        Some((i + 1, k.trim(), Some(v.trim())))
    })
}

/// The value of `key`, if the text sets it.
pub fn get<'a>(src: &'a str, key: &str) -> Option<&'a str> {
    pairs(src).find(|p| p.1 == key).map(|p| p.2)
}

/// `value` as a number; a value that is not one is reported with its file and line instead of
/// quietly becoming the default.
pub fn number<T: std::str::FromStr>(file: &str, line: usize, key: &str, value: &str) -> Option<T> {
    let n = value.parse().ok();
    if n.is_none() {
        log::warn!("{file}:{line}: {key} = '{value}' is not a number");
    }
    n
}

#[cfg(test)]
mod tests {
    #[test]
    fn comments_and_values() {
        let src = "# comment\n\n a = 1 \ncolor = #ff0000\nborder = 4 # px\nnot a pair\n";
        let p: Vec<_> = super::pairs(src).collect();
        assert_eq!(p, vec![(3, "a", "1"), (4, "color", "#ff0000"), (5, "border", "4 # px")]);
        assert_eq!(super::get(src, "color"), Some("#ff0000"));
        let long = super::long_pairs("a = one\n  two\n# c\nb = x=y\n");
        assert_eq!(long, vec![(1, "a", "one\ntwo".to_string()), (4, "b", "x=y".to_string())]);
        let s = super::sections("a = 1\n[one]\nb = 2\n[two]\nc = 3\nd = 4\n");
        assert_eq!(s, vec![vec![(1, "a", "1")], vec![(3, "b", "2")], vec![(5, "c", "3"), (6, "d", "4")]]);
        assert_eq!(super::number::<f32>("t", 5, "border", "4 # px"), None);
    }
}
