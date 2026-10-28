//! README.md lists exactly the modules declared in src/lib.rs.

#[test]
fn readme_lists_every_library_module() {
    let lib: Vec<&str> = include_str!("lib.rs")
        .lines()
        .filter_map(|l| l.trim().strip_prefix("pub mod "))
        .filter_map(|l| l.strip_suffix(';'))
        .collect();
    let readme = include_str!("../README.md");
    let block = readme
        .split("<!-- modules:begin -->")
        .nth(1)
        .and_then(|r| r.split("<!-- modules:end -->").next())
        .expect("README.md has a modules:begin/end block");
    let listed: Vec<&str> = block
        .lines()
        .filter_map(|l| l.strip_prefix("- `"))
        .filter_map(|l| l.split('`').next())
        .collect();
    let mut a = lib.clone();
    let mut b = listed.clone();
    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b, "README.md module list differs from src/lib.rs");
}

/// Source text before the first `#[cfg(test)]`, so tests don't count as users.
fn non_test(src: &str) -> &str {
    src.split("#[cfg(test)]").next().unwrap_or(src)
}

/// `src` with `//` and `/* */` comments blanked, so a module named only in
/// a comment does not count as used. String and char literals are kept.
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            let mut depth = 0;
            while i < b.len() {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            out.push(b' ');
        } else if b[i..].starts_with(b"'\"'") || b[i..].starts_with(b"'\\\"'") {
            // A '"' char literal does not open a string.
            let n = if b[i + 1] == b'\\' { 4 } else { 3 };
            out.extend_from_slice(&b[i..i + n]);
            i += n;
        } else if b[i] == b'r' && matches!(b.get(i + 1), Some(b'"' | b'#')) {
            // Raw string r#"…"#: copy it whole.
            let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
            if b.get(i + 1 + hashes) != Some(&b'"') {
                out.push(b[i]);
                i += 1;
                continue;
            }
            let close: Vec<u8> = std::iter::once(b'"')
                .chain(std::iter::repeat_n(b'#', hashes))
                .collect();
            let start = i;
            i += 2 + hashes;
            while i < b.len() && !b[i..].starts_with(&close) {
                i += 1;
            }
            i = (i + close.len()).min(b.len());
            out.extend_from_slice(&b[start..i]);
        } else if b[i] == b'"' {
            out.push(b[i]);
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' && i + 1 < b.len() {
                    out.push(b[i]);
                    i += 1;
                }
                out.push(b[i]);
                i += 1;
            }
            if i < b.len() {
                out.push(b[i]);
                i += 1;
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).expect("comment stripping keeps UTF-8")
}

/// Every .rs file under `dir`, comments stripped. A directory that can't be
/// read fails the test instead of being skipped.
fn rs_files(dir: &std::path::Path, out: &mut Vec<String>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    for e in entries {
        let p = e
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
            .path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let src =
                std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
            out.push(strip_comments(&src));
        }
    }
}

fn has_word(hay: &str, word: &str) -> bool {
    hay.match_indices(word).any(|(i, _)| {
        let ok = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        ok(hay[..i].chars().next_back()) && ok(hay[i + word.len()..].chars().next())
    })
}

/// The README says every module is used by a binary or example. Check it:
/// walk from the non-test code of src/bin and examples through the modules
/// they name (`m::…` or an item lib.rs re-exports from `m`).
#[test]
fn every_library_module_is_reached_from_a_binary_or_example() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let lib = include_str!("lib.rs");
    let mods: Vec<&str> = lib
        .lines()
        .filter_map(|l| l.trim().strip_prefix("pub mod "))
        .filter_map(|l| l.strip_suffix(';'))
        .collect();
    // Names that point at module m from outside it.
    let names = |m: &str| -> Vec<String> {
        let mut v = vec![format!("{m}::")];
        for l in lib.lines() {
            let Some(rest) = l.trim().strip_prefix(&format!("pub use {m}::")) else {
                continue;
            };
            let rest = rest
                .trim_end_matches(';')
                .trim_matches(|c| c == '{' || c == '}');
            if rest == "*" {
                let src = std::fs::read_to_string(root.join(format!("src/{m}.rs"))).expect("read");
                for l in non_test(&src).lines() {
                    for kw in [
                        "pub struct ",
                        "pub enum ",
                        "pub fn ",
                        "pub trait ",
                        "pub const ",
                    ] {
                        if let Some(n) = l.strip_prefix(kw) {
                            let n: String = n
                                .chars()
                                .take_while(|c| c.is_alphanumeric() || *c == '_')
                                .collect();
                            v.push(n);
                        }
                    }
                }
            } else {
                for it in rest.split(',') {
                    if let Some(n) = it.split(" as ").last().map(str::trim) {
                        if !n.is_empty() {
                            v.push(n.to_string());
                        }
                    }
                }
            }
        }
        v
    };
    let refers = |src: &str, m: &str| {
        names(m).iter().any(|n| {
            if let Some(stem) = n.strip_suffix("::") {
                // `m::` as a path segment, or `m` inside a `use …::{…}` group.
                src.match_indices(n.as_str()).any(|(i, _)| {
                    src[..i]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
                }) || src.contains(&format!("{{{stem},"))
                    || src.contains(&format!(" {stem},"))
                    || src.contains(&format!("{{self, {stem}"))
            } else {
                has_word(src, n)
            }
        })
    };
    let mut users = Vec::new();
    rs_files(&root.join("src/bin"), &mut users);
    rs_files(&root.join("examples"), &mut users);
    let roots: String = users
        .iter()
        .map(|s| non_test(s))
        .collect::<Vec<_>>()
        .join("\n");
    let mut reached: Vec<&str> = mods.iter().copied().filter(|m| refers(&roots, m)).collect();
    let mut i = 0;
    while i < reached.len() {
        let src = strip_comments(
            &std::fs::read_to_string(root.join(format!("src/{}.rs", reached[i])))
                .expect("read module"),
        );
        for m in &mods {
            if !reached.contains(m) && refers(non_test(&src), m) {
                reached.push(m);
            }
        }
        i += 1;
    }
    let unused: Vec<&&str> = mods.iter().filter(|m| !reached.contains(m)).collect();
    assert!(
        unused.is_empty(),
        "lib modules no binary or example reaches: {unused:?}"
    );
}

/// Day of the week for a Gregorian date (Sakamoto's method), 0 = Sunday.
fn weekday(y: i32, m: usize, d: i32) -> usize {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + T[m - 1] + d) % 7) as usize
}

/// Every "Ddd Mmm D, YYYY" in `s`, checked: the weekday must match the date
/// and the date must be labelled "(target".
fn checked_dates(s: &str) -> Vec<String> {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut out = Vec::new();
    for (wd, day) in DAYS.iter().enumerate() {
        for (i, _) in s.match_indices(&format!("{day} ")) {
            let rest = &s[i + 4..];
            let Some(m) = MONTHS
                .iter()
                .position(|m| rest.starts_with(&format!("{m} ")))
            else {
                continue;
            };
            let Some((d, rest)) = rest[4..].split_once(", ") else {
                continue;
            };
            let (Ok(d), Ok(y)) = (d.parse::<i32>(), rest.get(..4).unwrap_or("").parse::<i32>())
            else {
                continue;
            };
            assert_eq!(
                weekday(y, m + 1, d),
                wd,
                "wrong weekday: {}",
                s[i..].chars().take(16).collect::<String>()
            );
            assert!(
                rest[4..].starts_with(" (target"),
                "date not labelled as a target: {}",
                s[i..].chars().take(16).collect::<String>()
            );
            out.push(format!("{day} {} {d}, {y}", MONTHS[m]));
        }
    }
    out
}

/// The Planned section carries the agreed target dates, each labelled as a
/// target with the right weekday, and every other row says "after v0.2.0".
#[test]
fn planned_tables_have_target_dates_or_after_v0_2_0() {
    let readme = include_str!("../README.md");
    let planned = readme
        .split("### Planned (not built)")
        .nth(1)
        .and_then(|r| r.split("Removed and not planned").next())
        .expect("README.md has a Planned (not built) section");
    let rows: Vec<&str> = planned
        .lines()
        .filter(|l| {
            l.starts_with("| ") && !l.starts_with("| Item ") && !l.starts_with("| Milestone ")
        })
        .collect();
    assert!(rows.len() >= 10, "{rows:?}");
    for r in &rows {
        let target = r.trim_end_matches('|').rsplit(" | ").next().unwrap_or("");
        assert!(
            target.starts_with("after v0.2.0") || !checked_dates(target).is_empty(),
            "row has neither a target date nor \"after v0.2.0\": {r}"
        );
    }
    let dates = checked_dates(planned);
    for want in ["Wed Oct 28, 2026", "Sun Nov 1, 2026", "Fri Nov 6, 2026"] {
        assert!(dates.iter().any(|d| d == want), "missing {want}: {dates:?}");
    }
    assert!(planned.contains("America/Edmonton"));
    assert!(planned.contains("pending the owner's approval"));
    assert!(planned.contains("does not unlock automatically"));
    for (name, text) in [
        ("README.md", readme),
        ("PLAN.md", include_str!("../PLAN.md")),
    ] {
        assert!(
            !text.contains("not dates"),
            "{name} still says \"not dates\""
        );
        checked_dates(text);
    }
}

#[test]
fn strip_comments_drops_comments_and_keeps_literals() {
    let src = "a::x(); // b::y\n/* c::z /* nested */ d::w */ e::v;\n\
               let s = \"// f::u\"; let q = '\"'; g::t();\n\
               let r = r#\"/* h::s \"# ; i::r();";
    let out = strip_comments(src);
    for kept in [
        "a::x",
        "e::v",
        "\"// f::u\"",
        "g::t",
        "r#\"/* h::s \"#",
        "i::r",
    ] {
        assert!(out.contains(kept), "lost {kept}: {out}");
    }
    for gone in ["b::y", "c::z", "d::w"] {
        assert!(!out.contains(gone), "kept {gone}: {out}");
    }
}

#[test]
#[should_panic(expected = "read_dir")]
fn an_unreadable_source_directory_fails_the_walk() {
    rs_files(
        std::path::Path::new("/nonexistent/seraphina-src"),
        &mut Vec::new(),
    );
}
