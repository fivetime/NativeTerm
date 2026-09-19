//! Prototype: inspect Windows Terminal through UI Automation.
//!
//! uia-probe list                 windows and their tabs (name, RuntimeId, selected)
//! uia-probe select <substring>   select the first tab whose name contains <substring>
//! uia-probe close <substring>    close that tab through its close button
//! uia-probe text                 visible text of each window's terminal control
//! uia-probe dump [depth]         control tree of the first window (exploration)
//! uia-probe watch <seconds>      print the tab list whenever it changes
//! uia-probe events <secs> <tab>  print tab-selection and focus events as they arrive,
//!                                on the window that has a tab named like <tab>

use std::time::{Duration, Instant};

use uiautomation::patterns::{UIInvokePattern, UISelectionItemPattern, UITextPattern};
use uiautomation::types::{ControlType, TreeScope};
use uiautomation::{UIAutomation, UIElement};

const WT_WINDOW_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";

type Res<T> = uiautomation::Result<T>;

/// Test Terminal install; set NT_PROBE_WT_DIR to override, or to "*" for all.
/// Never probe the Store Terminal the user works in.
pub const TEST_TERMINAL_DIR: &str = r"C:\MyProjects\RustProjects\terminal-1.26.2581.0";

fn process_image(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn is_test_terminal(window: &UIElement) -> bool {
    let dir = std::env::var("NT_PROBE_WT_DIR").unwrap_or_else(|_| TEST_TERMINAL_DIR.into());
    if dir == "*" {
        return true;
    }
    let pid = window.get_process_id().unwrap_or(0);
    process_image(pid).is_some_and(|p| p.to_lowercase().starts_with(&format!("{}\\", dir.to_lowercase())))
}

/// Terminal top-level windows via Win32 (no UIA), with a WM_NULL probe.
fn terminal_hwnds() -> Vec<(windows::Win32::Foundation::HWND, bool)> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowThreadProcessId, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
        WM_NULL,
    };
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, &mut class) };
        if String::from_utf16_lossy(&class[..n as usize]) == WT_WINDOW_CLASS {
            unsafe { &mut *(found.0 as *mut Vec<HWND>) }.push(hwnd);
        }
        BOOL(1)
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut all as *mut _ as isize));
    }
    let dir = std::env::var("NT_PROBE_WT_DIR").unwrap_or_else(|_| TEST_TERMINAL_DIR.into()).to_lowercase();
    all.into_iter()
        .filter(|hwnd| {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(*hwnd, Some(&mut pid)) };
            dir == "*" || process_image(pid).is_some_and(|p| p.to_lowercase().starts_with(&format!("{dir}\\")))
        })
        .map(|hwnd| {
            let mut result = 0usize;
            let answered = unsafe {
                SendMessageTimeoutW(
                    hwnd,
                    WM_NULL,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    250,
                    Some(&mut result),
                )
            };
            (hwnd, answered.0 != 0)
        })
        .collect()
}

fn terminal_windows(automation: &UIAutomation) -> Res<Vec<UIElement>> {
    // NT_UIA_FROM_HWND: enumerate with Win32 and skip unresponsive windows
    // (NT_UIA_NOGATE=1 keeps them, to see whether UIA timeouts apply).
    if std::env::var("NT_UIA_FROM_HWND").is_ok() {
        let gate = std::env::var("NT_UIA_NOGATE").is_err();
        let mut out = Vec::new();
        for (hwnd, responsive) in terminal_hwnds() {
            if gate && !responsive {
                eprintln!("skipped unresponsive window {:?}", hwnd.0);
                continue;
            }
            let started = Instant::now();
            let element = automation.element_from_handle(hwnd.into());
            eprintln!("ElementFromHandle {:?}: {} ms, ok={}", hwnd.0, started.elapsed().as_millis(), element.is_ok());
            if let Ok(e) = element {
                out.push(e);
            }
        }
        return Ok(out);
    }
    let root = automation.get_root_element()?;
    let all = root.find_all(TreeScope::Children, &automation.create_true_condition()?)?;
    Ok(all
        .into_iter()
        .filter(|w| w.get_classname().map(|c| c == WT_WINDOW_CLASS).unwrap_or(false))
        .filter(is_test_terminal)
        .collect())
}

// FindAll(Descendants) doesn't cross into Windows Terminal's XAML island
// (it returns no tab items), so walk the control view instead.
fn descendants(automation: &UIAutomation, el: &UIElement) -> Res<Vec<UIElement>> {
    fn visit(walker: &uiautomation::UITreeWalker, e: &UIElement, out: &mut Vec<UIElement>) {
        let mut child = walker.get_first_child(e).ok();
        while let Some(c) = child {
            out.push(c.clone());
            visit(walker, &c, out);
            child = walker.get_next_sibling(&c).ok();
        }
    }
    let walker = automation.get_control_view_walker()?;
    let mut out = Vec::new();
    visit(&walker, el, &mut out);
    Ok(out)
}

// Tree order is the tab strip order.
fn descendants_of_type(automation: &UIAutomation, el: &UIElement, ty: ControlType) -> Res<Vec<UIElement>> {
    Ok(descendants(automation, el)?
        .into_iter()
        .filter(|e| e.get_control_type().map(|t| t == ty).unwrap_or(false))
        .collect())
}

fn tabs(automation: &UIAutomation, window: &UIElement) -> Res<Vec<UIElement>> {
    descendants_of_type(automation, window, ControlType::TabItem)
}

fn is_selected(tab: &UIElement) -> bool {
    tab.get_pattern::<UISelectionItemPattern>().and_then(|p| p.is_selected()).unwrap_or(false)
}

fn list(automation: &UIAutomation) -> Res<String> {
    let mut out = String::new();
    for (wi, w) in terminal_windows(automation)?.iter().enumerate() {
        let started = Instant::now();
        let ts = tabs(automation, w)?;
        out.push_str(&format!(
            "window {wi}: \"{}\" pid={} tabs={} (query {} ms)\n",
            w.get_name().unwrap_or_default(),
            w.get_process_id().unwrap_or(0),
            ts.len(),
            started.elapsed().as_millis()
        ));
        for (ti, t) in ts.iter().enumerate() {
            out.push_str(&format!(
                "  [{ti:>2}] {} {:?} {}\n",
                if is_selected(t) { "*" } else { " " },
                t.get_runtime_id().unwrap_or_default(),
                t.get_name().unwrap_or_default()
            ));
        }
    }
    Ok(out)
}

fn select(automation: &UIAutomation, needle: &str) -> Res<()> {
    for w in terminal_windows(automation)? {
        for t in tabs(automation, &w)? {
            let name = t.get_name().unwrap_or_default();
            if name.contains(needle) {
                t.get_pattern::<UISelectionItemPattern>()?.select()?;
                println!("selected: {name}");
                return Ok(());
            }
        }
    }
    println!("no tab contains {needle:?}");
    Ok(())
}

// Tabs scrolled out of view are virtualized away from the tree. The tab
// list supports ItemContainerPattern: FindItemByProperty(prev, 0, empty)
// walks every item in order, including virtualized ones.
fn all_tabs(automation: &UIAutomation) -> Res<()> {
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationItemContainerPattern, IUIAutomationVirtualizedItemPattern,
        UIA_ItemContainerPatternId, UIA_VirtualizedItemPatternId, UIA_PROPERTY_ID,
    };
    for (wi, w) in terminal_windows(automation)?.iter().enumerate() {
        let lists = descendants_of_type(automation, w, ControlType::List)?;
        let Some(list) = lists.first() else { continue };
        let raw: &IUIAutomationElement = list.as_ref();
        let started = Instant::now();
        let container: IUIAutomationItemContainerPattern =
            unsafe { raw.GetCurrentPatternAs(UIA_ItemContainerPatternId) }.map_err(to_err)?;
        let empty = VARIANT::default();
        let mut prev: Option<IUIAutomationElement> = None;
        let mut n = 0;
        let mut realized = 0;
        loop {
            let next = unsafe { container.FindItemByProperty(prev.as_ref(), UIA_PROPERTY_ID(0), &empty) };
            let Ok(item) = next else { break };
            let mut name = unsafe { item.CurrentName() }.map(|s| s.to_string()).unwrap_or_default();
            if name.is_empty() {
                if let Ok(v) = unsafe {
                    item.GetCurrentPatternAs::<IUIAutomationVirtualizedItemPattern>(UIA_VirtualizedItemPatternId)
                } {
                    if unsafe { v.Realize() }.is_ok() {
                        realized += 1;
                        name = unsafe { item.CurrentName() }.map(|s| s.to_string()).unwrap_or_default();
                    }
                }
            }
            println!("  [{n:>2}] {name}");
            n += 1;
            prev = Some(item);
        }
        println!(
            "window {wi}: {n} tabs via ItemContainerPattern ({realized} realized, {} ms)",
            started.elapsed().as_millis()
        );
    }
    Ok(())
}

// Select a tab that may be virtualized: find it through ItemContainerPattern,
// realize it, then select it.
fn select_any(automation: &UIAutomation, needle: &str) -> Res<()> {
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationItemContainerPattern, IUIAutomationSelectionItemPattern,
        IUIAutomationVirtualizedItemPattern, UIA_ItemContainerPatternId, UIA_SelectionItemPatternId,
        UIA_VirtualizedItemPatternId, UIA_PROPERTY_ID,
    };
    for w in terminal_windows(automation)? {
        let lists = descendants_of_type(automation, &w, ControlType::List)?;
        let Some(list) = lists.first() else { continue };
        let raw: &IUIAutomationElement = list.as_ref();
        let container: IUIAutomationItemContainerPattern =
            unsafe { raw.GetCurrentPatternAs(UIA_ItemContainerPatternId) }.map_err(to_err)?;
        let empty = VARIANT::default();
        let mut prev: Option<IUIAutomationElement> = None;
        while let Ok(item) = unsafe { container.FindItemByProperty(prev.as_ref(), UIA_PROPERTY_ID(0), &empty) } {
            let name = unsafe { item.CurrentName() }.map(|s| s.to_string()).unwrap_or_default();
            if name.contains(needle) {
                if let Ok(v) = unsafe {
                    item.GetCurrentPatternAs::<IUIAutomationVirtualizedItemPattern>(UIA_VirtualizedItemPatternId)
                } {
                    println!("realize: {:?}", unsafe { v.Realize() });
                }
                let sel: IUIAutomationSelectionItemPattern =
                    unsafe { item.GetCurrentPatternAs(UIA_SelectionItemPatternId) }.map_err(to_err)?;
                unsafe { sel.Select() }.map_err(to_err)?;
                println!("selected (possibly virtualized): {name}");
                return Ok(());
            }
            prev = Some(item);
        }
    }
    println!("no tab contains {needle:?}");
    Ok(())
}

fn to_err(e: windows::core::Error) -> uiautomation::Error {
    uiautomation::Error::new(e.code().0, &e.message())
}

// The close button is the tab item's Button child; its name is localized,
// so it is found by control type, not by name.
fn close(automation: &UIAutomation, needle: &str) -> Res<()> {
    for w in terminal_windows(automation)? {
        for t in tabs(automation, &w)? {
            let name = t.get_name().unwrap_or_default();
            if name.contains(needle) {
                let buttons = descendants_of_type(automation, &t, ControlType::Button)?;
                let button = buttons.first().ok_or_else(|| uiautomation::Error::new(-1, "no close button"))?;
                button.get_pattern::<UIInvokePattern>()?.invoke()?;
                println!("closed: {name}");
                return Ok(());
            }
        }
    }
    println!("no tab contains {needle:?}");
    Ok(())
}

fn text(automation: &UIAutomation) -> Res<()> {
    for (wi, w) in terminal_windows(automation)?.iter().enumerate() {
        for e in descendants(automation, w)? {
            if e.get_classname().unwrap_or_default() != "TermControl" {
                continue;
            }
            if let Ok(tp) = e.get_pattern::<UITextPattern>() {
                let started = Instant::now();
                let visible =
                    tp.get_visible_ranges()?.iter().filter_map(|r| r.get_text(-1).ok()).collect::<Vec<_>>().join("\n");
                let lines: Vec<&str> = visible.lines().filter(|l| !l.trim().is_empty()).collect();
                let keep = std::env::var("NT_TEXT_LINES").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
                let tail = &lines[lines.len().saturating_sub(keep)..];
                println!(
                    "window {wi}: class={} type={:?} ({} chars, {} ms), last lines:",
                    e.get_classname().unwrap_or_default(),
                    e.get_control_type(),
                    visible.chars().count(),
                    started.elapsed().as_millis()
                );
                for l in tail {
                    println!("    | {l}");
                }
                // NT_TEXT_OUT: also write the lines as UTF-8, with the code
                // points of non-ASCII characters, to avoid console re-encoding.
                if let Ok(path) = std::env::var("NT_TEXT_OUT") {
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                        let _ = writeln!(f, "window {wi}:");
                        for l in tail {
                            let cps: Vec<String> = l
                                .trim_end()
                                .chars()
                                .filter(|c| !c.is_ascii())
                                .map(|c| format!("U+{:04X}", c as u32))
                                .collect();
                            let _ = writeln!(f, "    | {}    [{}]", l.trim_end(), cps.join(" "));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// Panes of each window's selected tab: Name (starting title) and HelpText
// (live terminal title), per TermControlAutomationPeer.
fn panes(automation: &UIAutomation) -> Res<()> {
    for (wi, w) in terminal_windows(automation)?.iter().enumerate() {
        let selected = tabs(automation, w)?.into_iter().find(is_selected).and_then(|t| t.get_name().ok());
        println!("window {wi}: selected tab {selected:?}");
        for e in descendants(automation, w)? {
            if e.get_classname().unwrap_or_default() != "TermControl" {
                continue;
            }
            let r = e.get_bounding_rectangle()?;
            println!(
                "    pane name={:?} help={:?} focused={} rect=({},{})-({},{})",
                e.get_name().unwrap_or_default(),
                e.get_help_text().unwrap_or_default(),
                e.has_keyboard_focus().unwrap_or(false),
                r.get_left(),
                r.get_top(),
                r.get_right(),
                r.get_bottom()
            );
        }
    }
    Ok(())
}

fn dump(automation: &UIAutomation, depth: usize) -> Res<()> {
    let windows = terminal_windows(automation)?;
    let Some(w) = windows.first() else {
        println!("no Windows Terminal window");
        return Ok(());
    };
    let walker = automation.get_control_view_walker()?;
    fn walk(walker: &uiautomation::UITreeWalker, e: &UIElement, level: usize, max: usize) {
        println!(
            "{}{:?} class={:?} name={:?}",
            "  ".repeat(level),
            e.get_control_type().ok(),
            e.get_classname().unwrap_or_default(),
            e.get_name().unwrap_or_default()
        );
        if level >= max {
            return;
        }
        let mut child = walker.get_first_child(e).ok();
        while let Some(c) = child {
            walk(walker, &c, level + 1, max);
            child = walker.get_next_sibling(&c).ok();
        }
    }
    walk(&walker, w, 0, depth);
    Ok(())
}

fn watch(automation: &UIAutomation, seconds: u64) -> Res<()> {
    let end = Instant::now() + Duration::from_secs(seconds);
    let mut last = String::new();
    while Instant::now() < end {
        let now = list(automation)?;
        if now != last {
            println!("--- {:?}\n{now}", std::time::SystemTime::now());
            last = now;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

mod events {
    use std::time::Instant;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
        IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl, TreeScope_Subtree,
        UIA_SelectionItem_ElementSelectedEventId, UIA_EVENT_ID,
    };
    use windows_core::{implement, Ref};

    fn describe(e: &IUIAutomationElement) -> String {
        unsafe {
            format!(
                "type={} class={:?} name={:?} pid={}",
                e.CurrentControlType().map(|t| t.0).unwrap_or(0),
                e.CurrentClassName().map(|s| s.to_string()).unwrap_or_default(),
                e.CurrentName().map(|s| s.to_string()).unwrap_or_default(),
                e.CurrentProcessId().unwrap_or(0)
            )
        }
    }

    #[implement(IUIAutomationEventHandler)]
    struct Selected(Instant);
    impl IUIAutomationEventHandler_Impl for Selected_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: Ref<IUIAutomationElement>,
            _id: UIA_EVENT_ID,
        ) -> windows_core::Result<()> {
            if let Some(e) = sender.as_ref() {
                println!("{:>7} ms  selected: {}", self.0.elapsed().as_millis(), describe(e));
            }
            Ok(())
        }
    }

    #[implement(IUIAutomationFocusChangedEventHandler)]
    struct Focus(Instant, u32);
    impl IUIAutomationFocusChangedEventHandler_Impl for Focus_Impl {
        fn HandleFocusChangedEvent(&self, sender: Ref<IUIAutomationElement>) -> windows_core::Result<()> {
            if let Some(e) = sender.as_ref() {
                let pid = unsafe { e.CurrentProcessId() }.unwrap_or(0) as u32;
                let tag = if pid == self.1 { "focus(WT)" } else { "focus(other)" };
                println!("{:>7} ms  {tag}: {}", self.0.elapsed().as_millis(), describe(e));
            }
            Ok(())
        }
    }

    pub fn run(window: &IUIAutomationElement, seconds: u64) -> windows_core::Result<()> {
        let started = Instant::now();
        let automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)? };
        let wt_pid = unsafe { window.CurrentProcessId()? } as u32;
        let selected: IUIAutomationEventHandler = Selected(started).into();
        let focus: IUIAutomationFocusChangedEventHandler = Focus(started, wt_pid).into();
        unsafe {
            automation.AddAutomationEventHandler(
                UIA_SelectionItem_ElementSelectedEventId,
                window,
                TreeScope_Subtree,
                None,
                &selected,
            )?;
            automation.AddFocusChangedEventHandler(None, &focus)?;
        }
        println!("listening {seconds}s on WT pid {wt_pid} (subscribe took {} ms)", started.elapsed().as_millis());
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        unsafe { automation.RemoveAllEventHandlers() }
    }
}

// Event-driven alternative to polling: tab selection and focus changes.
// Listens on the first window that has a tab whose name contains `needle`.
fn watch_events(automation: &UIAutomation, seconds: u64, needle: &str) -> Res<()> {
    for w in terminal_windows(automation)? {
        let names: Vec<String> = tabs(automation, &w)?.iter().map(|t| t.get_name().unwrap_or_default()).collect();
        if !names.iter().any(|n| n.contains(needle)) {
            continue;
        }
        println!("window: {}", w.get_name().unwrap_or_default());
        return events::run(w.as_ref(), seconds).map_err(to_err);
    }
    println!("no Windows Terminal window");
    Ok(())
}

// Win32-only responsiveness check of the test Terminal's windows: no UIA,
// so it can't block on a hung provider.
fn hung() {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsHungAppWindow, SendMessageTimeoutW, SMTO_ABORTIFHUNG,
        SMTO_BLOCK, WM_NULL,
    };
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, &mut class) };
        if String::from_utf16_lossy(&class[..n as usize]) == WT_WINDOW_CLASS {
            let list = unsafe { &mut *(found.0 as *mut Vec<HWND>) };
            list.push(hwnd);
        }
        BOOL(1)
    }
    let mut windows: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut windows as *mut _ as isize));
    }
    let dir = std::env::var("NT_PROBE_WT_DIR").unwrap_or_else(|_| TEST_TERMINAL_DIR.into()).to_lowercase();
    for hwnd in windows {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let image = process_image(pid).unwrap_or_default();
        if dir != "*" && !image.to_lowercase().starts_with(&format!("{dir}\\")) {
            continue;
        }
        let started = Instant::now();
        let hung = unsafe { IsHungAppWindow(hwnd) }.as_bool();
        let t_hung = started.elapsed().as_micros();
        let started = Instant::now();
        let mut result = 0usize;
        let answered = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                250,
                Some(&mut result),
            )
        };
        println!(
            "hwnd {:?} pid {pid}: IsHungAppWindow={hung} ({t_hung} us), WM_NULL answered={} ({} ms)",
            hwnd.0,
            answered.0 != 0,
            started.elapsed().as_millis()
        );
    }
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("hung") {
        hung();
        return Ok(());
    }
    let started = Instant::now();
    let automation = UIAutomation::new()?;
    // Short UIA timeouts, so a hung Terminal can't block the caller for long.
    if let Some(ms) = std::env::var("NT_UIA_TIMEOUT_MS").ok().and_then(|v| v.parse::<u32>().ok()) {
        use windows::core::Interface;
        use windows::Win32::UI::Accessibility::IUIAutomation2;
        let raw: &windows::Win32::UI::Accessibility::IUIAutomation = automation.as_ref();
        if let Ok(a2) = raw.cast::<IUIAutomation2>() {
            unsafe {
                let _ = a2.SetConnectionTimeout(ms);
                let _ = a2.SetTransactionTimeout(ms);
            }
            eprintln!("UIA timeouts set to {ms} ms");
        }
    }
    let result = run_command(&automation, &args);
    eprintln!("total {} ms, result {:?}", started.elapsed().as_millis(), result.as_ref().map(|_| ()));
    result
}

fn run_command(automation: &UIAutomation, args: &[String]) -> Res<()> {
    let automation = automation.clone();
    match args.first().map(String::as_str) {
        Some("select") => select(&automation, args.get(1).map(String::as_str).unwrap_or("")),
        Some("close") => close(&automation, args.get(1).map(String::as_str).unwrap_or("")),
        Some("all") => all_tabs(&automation),
        Some("select-any") => select_any(&automation, args.get(1).map(String::as_str).unwrap_or("")),
        Some("text") => text(&automation),
        Some("panes") => panes(&automation),
        Some("dump") => dump(&automation, args.get(1).and_then(|d| d.parse().ok()).unwrap_or(6)),
        Some("events") => watch_events(
            &automation,
            args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30),
            args.get(2).map(String::as_str).unwrap_or(""),
        ),
        Some("watch") => watch(&automation, args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30)),
        _ => {
            print!("{}", list(&automation)?);
            Ok(())
        }
    }
}
