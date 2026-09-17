//! NativeTerm's tab menu against a portable Windows Terminal (setup as in
//! `core_portable.rs`). Moves the mouse and types: don't touch the machine
//! while it runs.
//!
//! ```text
//! cargo test -p native-term-app --test menu_portable -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use native_term_app::{tab_menu, Core, HostRequest, SessionView, State};
use native_term_platform::windows_terminal::install::{Install, Kind};
use native_term_platform::windows_terminal::{launch, WindowsTerminal};
use native_term_platform::{Rect, Target};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY, VK_DOWN, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos, SetForegroundWindow};

const WAIT: Duration = Duration::from_secs(30);

fn core() -> Core {
    // tab rectangles and the cursor in physical pixels, as in NativeTerm
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
    let dir = std::env::var("NATIVETERM_TEST_WT_DIR").expect("set NATIVETERM_TEST_WT_DIR to a portable Terminal");
    let install = Install::from_dir(dir.as_ref()).unwrap();
    assert_eq!(install.kind, Kind::Portable, "only a portable Terminal is used for tests");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
    let shim = root.join("target").join("debug").join("nativeterm-shim.exe");
    Core::start(WindowsTerminal::new(install, &shim), None).expect("is a NativeTerm running?")
}

fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < WAIT, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("{what}: {} ms", started.elapsed().as_millis());
}

fn send(inputs: &[INPUT]) {
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    std::thread::sleep(Duration::from_millis(60));
}

fn mouse(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } } }
}

fn key(vk: VIRTUAL_KEY) {
    let down = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, ..Default::default() } } };
    let up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: KEYEVENTF_KEYUP, ..Default::default() } },
    };
    send(&[down, up]);
}

fn right_click(rect: Rect) {
    let (x, y) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
    unsafe {
        let mut saved = POINT::default();
        let _ = GetCursorPos(&mut saved);
        let _ = SetCursorPos(x, y);
        std::thread::sleep(Duration::from_millis(80));
        send(&[mouse(MOUSEEVENTF_RIGHTDOWN), mouse(MOUSEEVENTF_RIGHTUP)]);
        std::thread::sleep(Duration::from_millis(300));
        let _ = SetCursorPos(saved.x, saved.y);
    }
}

fn session<'a>(sessions: &'a [SessionView], label: &str) -> &'a SessionView {
    sessions.iter().find(|s| s.label == label).unwrap()
}

/// The tab's rectangle and the window, from a fresh snapshot.
fn tab_rect(core: &Core, name: &str) -> (isize, Rect) {
    let started = Instant::now();
    loop {
        let snapshot = core.snapshot();
        let found = snapshot.windows.iter().find_map(|w| {
            w.tabs.iter().find(|t| t.name == name).and_then(|t| t.rect.map(|r| (w.handle, r)))
        });
        if let Some(found) = found {
            return found;
        }
        assert!(started.elapsed() < WAIT, "no rectangle for {name}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore = "needs a portable Windows Terminal; moves the mouse"]
fn tab_menu_on_nativeterm_tabs_only() {
    let core = core();
    core.start_tab_menu(|_| {}).unwrap();
    let hosts: Vec<HostRequest> = ["m a", "m b", "m c"]
        .iter()
        .map(|l| HostRequest::new("nativeterm-test.invalid", *l))
        .collect();
    core.open(&hosts, Target::NewWindow);
    wait_until("three tabs failed to log in and were located", || {
        let s = core.sessions();
        s.len() == 3 && s.iter().all(|s| matches!(s.state, State::LoginFailed(_)) && s.location.is_some())
    });
    // one of the user's own tabs, in the same window
    launch::run(
        &core.terminal().install().launcher,
        &["-w".into(), "0".into(), "new-tab".into(), "--title=user tab".into(), "cmd".into()],
    )
    .unwrap();
    let (window, user_rect) = tab_rect(&core, "user tab");
    unsafe {
        let _ = SetForegroundWindow(windows::Win32::Foundation::HWND(window as *mut _));
    }
    std::thread::sleep(Duration::from_millis(500));

    // our menu on our tab; Esc closes it
    let (_, b_rect) = tab_rect(&core, "m b");
    right_click(b_rect);
    wait_until("menu open on m b", || core.tab_menu_state() == Some((1, true)));
    key(VK_ESCAPE);
    wait_until("menu closed by Esc", || core.tab_menu_state() == Some((1, false)));

    // the user's own tab keeps Terminal's menu
    right_click(user_rect);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(core.tab_menu_state(), Some((1, false)), "no NativeTerm menu on the user's tab");
    key(VK_ESCAPE); // Terminal's own menu

    // "Close Tabs to the Right" on m a
    let (_, a_rect) = tab_rect(&core, "m a");
    right_click(a_rect);
    wait_until("menu open on m a", || core.tab_menu_state() == Some((2, true)));
    // the keyboard moves the highlight over enabled items only
    key(VK_DOWN);
    wait_until("an item highlighted", || core.tab_menu_hovered().is_some());
    let first = core.tab_menu_hovered().unwrap();
    key(VK_DOWN);
    wait_until("the highlight moved", || core.tab_menu_hovered().is_some_and(|h| h != first));
    assert_ne!(core.tab_menu_hovered(), Some(tab_menu::DISCONNECT), "disabled for an ended session");
    core.tab_menu_choose(tab_menu::CLOSE_RIGHT);
    wait_until("m b and m c closed", || {
        let s = core.sessions();
        session(&s, "m b").state == State::Closed && session(&s, "m c").state == State::Closed
    });
    let s = core.sessions();
    assert!(session(&s, "m a").state.is_open(), "the tab itself stays");
    std::thread::sleep(Duration::from_secs(1));
    let names: Vec<String> = core.terminal().snapshot(&["m a".to_string()].into_iter().collect()).windows
        .iter()
        .filter(|w| w.handle == window)
        .flat_map(|w| w.tabs.iter().map(|t| t.name.clone()))
        .collect();
    assert_eq!(names, ["m a", "user tab"], "the user's tab to the right is untouched");

    // clean up: our tab through the core, the user's tab with its close button
    core.close(&session(&s, "m a").id);
    let snapshot = core.terminal().snapshot(&Default::default());
    if let Some((w, t)) = snapshot.windows.iter().find_map(|w| w.tabs.iter().find(|t| t.name == "user tab").map(|t| (w, t))) {
        let _ = core.terminal().close(w.handle, t);
    }
    wait_until("window closed", || !core.terminal().windows().iter().any(|w| w.handle == window));
}
