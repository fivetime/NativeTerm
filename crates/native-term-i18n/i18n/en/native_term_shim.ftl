# Lines the shim prints into the terminal tab.

reopening = [NativeTerm] Reopening this restored session in a new tab…
restored = [NativeTerm] { $alias }: restored, not connected yet.
ssh-not-started = [NativeTerm] Could not start ssh ({ $path }): { $error }
login-failed = [NativeTerm] Login failed or cancelled (exit code { $code }).
disconnected = [NativeTerm] Disconnected (exit code { $code }).
ended = [NativeTerm] Session ended (exit code { $code }).
reconnect-or-close = [NativeTerm] Press R to reconnect, C to close this tab.
any-key = [NativeTerm] Press any key to close.
key-installing = [NativeTerm] Adding { $path } to { $alias }; ssh may ask for the password.
key-windows-host = { $alias } is a Windows host: adding the key the PowerShell way (ssh may ask for the password once more).
key-installed = [NativeTerm] Done: { $alias } now accepts the key.
key-present = [NativeTerm] { $alias } already had the key.
key-failed = [NativeTerm] The key was not added to { $alias } (exit code { $code }). Network devices without a POSIX shell can't take it this way.
key-unreadable = [NativeTerm] { $path } can't be used: { $error }
key-creating = [NativeTerm] Creating a key pair at { $path }. A passphrase protects it (ssh-agent can remember it).
key-created = [NativeTerm] Created. The public key is { $path }.
key-exists = [NativeTerm] { $path } exists already; nothing was changed.
key-create-failed = [NativeTerm] No key was created: { $error }
agent-adding = [NativeTerm] Adding your keys to ssh-agent; type each key's passphrase once.
agent-added = [NativeTerm] Done: until you sign out, ssh won't ask for these passphrases again.
agent-add-failed = [NativeTerm] ssh-add failed: { $error }. Is the ssh-agent service running?
