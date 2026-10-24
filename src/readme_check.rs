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

/// `src` without its `#[cfg(test)]` items, so tests don't count as users.
/// An item ends at a `;` or `,` outside brackets, or at the `}` that closes
/// its body: a test module, a `mod x;`, a struct field or a `let` all go,
/// and the code after them stays.
fn non_test(src: &str) -> String {
    const ATTR: &str = "#[cfg(test)]";
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find(ATTR) {
        out.push_str(&rest[..at]);
        let item = &rest[at + ATTR.len()..];
        let mut depth = 0i32;
        let mut end = item.len();
        for (i, c) in item.char_indices() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' => depth -= 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                ';' | ',' if depth == 0 => {
                    end = i + 1;
                    break;
                }
                _ => {}
            }
        }
        rest = &item[end..];
    }
    out.push_str(rest);
    out
}

#[test]
fn non_test_drops_test_items_and_keeps_the_code_after_them() {
    let src = "mod a;\n#[cfg(test)]\nmod test_dir;\nmod b;\nstruct S {\n    x: u8,\n    #[cfg(test)]\n    dir: Vec<(u8, u8)>,\n    y: b::Y,\n}\nfn f() {\n    #[cfg(test)]\n    let d = format!(\"{}\", 1);\n    c::go();\n}\n#[cfg(test)]\nmod tests {\n    fn t() { d::x(); }\n}\nfn g() { e::y(); }\n";
    let kept = non_test(src);
    for used in ["mod a;", "mod b;", "b::Y", "c::go()", "e::y()"] {
        assert!(kept.contains(used), "{used} dropped:\n{kept}");
    }
    for gone in ["test_dir", "dir:", "format!", "d::x"] {
        assert!(!kept.contains(gone), "{gone} kept:\n{kept}");
    }
}

/// `src` with `//` and `/* */` comments dropped and the contents of string
/// and char literals emptied (the quotes stay), so a module named only in a
/// comment or inside a string does not count as used. The exception is a
/// serde attribute path (`deserialize_with = "m::f"`), which is code.
fn code_only(src: &str) -> String {
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
            out.extend_from_slice(b"''");
            i += if b[i + 1] == b'\\' { 4 } else { 3 };
        } else if b[i] == b'r' && matches!(b.get(i + 1), Some(b'"' | b'#')) {
            // Raw string r#"…"#: keep the delimiters, drop the contents.
            let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
            if b.get(i + 1 + hashes) != Some(&b'"') {
                out.push(b[i]);
                i += 1;
                continue;
            }
            let close: Vec<u8> = std::iter::once(b'"')
                .chain(std::iter::repeat_n(b'#', hashes))
                .collect();
            out.extend_from_slice(&b[i..i + 2 + hashes]);
            i += 2 + hashes;
            while i < b.len() && !b[i..].starts_with(&close) {
                i += 1;
            }
            out.extend_from_slice(&b[i..(i + close.len()).min(b.len())]);
            i = (i + close.len()).min(b.len());
        } else if b[i] == b'"' {
            // serde's `with = "path"` and `default = "path"` name real code,
            // but only inside an open `#[serde(…)]` attribute (or
            // `#[cfg_attr(…, serde(…))]`).
            let tail = String::from_utf8_lossy(&out[out.len().saturating_sub(1024)..]);
            let in_serde = tail.rfind("#[").is_some_and(|a| {
                let attr = &tail[a..];
                !attr.contains(']')
                    && (attr.starts_with("#[serde(")
                        || attr.starts_with("#[cfg_attr(") && attr.contains(" serde("))
            });
            let before = tail.trim_end();
            let path = in_serde && ["with =", "default ="].iter().any(|k| before.ends_with(k));
            let start = i;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            if path {
                out.extend_from_slice(&b[start..i.min(b.len())]);
            } else {
                out.push(b'"');
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
    String::from_utf8(out).expect("dropping whole comments and literals keeps UTF-8")
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
            out.push(code_only(&src));
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
        let src = code_only(
            &std::fs::read_to_string(root.join(format!("src/{}.rs", reached[i])))
                .expect("read module"),
        );
        for m in &mods {
            if !reached.contains(m) && refers(&non_test(&src), m) {
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

/// The agreed ship targets. README's Planned section and PLAN.md both carry
/// every one of them.
const TARGET_DATES: [&str; 3] = ["Wed Oct 28, 2026", "Sun Nov 1, 2026", "Fri Nov 6, 2026"];

/// The Planned section carries the agreed target dates, each labelled as a
/// target with the right weekday, and every other row says "after v0.2.0".
/// PLAN.md carries the same dates.
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
    let plan = include_str!("../PLAN.md");
    for (name, text) in [("README.md Planned", planned), ("PLAN.md", plan)] {
        let dates = checked_dates(text);
        for want in TARGET_DATES {
            assert!(
                dates.iter().any(|d| d == want),
                "{name} is missing {want}: {dates:?}"
            );
        }
    }
    assert!(planned.contains("America/Edmonton"));
    assert!(planned.contains("pending the owner's approval"));
    assert!(planned.contains("does not unlock automatically"));
    for (name, text) in [("README.md", readme), ("PLAN.md", plan)] {
        assert!(
            !text.contains("not dates"),
            "{name} still says \"not dates\""
        );
        checked_dates(text);
    }
}

#[test]
fn code_only_drops_comments_and_literal_contents() {
    let src = "a::x(); // b::y\n/* c::z /* nested */ d::w */ e::v;\n\
               let s = \"// f::u \\\" j::q\"; let q = '\"'; g::t();\n\
               let r = r#\"/* h::s \"# ; i::r();";
    let out = code_only(src);
    for kept in [
        "a::x",
        "e::v",
        "let s = \"\";",
        "''",
        "g::t",
        "r#\"\"#",
        "i::r",
    ] {
        assert!(out.contains(kept), "lost {kept}: {out}");
    }
    for gone in ["b::y", "c::z", "d::w", "f::u", "j::q", "h::s"] {
        assert!(!out.contains(gone), "kept {gone}: {out}");
    }
}

/// A module whose name appears only inside a string literal in a binary
/// is not reached.
#[test]
fn a_module_named_only_in_a_string_is_not_used() {
    let bin = code_only("fn main() { println!(\"see lonely::probe()\"); }");
    assert!(!bin.contains("lonely::"), "{bin}");
    let attr = code_only("#[serde(deserialize_with = \"crate::helper::f\")] x: u8,");
    assert!(attr.contains("crate::helper::f"), "{attr}");
    let attr = code_only("#[serde(default = \"a::d\", with = \"b::w\")] x: u8,");
    assert!(attr.contains("a::d") && attr.contains("b::w"), "{attr}");
    let attr = code_only("#[cfg_attr(feature = \"ui\", serde(with = \"c::w\"))] x: u8,");
    assert!(attr.contains("c::w"), "{attr}");
    // the same words outside a serde attribute are just strings
    for src in [
        "#[arg(default = \"lonely::x\")] x: u8,",
        "let with = \"lonely::y\";",
        "#[serde(rename = \"n\")] #[arg(with = \"lonely::z\")] x: u8,",
    ] {
        let out = code_only(src);
        assert!(!out.contains("lonely::"), "{out}");
    }
}

/// `ui` is a default feature, so current docs and comments give commands
/// without naming that feature. docs/UI_AUDIT.md is a dated record and keeps
/// the commands it ran.
#[test]
fn current_docs_do_not_ask_for_the_default_ui_feature() {
    fn walk(p: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if p.is_dir() {
            let entries =
                std::fs::read_dir(p).unwrap_or_else(|e| panic!("read_dir {}: {e}", p.display()));
            for e in entries {
                walk(&e.expect("dir entry").path(), out);
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for d in ["src", "examples", "docs", ".github"] {
        walk(&root.join(d), &mut files);
    }
    for f in ["README.md", "CONTRIBUTING.md", "PLAN.md", "TOOLS.md"] {
        files.push(root.join(f));
    }
    let needle = ["--features", " ui"].concat();
    let stale: Vec<String> = files
        .iter()
        .filter(|f| !f.ends_with("docs/UI_AUDIT.md"))
        .filter(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains(&needle)))
        .map(|f| f.strip_prefix(root).unwrap_or(f).display().to_string())
        .collect();
    assert!(stale.is_empty(), "still say {needle}: {stale:?}");
}

#[test]
#[should_panic(expected = "read_dir")]
fn an_unreadable_source_directory_fails_the_walk() {
    rs_files(
        std::path::Path::new("/nonexistent/seraphina-src"),
        &mut Vec::new(),
    );
}

/// The README's Windows steps are a plain clone and `cargo build --release`,
/// and that must build the app: `ui` is a default feature, the bin is `ui`
/// (so the exe is target\release\ui.exe), and CI's Windows job runs the
/// same plain build.
#[test]
fn a_plain_release_build_makes_the_exe_the_readme_names() {
    let cargo = include_str!("../Cargo.toml");
    let readme = include_str!("../README.md");
    let ci = include_str!("../.github/workflows/ci.yml");

    let features = cargo
        .split("\n[features]\n")
        .nth(1)
        .expect("Cargo.toml has a [features] table");
    let default = features
        .lines()
        .take_while(|l| !l.starts_with('['))
        .find_map(|l| l.strip_prefix("default = "))
        .expect("Cargo.toml [features] sets default");
    assert!(
        default.contains("\"ui\""),
        "default features {default} leave out ui, so `cargo build --release` builds no exe"
    );

    let bin = cargo
        .split("\n[[bin]]\n")
        .nth(1)
        .expect("Cargo.toml has a [[bin]]");
    assert!(
        bin.lines().next() == Some("name = \"ui\""),
        "the app bin is not named ui, so it is not ui.exe"
    );

    let steps = "```bat\ngit clone https://github.com/SynerGro-AI/seraphina-studio\n\
                 cd seraphina-studio\ncargo build --release\n```";
    assert!(
        readme.contains(steps),
        "README lacks the plain Windows build steps:\n{steps}"
    );
    assert!(readme.contains("**`target\\release\\ui.exe`**"));
    assert!(readme.contains("**ffmpeg** is not bundled"));
    assert!(
        ci.contains("- run: cargo build --locked --release\n"),
        "CI does not run the README's plain release build"
    );
    assert!(
        ci.contains("./packaging/windows/stage.ps1 -Dest pkg -Exe target/release/ui.exe"),
        "CI does not package the exe the README's build makes"
    );
}

/// packaging/windows/README.txt promises the exe needs no Visual C++
/// Redistributable. That holds only if the MSVC build links the C runtime
/// in, which .cargo/config.toml does for every build (local, ci.yml,
/// release.yml); a RUSTFLAGS variable in CI would silently replace it.
#[test]
fn the_msvc_build_links_the_c_runtime_the_package_readme_promises() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = include_str!("../packaging/windows/README.txt");
    let ci = include_str!("../.github/workflows/ci.yml");
    let words = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        words.contains("no Visual C++ Redistributable should be needed"),
        "README.txt no longer makes the promise this test backs"
    );
    let path = root.join(".cargo/config.toml");
    let config = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; without it the MSVC exe needs the VC++ Redistributable",
            path.display()
        )
    });
    let msvc = config
        .split("[target.x86_64-pc-windows-msvc]")
        .nth(1)
        .expect(".cargo/config.toml has no [target.x86_64-pc-windows-msvc] table");
    let rustflags = msvc
        .lines()
        .take_while(|l| !l.trim_start().starts_with('['))
        .find(|l| l.trim_start().starts_with("rustflags"))
        .expect("the msvc table sets rustflags");
    assert!(
        rustflags.contains("\"target-feature=+crt-static\""),
        "{rustflags}"
    );
    assert!(
        !ci.contains("RUSTFLAGS"),
        "ci.yml sets RUSTFLAGS, which overrides .cargo/config.toml's rustflags"
    );
}

/// Until a real MSVC build has run, the docs must not claim the MSVC exe
/// or its static CRT works: README.md and packaging/windows/README.txt say
/// "not verified". README.md also says hardware camera testing is manual,
/// and the real-camera tests stay ignored.
#[test]
fn msvc_build_and_hardware_cameras_are_marked_unverified() {
    let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let readme = words(include_str!("../README.md"));
    let package = words(include_str!("../packaging/windows/README.txt"));
    assert!(
        readme.contains("**MSVC build and static CRT: not verified.**"),
        "README.md doesn't mark the MSVC build and static CRT not verified"
    );
    assert!(
        package.contains("static CRT")
            && package.contains("NOT VERIFIED: no MSVC build of this version has been run"),
        "packaging/windows/README.txt claims the MSVC build without marking it not verified"
    );
    assert!(
        readme.contains("**Hardware camera testing is manual.**"),
        "README.md doesn't say hardware camera testing is manual"
    );
    let camera = include_str!("bin/ui/camera.rs");
    for t in [
        "fn v4l2_reads_frames_from_a_real_device()",
        "fn media_foundation_reads_frames_from_a_real_camera()",
    ] {
        let at = camera.find(t).unwrap_or_else(|| panic!("{t} is gone"));
        let attrs = &camera[camera[..at].rfind("#[test]").unwrap()..at];
        assert!(attrs.contains("#[ignore"), "{t} is no longer ignored");
    }
}

/// The DLLs a PE image imports, read from its import directory.
#[cfg(all(windows, target_env = "msvc"))]
fn pe_imports(b: &[u8]) -> Vec<String> {
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
    let pe = u32_at(0x3c);
    assert_eq!(&b[pe..pe + 4], b"PE\0\0", "not a PE image");
    let sections = u16_at(pe + 6);
    let opt = pe + 24;
    let opt_size = u16_at(pe + 20);
    // PE32+ (0x20b) puts the data directories at 112, PE32 at 96.
    let dirs = opt + if u16_at(opt) == 0x20b { 112 } else { 96 };
    let import_rva = u32_at(dirs + 8);
    let table = opt + opt_size;
    let file_offset = |rva: usize| {
        (0..sections)
            .map(|i| table + i * 40)
            .find_map(|s| {
                let (va, size, raw) = (
                    u32_at(s + 12),
                    u32_at(s + 8).max(u32_at(s + 16)),
                    u32_at(s + 20),
                );
                (va..va + size).contains(&rva).then(|| rva - va + raw)
            })
            .expect("RVA outside every section")
    };
    let mut out = Vec::new();
    let mut d = file_offset(import_rva);
    while u32_at(d + 12) != 0 {
        let name = file_offset(u32_at(d + 12));
        let end = b[name..].iter().position(|&c| c == 0).expect("NUL") + name;
        out.push(String::from_utf8_lossy(&b[name..end]).to_ascii_lowercase());
        d += 20;
    }
    out
}

/// The MSVC build (CI's windows-ui job, release.yml): this test exe, built
/// with the same flags as the app, must not import the VC++ / Universal C
/// runtime DLLs. Only for MSVC: the mingw build links the C runtime its
/// own way, so passing there would say nothing about the shipped exe.
#[cfg(all(windows, target_env = "msvc"))]
#[test]
fn this_exe_does_not_import_the_vc_runtime() {
    let exe = std::env::current_exe().expect("current_exe");
    let imports = pe_imports(&std::fs::read(&exe).expect("read current exe"));
    assert!(
        imports.iter().any(|d| d == "kernel32.dll"),
        "import table not read: {imports:?}"
    );
    let crt: Vec<&String> = imports
        .iter()
        .filter(|d| {
            d.starts_with("vcruntime") || d.starts_with("api-ms-win-crt-") || *d == "ucrtbase.dll"
        })
        .collect();
    assert!(
        crt.is_empty(),
        "{} imports {crt:?}: the C runtime is not linked in",
        exe.display()
    );
}
