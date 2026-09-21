# Files dropped into a tab

Dropping a file on a terminal tab and having it uploaded is what Tabby,
MobaXterm and Termius do. Their terminal is their own control, so they see
the drop itself. NativeTerm's tabs are Windows Terminal's, and a drop
target belongs to the process that owns the window: taking it would mean
injecting into WindowsTerminal.exe, which this program does not do (see
`ARCHITECTURE.md`).

What it does instead is take the drop's *result*. Terminal answers a drop
by pasting the file names into the tab as text, and that text passes
through our own client — ssh (`nativeterm/nt_drop.c` in the OpenSSH fork).

## How it works

1. **The client spots it.** Keyboard input whose every word is a path that
   exists on this machine (`C:\...` or `\\server\share`, a name with a
   space in quotes, as Terminal writes them) is held back instead of going
   to the server. Text the terminal brackets (`ESC [ 200 ~ … ESC [ 201 ~`,
   which a shell with bracketed paste gets) is collected first. Anything
   that is not paths goes on untouched, so typing is never taken for a
   drop.
2. **NativeTerm is told**: `nativeterm-shim --drop <path>...` sends the
   names over the pipe (`Role::Request`, `ShimMessage::Dropped`), and the
   tab is found by `WT_SESSION` as the files request is.
3. **The question**: what should happen — upload them, or type the names
   into the terminal after all? With "always do this" ticked the answer is
   kept (`drop.action` in `state.db`) and the question stops; the general
   settings have it back as "Ask about dropped files".
4. **Uploading** goes through the files window (`files_window::upload_into`),
   which is the transfer UI: it opens at the tab's folder (a tmux session's
   current folder, else the one last used for that host), shows progress,
   and can pause, resume and cancel. Nothing is drawn in the terminal, and
   a running command is not disturbed.
5. **Typing the names** sends the text with `AppMessage::SendText`, as
   "Send commands" does, with a marker in front (`ESC _ nt ESC \`) that the
   client strips: without it the names would arrive as another drop, for
   ever.

## A drop, or a paste?

The client cannot tell a drop from someone pasting a path — both arrive as
text. That is why the question is asked, and why a remembered answer is
used only when the mouse says it was a drop: NativeTerm's tab-menu hook
(`menu.rs`, already there for right-clicks) remembers when the left button
was last released over a *different* window than it went down on, which is
how a drag from Explorer ends and how a click never does. Without that
within two seconds, the question is asked however it was answered before.

## What is not covered

- **ntplink sessions** (Telnet, serial, raw): nothing is intercepted, so a
  drop pastes the names as it always did. There is no SFTP for these
  sessions, so there would be nothing to offer.
- **plink hosts**: the same, they do not run our ssh.
- Dropping onto the files window's own panes has worked all along (egui's
  drag and drop) and is unchanged.

## Verified live

Against an Ubuntu container over ssh, with the tab in `/srv/drop`:

- one file, and several at once including a name with a space (quoted, as
  Terminal writes it): uploaded there, SHA-256 equal;
- "type the names into the terminal": the path appears at the prompt, once,
  with no loop;
- a path that does not exist: passed through untouched, no question;
- a name with a space without quotes: passed through (it is not what a drop
  looks like);
- "always do this" and then a drag: uploaded with no question;
- the same text pasted from the keyboard afterwards: the question is asked
  again, because no drag ended over the tab.
