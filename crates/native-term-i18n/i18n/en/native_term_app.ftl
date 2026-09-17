# NativeTerm's window. Keep product names ("NativeTerm SSH", Windows
# Terminal, SecureCRT, ssh) and file names untranslated.

## Top bar and settings

settings-toggle = Settings
import-securecrt-button = Import from SecureCRT…
language-label = Language
language-system = System default
theme-label = Theme
theme-system = System default
theme-light = Light
theme-dark = Dark
auto-reconnect-setting = Reconnect dropped sessions automatically (never after a failed login)
dock-pin = Pin
dock-pin-hint = Docked at the { $edge ->
        [top] top
        [left] left
       *[right] right
    } edge: it slides away when the pointer leaves, unless pinned. Drag it away from the edge to undock.
settings-terminal = Windows Terminal: { $dir } ({ $kind })
terminal-kind-packaged = Store package
terminal-kind-portable = portable
terminal-kind-unpackaged = unpackaged
settings-profile = "NativeTerm SSH" profile: { $status }

## Session tree

tree-heading = Sessions
tree-reload-hint = Reload ~/.ssh
tree-new-folder = Folder
tree-search-hint = Search name, host, user, note…  (Ctrl+F)
tree-search-clear = Clear the search (Esc)
tree-no-match = No host matches "{ $query }"
tree-recent = Recent
tree-all = All sessions
tree-empty = No hosts yet: right-click a folder, or add a folder first.
tree-main-config = ~/.ssh/config
quick-connect = Connect to { $target }
quick-save = Save…
quick-save-hint = Save as a host in ~/.ssh/config
menu-connect-all = Connect All
menu-connect-all-new-window = Connect All in New Window
menu-new-host = New Host…
menu-rename-folder = Rename Folder…
menu-connect = Connect
menu-connect-new-window = Connect in New Window
menu-edit = Edit…
menu-move-to = Move to
menu-delete = Delete…
host-via = via { $jump }
host-alias = alias { $alias }

## Open sessions

sessions-heading = Open sessions ({ $count })
sessions-clear-finished = Clear finished
sessions-restored-waiting =
    { $count ->
        [one] One restored session is waiting to connect.
       *[other] { $count } restored sessions are waiting to connect.
    }
sessions-connect-all = Connect all
sessions-close-all = Close all
sessions-one-by-one = Or connect them one by one below.
sessions-empty = Double-click a host, or right-click it or a folder for more.
session-attempt = attempt { $n }
session-auto-reconnect = auto-reconnect { $n }
session-location = window { $window } · tab { $tab }
session-selected = selected
session-split = split
session-current-title = current title: { $title }
session-not-located = not located
button-focus = Focus
button-connect = Connect
button-reconnect = Reconnect
button-disconnect = Disconnect
button-close = Close
button-save = Save
button-cancel = Cancel
button-delete = Delete
button-check-again = Check again

## Session states

state-opening = opening
state-detached = looking for its tab…
state-waiting = restored, not connected
state-connecting = connecting / waiting for login
state-connected = connected
state-login-failed = login failed ({ $code })
state-disconnected = disconnected ({ $code })
state-ended = ended ({ $code })
state-failed = failed: { $reason }
state-gone = tab gone
state-closed = closed

## Notices

notice-lost-sessions =
    { $count ->
        [one] One session from the last run has no tab any more
       *[other] { $count } sessions from the last run have no tab any more
    }
notice-tab-not-found = { $label }: its tab wasn't found (a split tab is found once it's selected)
notice-tab-changed = { $label }: the tab changed, not closed
notice-not-linked = { $label }: its tab isn't connected to NativeTerm
notice-terminal-failed = Windows Terminal could not be started: { $error }
notice-tabs-pending = { $count } tabs were not opened: another Terminal window became active
notice-tabs-missing = { $count } tabs didn't appear in Windows Terminal: { $labels }
notice-pipe-stopped = NativeTerm's pipe server stopped: { $error }
notice-old-shim = A shim with protocol { $protocol } connected (expected { $expected }); update it
notice-gave-up = { $label }: gave up reconnecting after { $tries } tries
notice-db-unavailable = { $path }: { $error }; open sessions won't be remembered
notice-no-data-dir = No data directory: { $error }; open sessions won't be remembered
notice-shim-missing = { $path } is missing; tabs can't start
notice-no-pipe = NativeTerm can't serve its pipe: { $error }
notice-tab-menu-unavailable = NativeTerm's tab menu isn't available: { $error }
notice-move-failed = Moving { $alias } failed: { $error }
error-host-gone = { $alias } is gone (changed outside NativeTerm?)
fatal-title = NativeTerm can't start
fatal-window = NativeTerm's window can't start: { $error }

## "NativeTerm SSH" profile

profile-updated = Updated the "NativeTerm SSH" profile: { $old } moved to { $new }
profile-update-failed = Could not update the "NativeTerm SSH" profile: { $error }
profile-installed = installed (fragment)
profile-in-settings = defined in this Terminal's settings.json
profile-outdated = points at { $path }
profile-disabled = turned off on Terminal's Extensions page
profile-missing = not installed
profile-banner-disabled = The "NativeTerm SSH" profile is turned off in Windows Terminal (Settings → Extensions → NativeTerm).
profile-banner-missing = Windows Terminal doesn't have the "NativeTerm SSH" profile yet; tabs can't open.
profile-install = Install profile
profile-install-hint = Writes { $path } (read by every Windows Terminal of this user)
profile-install-update = Install / update
profile-remove = Remove
profile-install-failed = Installing the profile failed: { $error }
profile-remove-failed = Removing the profile failed: { $error }
profile-no-localappdata = LOCALAPPDATA is not set

## Host and folder dialogs

host-new-title = New host in { $folder }
host-edit-title = Edit { $alias }
host-bad-port = port "{ $port }" is not a number from 1 to 65535
field-name = Name
field-name-hint = shown in the tree and on the tab
field-host = Host
field-host-hint = host name or address
field-user = User
field-user-hint = (ssh default)
field-port = Port
field-jump = Jump host
field-jump-hint = e.g. bastion or user@bastion:22
field-keys = Keys
field-keys-hint = one IdentityFile per line, e.g. ~/.ssh/id_ed25519
field-note = Note
field-note-hint = one line
host-alias-kept = ssh alias: { $alias } (kept, so tabs and scripts keep working)
folder-new-title = New folder
folder-rename-title = Rename { $name }
folder-name-hint = folder name
delete-title = Delete host
delete-question = Delete "{ $label }" ({ $alias }) from the ssh config?
delete-backup-note = A backup of the file is kept in the data directory.

## SecureCRT import

import-title = Import from SecureCRT
import-folder-label = SecureCRT config folder
import-into = Into: { $path }  (every changed file is backed up first)
import-not-found = SecureCRT's config folder wasn't found; enter it above.
import-preview = Preview
import-run =
    { $count ->
        [one] Import 1 host
       *[other] Import { $count } hosts
    }
import-checking = Each folder is checked with ssh -G after writing.
import-done = Imported { $hosts } hosts into { $folders } folders.
import-folder-failed = Folder { $folder } was not written: { $error }
import-failed = Import failed: { $error }
summary-found = { $sessions } sessions found in { $folders } folders; { $hosts } will be imported into { $new } new and { $existing } existing folders
summary-skipped = Skipped, { $reason }: { $count }
skip-already = already imported
skip-plink-later = { $protocol }: not supported yet (planned through plink)
skip-protocol = { $protocol }: not a terminal session NativeTerm opens
skip-no-hostname = no host name
summary-duplicates = Same host, port and user in several sessions: { $count } groups (all imported; review them)
summary-firewall = Firewall/proxy “{ $name }” isn't imported: { $count } sessions connect directly
summary-unresolved-jumps = Jump session not found or not imported: { $count } sessions connect directly
summary-logon-actions = Logon actions/scripts aren't imported: { $count } sessions
summary-saved-passwords = Saved passwords aren't imported ({ $count } sessions); use keys or ssh-agent
summary-encodings = Non-UTF-8 character sets (OpenSSH sessions are UTF-8): { $count } sessions
summary-bad-forwards = Unreadable port forwards left out: { $count } sessions
summary-also = Also imported: { $items }
summary-forwards = { $count } port forwards
summary-keys = { $count } key files (must be in OpenSSH format)
summary-descriptions = { $count } multi-line descriptions joined into one line
summary-unreadable = Unreadable session files: { $count }
summary-not-utf8 = Session files that aren't UTF-8 (read with replacement characters): { $count }
summary-not-yet = Not imported yet: host keys (KnownHosts) and saved commands

## Tab menu (on NativeTerm's tabs in Windows Terminal)

tabmenu-header-mixed = { $label } · this tab has other panes too
tabmenu-header-titled = { $label } · titled “{ $title }”
tabmenu-connect = Connect
tabmenu-reconnect = Reconnect
tabmenu-disconnect = Disconnect
tabmenu-clone = Clone Session
tabmenu-close = Close
tabmenu-close-mixed = Close This Session (keeps the other panes)
tabmenu-close-others = Close Other NativeTerm Tabs
tabmenu-close-disconnected = Close Disconnected Tabs
tabmenu-close-right = Close Tabs to the Right
