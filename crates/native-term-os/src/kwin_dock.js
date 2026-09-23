// Docking for NativeTerm's main window on KDE Plasma under Wayland, where
// no program may place its own window or see the pointer outside it.
// NativeTerm loads this into KWin (org.kde.kwin.Scripting) with its own
// process id in place of the marker below, and unloads it when it exits.
//
// The same behaviour as on Windows and X11 (native-term-app/src/dock.rs):
// a window dragged to the top, left or right edge of the work area docks
// there (the edge the pointer is at when the drag ends) and stays above
// other windows; 450 ms after the pointer leaves it, it slides away until
// only a 4 px strip is on the screen; the pointer touching the strip, or
// the window being activated, brings it back. Dragging it away undocks
// it. A window KWin tiled to a side stays tiled (a full-height side
// panel); one maximized by a drag to the top is unmaximized first.

const PID = __NATIVETERM_PID__;
const SNAP = 12;
const STRIP = 4;
const LEAVE_MS = 450;

let win = null;
let edge = null; // "top", "left", "right" or null
let hidden = false;
let shownAt = null;

const leave = new QTimer();
leave.interval = LEAVE_MS;
leave.singleShot = true;

function log(text) {
    if (__NATIVETERM_LOG__) print("nativeterm-dock: " + text);
}

function find() {
    const all = workspace.windowList();
    for (let i = 0; i < all.length; i++) {
        const w = all[i];
        if (w.pid === PID && w.normalWindow && w.caption === "NativeTerm") return w;
    }
    return null;
}

function area() {
    return workspace.clientArea(KWin.MaximizeArea, win);
}

function inside(g, x, y) {
    return x >= g.x && x < g.x + g.width && y >= g.y && y < g.y + g.height;
}

// The edge the pointer is at, within SNAP of the work area's edge, the
// nearest in a corner.
function edgeAtPointer() {
    const a = area();
    const p = workspace.cursorPos;
    const near = [
        ["left", p.x - a.x],
        ["right", a.x + a.width - 1 - p.x],
        ["top", p.y - a.y],
    ].filter(e => e[1] >= 0 && e[1] <= SNAP);
    near.sort((l, r) => l[1] - r[1]);
    return near.length ? near[0][0] : null;
}

function place(g) {
    win.frameGeometry = { x: g.x, y: g.y, width: g.width, height: g.height };
}

// Where the frame goes, docked at `edge`, shown or hidden.
function docked(g, away) {
    const a = area();
    const x = Math.min(Math.max(g.x, a.x), a.x + a.width - g.width);
    const y = Math.min(Math.max(g.y, a.y), a.y + a.height - g.height);
    switch (edge) {
        case "top": return { x: x, y: away ? a.y - g.height + STRIP : a.y, width: g.width, height: g.height };
        case "left": return { x: away ? a.x - g.width + STRIP : a.x, y: y, width: g.width, height: g.height };
        default: return { x: away ? a.x + a.width - STRIP : a.x + a.width - g.width, y: y, width: g.width, height: g.height };
    }
}

function dock(at) {
    edge = at;
    hidden = false;
    win.keepAbove = at !== null;
    if (at !== null) place(docked(win.frameGeometry, false));
    log("docked at " + at);
}

function hide() {
    if (edge === null || hidden) return;
    const p = workspace.cursorPos;
    if (inside(win.frameGeometry, p.x, p.y) || win.move || win.resize) return;
    shownAt = docked(win.frameGeometry, false);
    hidden = true;
    place(docked(win.frameGeometry, true));
    log("hidden");
}

function show() {
    if (!hidden) return;
    hidden = false;
    place(shownAt || docked(win.frameGeometry, false));
    log("shown");
}

function settled() {
    if (hidden) return;
    let at = edgeAtPointer();
    if (at === "top" && win.maximizeMode !== KWin.MaximizeRestore) {
        // KWin maximizes a window dragged to the top: its own size back
        win.setMaximize(false, false);
    }
    dock(at);
}

function attach(w) {
    win = w;
    win.interactiveMoveResizeFinished.connect(settled);
    win.closed.connect(() => { win = null; leave.stop(); });
    log("attached to " + win.caption);
}

leave.timeout.connect(hide);

workspace.cursorPosChanged.connect(() => {
    if (win === null || edge === null) return;
    const p = workspace.cursorPos;
    if (hidden) {
        // the strip, and the screen's edge pixel next to it
        const g = win.frameGeometry;
        const s = { x: g.x - 1, y: g.y - 1, width: g.width + 2, height: g.height + 2 };
        const a = area();
        const visible = {
            x: Math.max(s.x, a.x - 1), y: Math.max(s.y, a.y - 1),
            width: Math.min(s.x + s.width, a.x + a.width + 1) - Math.max(s.x, a.x - 1),
            height: Math.min(s.y + s.height, a.y + a.height + 1) - Math.max(s.y, a.y - 1),
        };
        if (inside(visible, p.x, p.y)) show();
    } else if (inside(win.frameGeometry, p.x, p.y)) {
        leave.stop();
    } else if (!leave.active) {
        leave.start();
    }
});

workspace.windowActivated.connect(w => {
    if (win !== null && w === win && hidden) show();
});

workspace.windowAdded.connect(w => {
    if (win === null && w.pid === PID && w.caption === "NativeTerm") attach(w);
});

const found = find();
if (found !== null) attach(found);
