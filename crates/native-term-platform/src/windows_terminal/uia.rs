//! Reading and acting on one Terminal window through UI Automation. Only
//! call these on a worker thread (see `worker`), for windows that answered
//! the `WM_NULL` probe.

use uiautomation::patterns::{UIInvokePattern, UISelectionItemPattern};
use uiautomation::types::{ControlType, Handle};
use uiautomation::{UIAutomation, UIElement, UITreeWalker};
use windows::core::Interface;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationItemContainerPattern, IUIAutomationSelectionItemPattern,
    IUIAutomationTextPattern, IUIAutomationVirtualizedItemPattern, UIA_ItemContainerPatternId,
    UIA_SelectionItemPatternId, UIA_TextPatternId, UIA_VirtualizedItemPatternId, UIA_PROPERTY_ID,
};

use super::window::hwnd;
use crate::claim::{Pane, WindowTabs};
use crate::Rect;

pub type Result<T> = uiautomation::Result<T>;

fn to_error(e: windows::core::Error) -> uiautomation::Error {
    uiautomation::Error::new(e.code().0, &e.message())
}

struct Found {
    /// Realized tab items, in strip order.
    tabs: Vec<UIElement>,
    panes: Vec<Pane>,
    list: Option<UIElement>,
}

// FindAll doesn't cross into Terminal's XAML island: walk the control view.
fn walk(walker: &UITreeWalker, element: &UIElement, found: &mut Found) {
    let mut child = walker.get_first_child(element).ok();
    while let Some(c) = child {
        match c.get_control_type() {
            Ok(ControlType::TabItem) => found.tabs.push(c.clone()),
            kind => {
                if kind == Ok(ControlType::List) && found.list.is_none() {
                    found.list = Some(c.clone());
                }
                if c.get_classname().is_ok_and(|n| n == "TermControl") {
                    found.panes.push(Pane {
                        title: c.get_help_text().unwrap_or_default(),
                        profile: c.get_name().unwrap_or_default(),
                    });
                } else {
                    walk(walker, &c, found);
                }
            }
        }
        child = walker.get_next_sibling(&c).ok();
    }
}

fn window_element(automation: &UIAutomation, handle: isize) -> Result<UIElement> {
    automation.element_from_handle(Handle::from(hwnd(handle)))
}

fn rect_of(element: &UIElement) -> Option<Rect> {
    let r = element.get_bounding_rectangle().ok()?;
    let rect = Rect { left: r.get_left(), top: r.get_top(), right: r.get_right(), bottom: r.get_bottom() };
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

fn is_selected(tab: &UIElement) -> bool {
    tab.get_pattern::<UISelectionItemPattern>().and_then(|p| p.is_selected()).unwrap_or(false)
}

/// Every tab item of the list in order, including virtualized ones.
fn all_items(list: &UIElement) -> Result<Vec<IUIAutomationElement>> {
    let raw: &IUIAutomationElement = list.as_ref();
    let container: IUIAutomationItemContainerPattern =
        unsafe { raw.GetCurrentPatternAs(UIA_ItemContainerPatternId) }.map_err(to_error)?;
    let empty = VARIANT::default();
    let mut items = Vec::new();
    let mut previous: Option<IUIAutomationElement> = None;
    while let Ok(item) = unsafe { container.FindItemByProperty(previous.as_ref(), UIA_PROPERTY_ID(0), &empty) } {
        items.push(item.clone());
        previous = Some(item);
    }
    Ok(items)
}

fn name_of(item: &IUIAutomationElement) -> String {
    unsafe { item.CurrentName() }.map(|s| s.to_string()).unwrap_or_default()
}

/// The text on the screen of the window's selected tab: Terminal's
/// control offers `TextPattern`, and only the selected tab has one (the
/// others aren't rendered). The visible ranges are the screen, without
/// the scrollback, which is what a preview wants; at most `max_lines`
/// lines, the last ones, each trimmed of its trailing blanks.
///
/// This is how a tab NativeTerm doesn't run — a local shell, someone's
/// AI session — can still show what was on it and be searched by it.
pub fn screen_text(automation: &UIAutomation, handle: isize, max_lines: usize) -> Result<Vec<String>> {
    let window = window_element(automation, handle)?;
    let walker = automation.get_control_view_walker()?;
    let control = find_text_control(&walker, &window).ok_or_else(|| {
        uiautomation::Error::new(windows::Win32::Foundation::E_FAIL.0, "no terminal control with text")
    })?;
    let raw: &IUIAutomationElement = control.as_ref();
    let pattern: IUIAutomationTextPattern = unsafe { raw.GetCurrentPatternAs(UIA_TextPatternId) }.map_err(to_error)?;
    // the visible screen, one range per text buffer region
    let ranges = unsafe { pattern.GetVisibleRanges() }.map_err(to_error)?;
    let count = unsafe { ranges.Length() }.map_err(to_error)?;
    let mut text = String::new();
    for i in 0..count {
        let range = unsafe { ranges.GetElement(i) }.map_err(to_error)?;
        // -1: the whole range
        if let Ok(part) = unsafe { range.GetText(-1) } {
            text.push_str(&part.to_string());
        }
    }
    let lines: Vec<String> = text.replace('\r', "\n").lines().map(|line| line.trim_end().to_string()).collect();
    // the last lines that hold anything
    let end = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
    let start = end.saturating_sub(max_lines);
    Ok(lines[start..end].to_vec())
}

/// The first element under `element` that offers `TextPattern` (the
/// selected tab's terminal control).
fn find_text_control(walker: &UITreeWalker, element: &UIElement) -> Option<UIElement> {
    let mut child = walker.get_first_child(element).ok();
    while let Some(c) = child {
        if c.get_classname().is_ok_and(|n| n == "TermControl") {
            let raw: &IUIAutomationElement = c.as_ref();
            if unsafe { raw.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }.is_ok() {
                return Some(c);
            }
        } else if let Some(found) = find_text_control(walker, &c) {
            return Some(found);
        }
        child = walker.get_next_sibling(&c).ok();
    }
    None
}

pub fn read_window(automation: &UIAutomation, handle: isize) -> Result<WindowTabs> {
    let window = window_element(automation, handle)?;
    let walker = automation.get_control_view_walker()?;
    let mut found = Found { tabs: Vec::new(), panes: Vec::new(), list: None };
    walk(&walker, &window, &mut found);

    let realized: Vec<String> = found.tabs.iter().map(|t| t.get_name().unwrap_or_default()).collect();
    let names = match found.list.as_ref().map(all_items) {
        Some(Ok(items)) if items.len() >= realized.len() => items.iter().map(name_of).collect(),
        _ => realized.clone(),
    };

    // realized tabs are in strip order: map them onto the full list
    let mut rects = vec![None; names.len()];
    let mut selected = None;
    let mut k = 0;
    for (tab, name) in found.tabs.iter().zip(&realized) {
        while k < names.len() && &names[k] != name {
            k += 1;
        }
        if k >= names.len() {
            break;
        }
        rects[k] = rect_of(tab);
        if is_selected(tab) {
            selected = Some(k);
        }
        k += 1;
    }
    Ok(WindowTabs { names, rects, selected, panes: found.panes })
}

/// The tab at `index` if it still has the expected name, realized.
fn item_at(automation: &UIAutomation, handle: isize, index: usize, name: &str) -> Result<Option<IUIAutomationElement>> {
    let window = window_element(automation, handle)?;
    let walker = automation.get_control_view_walker()?;
    let mut found = Found { tabs: Vec::new(), panes: Vec::new(), list: None };
    walk(&walker, &window, &mut found);
    let Some(list) = found.list else { return Ok(None) };
    let Some(item) = all_items(&list)?.into_iter().nth(index) else { return Ok(None) };
    if name_of(&item) != name {
        return Ok(None);
    }
    if let Ok(virtualized) =
        unsafe { item.GetCurrentPatternAs::<IUIAutomationVirtualizedItemPattern>(UIA_VirtualizedItemPatternId) }
    {
        let _ = unsafe { virtualized.Realize() };
    }
    Ok(Some(item))
}

/// Select a tab (even one scrolled away). False if it isn't there any more.
pub fn select_tab(automation: &UIAutomation, handle: isize, index: usize, name: &str) -> Result<bool> {
    let Some(item) = item_at(automation, handle, index, name)? else { return Ok(false) };
    let selection: IUIAutomationSelectionItemPattern =
        unsafe { item.GetCurrentPatternAs(UIA_SelectionItemPatternId) }.map_err(to_error)?;
    unsafe { selection.Select() }.map_err(to_error)?;
    Ok(true)
}

/// Close a tab with its close button (a fallback: normally the shim exits).
pub fn close_tab(automation: &UIAutomation, handle: isize, index: usize, name: &str) -> Result<bool> {
    let Some(item) = item_at(automation, handle, index, name)? else { return Ok(false) };
    let tab = UIElement::from(item.cast::<IUIAutomationElement>().map_err(to_error)?);
    let walker = automation.get_control_view_walker()?;
    // the button's name is localized: find it by type
    let mut child = walker.get_first_child(&tab).ok();
    while let Some(c) = child {
        if c.get_control_type() == Ok(ControlType::Button) {
            c.get_pattern::<UIInvokePattern>()?.invoke()?;
            return Ok(true);
        }
        child = walker.get_next_sibling(&c).ok();
    }
    Ok(false)
}
