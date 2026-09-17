//! `wt` command lines for opening tabs (see "`wt` command line" and
//! "Opening sessions in a new window" in `docs/ARCHITECTURE.md`).

use std::ffi::OsString;
use std::path::Path;

use crate::{TabSpec, Target};

/// The profile every NativeTerm tab uses: its command line is the shim
/// without a host, and it suppresses title changes.
pub const PROFILE_NAME: &str = "NativeTerm SSH";

/// Stays well below the 32,767-character limit of a command line.
pub const MAX_COMMAND_LINE: usize = 30_000;

/// Arguments of one `new-tab` subcommand.
pub fn new_tab(tab: &TabSpec, shim: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "new-tab".into(),
        "--profile".into(),
        PROFILE_NAME.into(),
        "--sessionId".into(),
        format!("{{{}}}", tab.terminal_session.trim_matches(['{', '}'])).into(),
        // `=` so a label starting with `-` isn't read as an option
        format!("--title={}", escape_delimiters(&tab.label)).into(),
        // no effect in 1.26 (the profile does it), harmless
        "--suppressApplicationTitle".into(),
        shim.into(),
        "--session".into(),
        tab.session.clone().into(),
    ];
    if tab.wait {
        args.push("--wait".into());
    }
    args.push(tab.alias.clone().into());
    args
}

/// `wt` splits subcommands at `;` anywhere in an argument unless escaped.
/// (A `\` right before a `;` can't be expressed; labels don't need it.)
pub fn escape_delimiters(text: &str) -> String {
    text.replace(';', "\\;")
}

fn window_argument(target: &Target, first: bool) -> String {
    match target {
        Target::Recent => "0".into(),
        Target::NewWindow if first => "new".into(),
        // the new window is the most recent one while the user stays there
        Target::NewWindow => "0".into(),
        Target::Named(name) => name.clone(),
    }
}

/// `wt` invocations for `tabs`: as many tabs per invocation as fit.
/// Each item is (the tabs it opens, its arguments).
pub fn batches<'a>(target: &Target, tabs: &'a [TabSpec], shim: &Path) -> Vec<(&'a [TabSpec], Vec<OsString>)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < tabs.len() {
        let mut args: Vec<OsString> = vec!["-w".into(), window_argument(target, out.is_empty()).into()];
        // wt.exe's own path plus a margin
        let mut length = 300 + args.iter().map(|a| quoted_len(a)).sum::<usize>();
        let mut end = start;
        while end < tabs.len() {
            let mut part = if end > start { vec![OsString::from(";")] } else { Vec::new() };
            part.extend(new_tab(&tabs[end], shim));
            let part_length: usize = part.iter().map(|a| quoted_len(a)).sum();
            if end > start && length + part_length > MAX_COMMAND_LINE {
                break;
            }
            length += part_length;
            args.extend(part);
            end += 1;
        }
        out.push((&tabs[start..end], args));
        start = end;
    }
    out
}

/// Length of an argument as quoted into a command line, plus a separator.
pub fn quoted_len(arg: &std::ffi::OsStr) -> usize {
    quote(&arg.to_string_lossy()).encode_utf16().count() + 1
}

/// Quote one argument the way `CommandLineToArgvW` reads it back.
pub fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// A whole command line (arguments only) for APIs that take one string.
pub fn join(args: &[OsString]) -> String {
    args.iter().map(|a| quote(&a.to_string_lossy())).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(n: usize, label: &str) -> TabSpec {
        TabSpec {
            terminal_session: format!("6e7a0000-0000-4000-8000-{n:012}"),
            label: label.to_string(),
            session: format!("s-{n}"),
            alias: format!("host{n}"),
            wait: false,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn one_tab() {
        let shim = Path::new(r"C:\Program Files\NativeTerm\nativeterm-shim.exe");
        let tabs = [tab(1, "web01; prod")];
        let batches = batches(&Target::Recent, &tabs, shim);
        assert_eq!(batches.len(), 1);
        assert_eq!(
            strings(&batches[0].1),
            [
                "-w",
                "0",
                "new-tab",
                "--profile",
                "NativeTerm SSH",
                "--sessionId",
                "{6e7a0000-0000-4000-8000-000000000001}",
                r"--title=web01\; prod",
                "--suppressApplicationTitle",
                r"C:\Program Files\NativeTerm\nativeterm-shim.exe",
                "--session",
                "s-1",
                "host1",
            ]
        );
    }

    #[test]
    fn waiting_tab() {
        let mut t = tab(1, "a");
        t.wait = true;
        let args = strings(&new_tab(&t, Path::new("shim")));
        assert_eq!(args[args.len() - 4..], ["--session", "s-1", "--wait", "host1"]);
    }

    #[test]
    fn braces_are_not_doubled() {
        let mut t = tab(1, "a");
        t.terminal_session = "{6e7a0000-0000-4000-8000-000000000001}".into();
        assert_eq!(strings(&new_tab(&t, Path::new("s")))[4], "{6e7a0000-0000-4000-8000-000000000001}");
    }

    #[test]
    fn new_window_batches() {
        let shim = Path::new(r"C:\Users\someone\AppData\Local\Programs\NativeTerm\nativeterm-shim.exe");
        let tabs: Vec<TabSpec> = (0..250).map(|n| tab(n, &format!("中文会话名称 {n} (production cluster)"))).collect();
        let batches = batches(&Target::NewWindow, &tabs, shim);
        assert!(batches.len() >= 3, "{}", batches.len());
        assert_eq!(batches.iter().map(|b| b.0.len()).sum::<usize>(), 250);
        assert_eq!(strings(&batches[0].1)[..2], ["-w", "new"]);
        for (_, args) in &batches[1..] {
            assert_eq!(strings(args)[..2], ["-w", "0"]);
        }
        for (tabs, args) in &batches {
            assert!(join(args).encode_utf16().count() + 300 <= MAX_COMMAND_LINE);
            assert_eq!(strings(args).iter().filter(|a| *a == ";").count(), tabs.len() - 1);
        }
        assert!(batches[0].0.len() >= 70, "about 100 tabs per call: {}", batches[0].0.len());
    }

    #[test]
    fn named_window_everywhere() {
        let tabs = [tab(1, "a"), tab(2, "b")];
        let batches = batches(&Target::Named("NativeTerm".into()), &tabs, Path::new("shim"));
        assert_eq!(strings(&batches[0].1)[..2], ["-w", "NativeTerm"]);
    }

    #[test]
    fn quoting_matches_command_line_to_argv() {
        for (arg, quoted) in [
            ("plain", "plain"),
            ("", "\"\""),
            ("a b", "\"a b\""),
            (r#"say "hi""#, r#""say \"hi\"""#),
            (r"C:\dir with space\", r#""C:\dir with space\\""#),
            (r#"a\"b"#, r#""a\\\"b""#),
            (r"C:\no\space", r"C:\no\space"),
        ] {
            assert_eq!(quote(arg), quoted, "{arg}");
        }
    }
}
