# Messages from the configuration library: what it refuses to write, and
# why (see `native-term-config`). The values it quotes back — names,
# paths, ssh's own words — are not translated.

## a host or folder as the person typed it
config-hostname-blank = the address { $value } is empty or has spaces
config-name-one-line = the name must be one non-empty line
config-folder-name-one-line = the folder name must be one non-empty line
config-value-blank = { $what } { $value } is empty or has spaces
config-note-one-line = the note must be one line
config-login-one-line = the login command must be one line
config-credential-set = credential set { $value }: no spaces, quotes or / \ : * ?
config-persistent-host = persistent session { $value }: tmux, tmux-log, screen or off
config-persistent-folder = persistent sessions { $value }: tmux, tmux-log or screen
config-tab-color-host = tab color { $value }: a color name, #RRGGBB or none
config-tab-color-folder = tab color { $value }: a color name or #RRGGBB
config-color-scheme-host = color scheme { $value }: a scheme name or none
config-color-scheme-folder = color scheme { $value }
config-port-zero = port 0
config-name-taken = { $name } is taken

## what an operation is for, and what it isn't
config-not-ssh = { $alias } is not an ssh host
config-is-ssh = { $alias } is an ssh host
config-folder-options-main = folder options are for folder files, not the main config
config-main-no-folder-options = the main config has no folder settings

## moving the folder of session files
config-not-full-path = not a full path
config-folders-here = that is where the folders are
config-folders-already = { $path } already holds session folders
config-folders-none = { $path } holds no session folders
config-file-unreadable = { $path }: { $error }

## something that was there when it was read, and isn't now
config-missing = { $what } not found (changed outside NativeTerm?)
config-missing-host = host { $alias }
config-missing-host-in = host { $alias } in { $path }
config-missing-session = session { $name }

## writing
config-write-conflict = the file was changed by someone else since it was read
config-write-rejected = ssh rejected the change, previous version restored: { $reason }
config-include-wildcard = { $pattern }: a wildcard in a directory name is not followed
