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
agent-section = SSH keys and ssh-agent
agent-other = SSH_AUTH_SOCK is set: ssh uses that agent.
agent-checking = Checking…
agent-service = ssh-agent service: { $state }
agent-running = running
agent-stopped = stopped
agent-disabled = disabled (Windows' default)
agent-missing = not installed
agent-no-keys = No key pairs in { $dir }.
agent-key-protected = has a passphrase
agent-key-open = no passphrase
agent-key-unknown = can't tell
agent-loaded = The agent holds keys: connects don't ask for passphrases.
agent-empty = The agent holds no keys yet.
agent-unreachable = The agent doesn't answer.
agent-add = Add my keys to the agent…
agent-add-tab = ssh-add
agent-why = A key has a passphrase, so every connect asks for it. ssh-agent can remember it until you sign out. To turn the service on, run this in PowerShell as administrator, then use "Add my keys":
agent-copy = Copy
agent-admin = NativeTerm doesn't change Windows services itself.
agent-hint = Your key has a passphrase but ssh-agent isn't running: every connect will ask for it.
agent-hint-show = How to fix
agent-hint-dismiss = Don't show again
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
tree-favorites = Favorites
menu-favorite = Add to Favorites
menu-unfavorite = Remove from Favorites
tree-recent = Recent
tree-all = All sessions
tree-empty = No hosts yet: right-click a folder, or add a folder first.
tree-main-config = ~/.ssh/config
fab-open = NativeTerm: connect, tabs, sessions
fab-close = Close (Esc)
fab-search-hint = Host or user@host[:port]
fab-active = Active session: { $label }
fab-no-active = No NativeTerm session in the current Terminal tab.
fab-show-main = Show NativeTerm
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
menu-install-key = Install My Key…
menu-install-key-all = Install My Key on All…
key-title = Install my key
key-none = There is no public key in { $dir } yet.
key-create = Create a key…
key-create-tab = Create a key
key-refresh = Look again
key-which = Public key
key-hosts = On { $count } hosts: { $names }{ $more ->
        [0] {""}
       *[other] {" "}and { $more } more
    }
key-note = Each host gets a tab, where ssh asks for its password one last time. Hosts without a POSIX shell (many network devices) can't take a key this way.
key-install = { $count ->
        [one] Install
       *[other] Install on { $count } hosts
    }
key-tab = Key → { $label }
menu-options = Session Options…
options-title = Session options: { $label }
options-connection = Connection
options-authentication = Authentication
options-algorithms = Algorithms
options-host-key = Host key
options-forwarding = Port forwarding
options-environment = Environment
options-default = Default ({ $value })
options-choose = Choose…
options-use-default = Use the default
options-no-names = This ssh doesn't list them.
options-note = Written into the Host { $alias } block of your ssh config, so plain ssh, scp and VS Code use them too. Empty fields keep what ssh uses now (greyed).
options-forward-agent-note = Agent forwarding lets the remote host use your keys while you are connected; turn it on only for hosts you trust.
options-algorithms-note = A list replaces the default; start it with + to add to the default (e.g. +ssh-rsa for old devices), - to remove, ^ to put first.
options-forwarding-note = One forward per line. X11 forwarding needs an X server on this PC.
opt-connect-timeout = Connect timeout (s)
opt-server-alive-interval = Keepalive interval (s)
opt-server-alive-count-max = Keepalive misses
opt-tcp-keep-alive = TCP keepalive
opt-compression = Compression
opt-address-family = IP version
opt-request-tty = Terminal (TTY)
opt-remote-command = Remote command
opt-log-level = Log level
opt-preferred-authentications = Methods, in order
opt-pubkey-authentication = Public key
opt-password-authentication = Password
opt-kbd-interactive-authentication = Keyboard-interactive
opt-identities-only = Only the listed keys
opt-forward-agent = Agent forwarding
opt-pubkey-accepted-algorithms = Key signature algorithms
opt-kex-algorithms = Key exchange
opt-ciphers = Ciphers
opt-macs = MACs
opt-host-key-algorithms = Host key algorithms
opt-strict-host-key-checking = Unknown host keys
opt-update-host-keys = Learn new host keys
opt-check-host-ip = Check the IP too
opt-local-forward = Local (-L)
opt-remote-forward = Remote (-R)
opt-dynamic-forward = SOCKS (-D)
opt-exit-on-forward-failure = Fail if a forward fails
opt-gateway-ports = Allow other PCs
opt-forward-x11 = X11 forwarding
opt-forward-x11-trusted = Trusted X11
opt-set-env = Set variables
opt-send-env = Send variables
menu-forget-key = Forget Host Key…
forget-title = Forget host key
forget-question = Remove the saved host keys of { $alias }? ssh will ask to confirm the new key on the next connect.
forget-note = Only do this when the host really changed (reinstalled, new address). A copy is kept as known_hosts.old.
forget-button = Remove
forget-done = Removed the saved host keys of { $names }
forget-none = { $alias } had no saved host key
menu-delete = Delete…
host-via = via { $jump }
host-alias = alias { $alias }

## Open sessions

sessions-heading = Open sessions ({ $count })
view-tabs = All tabs
view-tabs-hint = Every tab in every Windows Terminal window, yours too (Ctrl+T)
tabs-search-hint = Search tab titles…  (Ctrl+T)
tabs-none = No Windows Terminal tabs found.
tabs-no-match = No tab matches.
tabs-window =
    { $count ->
        [one] Window { $number } · 1 tab
       *[other] Window { $number } · { $count } tabs
    }
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
session-renamed = now called { $name }
session-renamed-hint = The host was renamed after this tab opened. Windows Terminal keeps the tab's title; clones and reopened tabs use the new name.
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
notice-tab-gone = The tab “{ $title }” isn't there any more
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
notice-agent-forwarding = { $count } of the { $total } hosts just opened forward your ssh-agent ({ $names }…): while connected, anyone with root there can use your keys. Turn it off in Session Options → Authentication where it isn't needed.
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
field-on-login = After login
field-on-login-hint = typed after every login, e.g. sudo -i
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
summary-host-keys = Host keys: { $count } (added to known_hosts unless already there)
summary-host-keys-unknown = Host key files that weren't understood (left out): { $count }
import-keys-added = Added { $count } host keys to known_hosts.
import-keys-failed = known_hosts was not changed: { $error }
summary-not-yet = Not imported yet: saved commands

## Tab menu (on NativeTerm's tabs in Windows Terminal)

tabmenu-header-mixed = { $label } · this tab has other panes too
tabmenu-header-titled = { $label } · titled “{ $title }”
tabmenu-connect = Connect
tabmenu-reconnect = Reconnect
tabmenu-disconnect = Disconnect
tabmenu-clone = Clone Session
tabmenu-lock = Lock
tabmenu-unlock = Unlock
session-lock = Lock
session-unlock = Unlock
session-locked = Locked
session-lock-hint = A locked session is left out of "close disconnected", "close others", "close to the right" and group sends, and must be unlocked before closing it here.
send-locked = locked: not ticked by "All"
tabmenu-close = Close
tabmenu-close-mixed = Close This Session (keeps the other panes)
tabmenu-close-others = Close Other NativeTerm Tabs
tabmenu-close-disconnected = Close Disconnected Tabs
tabmenu-close-right = Close Tabs to the Right

## Sending commands

send-title = Send command
send-command-label = Command (one per line)
send-enter = Press Enter after the last line
send-saved = Saved commands
send-saved-pick = (choose)
send-saved-none = No saved commands yet.
send-save-as = Save as
send-save = Save
send-delete = Delete this saved command
send-library-error = { $path } can't be read ({ $error }); it isn't changed.
send-targets = Sessions
send-all = All logged in
send-none = None
send-no-sessions = No open sessions.
send-not-logged-in = not logged in ({ $state })
send-button =
    { $count ->
        [one] Send
       *[other] Send to { $count } sessions
    }
send-confirm = Really type this into { $count } sessions?
send-back = Back
send-result-sent = Sent to { $count }: { $labels }
send-result-skipped = Not sent (not logged in): { $labels }
send-result-failed = Not sent (NativeTerm lost the tab): { $labels }
session-send = Send…
sessions-send-many = Send to several…
fab-send-hint = Type into the active session, Enter sends
tabmenu-send = Send Command…
