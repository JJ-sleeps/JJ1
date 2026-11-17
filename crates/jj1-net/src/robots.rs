//! Minimal robots.txt parser: user-agent groups, Allow/Disallow, `*` and `$`.

#[derive(Debug, Clone, Default)]
pub struct Robots {
    /// (allow, pattern)
    rules: Vec<(bool, String)>,
}

impl Robots {
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// Parse `text`, keeping the group for `agent` if one exists, otherwise `*`.
    pub fn parse(text: &str, agent: &str) -> Self {
        let agent = agent.to_ascii_lowercase();
        let mut specific: Vec<(bool, String)> = Vec::new();
        let mut star: Vec<(bool, String)> = Vec::new();
        let mut specific_seen = false;
        let mut current: Vec<String> = Vec::new();
        let mut last_was_ua = false;

        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once(':') else { continue };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "user-agent" => {
                    if !last_was_ua {
                        current.clear();
                    }
                    let ua = value.to_ascii_lowercase();
                    if ua == agent {
                        specific_seen = true;
                    }
                    current.push(ua);
                    last_was_ua = true;
                }
                "allow" | "disallow" => {
                    last_was_ua = false;
                    if value.is_empty() {
                        continue;
                    }
                    let rule = (key == "allow", value.to_string());
                    for ua in &current {
                        if *ua == agent {
                            specific.push(rule.clone());
                        } else if ua == "*" {
                            star.push(rule.clone());
                        }
                    }
                }
                _ => last_was_ua = false,
            }
        }
        Robots { rules: if specific_seen { specific } else { star } }
    }

    /// Longest matching pattern wins; Allow wins ties.
    pub fn allowed(&self, path: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for (allow, pat) in &self.rules {
            if !pattern_matches(pat, path) {
                continue;
            }
            let len = pat.len();
            match best {
                Some((l, a)) if l > len || (l == len && a) => {}
                _ => best = Some((len, *allow)),
            }
        }
        best.map(|(_, allow)| allow).unwrap_or(true)
    }
}

fn pattern_matches(pat: &str, path: &str) -> bool {
    let (pat, anchored) = match pat.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pat, false),
    };
    let parts: Vec<&str> = pat.split('*').collect();
    let last = parts.len() - 1;
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == last && anchored {
            return path[pos..].ends_with(part);
        } else {
            match path[pos..].find(part) {
                Some(k) => pos += k + part.len(),
                None => return false,
            }
        }
    }
    !anchored || pos == path.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_group_and_longest_match() {
        let r = Robots::parse(
            "User-agent: *\nDisallow: /private\nAllow: /private/open\n",
            "jj1",
        );
        assert!(r.allowed("/"));
        assert!(!r.allowed("/private/x"));
        assert!(r.allowed("/private/open/x"));
    }

    #[test]
    fn specific_group_overrides_star() {
        let r = Robots::parse(
            "User-agent: *\nDisallow: /\n\nUser-agent: JJ1\nDisallow: /admin\n",
            "jj1",
        );
        assert!(r.allowed("/page"));
        assert!(!r.allowed("/admin/x"));
    }

    #[test]
    fn wildcards() {
        let r = Robots::parse("User-agent: *\nDisallow: /*.pdf$\nDisallow: /a/*/b\n", "jj1");
        assert!(!r.allowed("/docs/x.pdf"));
        assert!(r.allowed("/docs/x.pdf?x=1"));
        assert!(!r.allowed("/a/zzz/b/c"));
        assert!(r.allowed("/a/b"));
    }

    #[test]
    fn empty_disallow_allows_everything() {
        let r = Robots::parse("User-agent: *\nDisallow:\n", "jj1");
        assert!(r.allowed("/anything"));
    }
}
