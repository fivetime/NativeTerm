//! Saved passwords in the system's own store, as browsers keep theirs:
//!
//! - Windows: Credential Manager (`native_term_win::credentials`).
//! - macOS: the login keychain, a generic password per entry (service
//!   `NativeTerm`, the entry's name as its account).
//! - Linux: KWallet on KDE (the folder `NativeTerm`), else the desktop's
//!   Secret Service (GNOME Keyring, KDE's ksecretd, KeePassXC), over the
//!   session's D-Bus: as Chromium chooses and talks to them
//!   (`components/os_crypt/async/browser/freedesktop_secret_key_provider`),
//!   except that on KDE each KWallet there may be is tried, newest first:
//!   Chromium goes by `KDE_SESSION_VERSION`, which not every KDE sets.
//!
//! Off Windows an entry's value is a small JSON object (`user`, `secret`,
//! `comment`): the note NativeTerm keeps on a password (that a server
//! refused it) goes with it, as Credential Manager keeps it beside it.
//! Nothing is kept anywhere else, and nothing is encrypted by NativeTerm:
//! the store does that.

#[cfg(windows)]
pub use native_term_win::credentials::{delete, list, read, update, write, Saved};

/// Whether this system has a store to keep passwords in (on Linux: one
/// answers on the session's bus).
#[must_use]
pub fn supported() -> bool {
    #[cfg(any(windows, target_os = "macos"))]
    {
        true
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        secret::store().is_some()
    }
}

#[cfg(unix)]
mod unix {
    use std::io;

    /// A saved password and its note.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Saved {
        pub user: String,
        pub secret: String,
        /// NativeTerm's note on it (e.g. that a server refused it).
        pub comment: String,
    }

    /// What an entry holds, as the store keeps it.
    pub(crate) fn encode(saved: &Saved) -> String {
        serde_json::json!({ "user": saved.user, "secret": saved.secret, "comment": saved.comment }).to_string()
    }

    /// An entry's value read back; one that is not NativeTerm's JSON is a
    /// password alone (written by hand in the store).
    pub(crate) fn decode(value: &str) -> Saved {
        let field = |v: &serde_json::Value, name: &str| v.get(name).and_then(|f| f.as_str()).unwrap_or("").to_string();
        match serde_json::from_str::<serde_json::Value>(value) {
            Ok(v) if v.is_object() && v.get("secret").is_some_and(serde_json::Value::is_string) => {
                Saved { user: field(&v, "user"), secret: field(&v, "secret"), comment: field(&v, "comment") }
            }
            _ => Saved { secret: value.to_string(), ..Saved::default() },
        }
    }

    #[cfg(target_os = "macos")]
    use super::keychain as store;
    #[cfg(not(target_os = "macos"))]
    use super::secret as store;

    /// The credential named `target`, if there is one.
    pub fn read(target: &str) -> io::Result<Option<Saved>> {
        Ok(store::read(target)?.map(|value| decode(&value)))
    }

    pub fn write(target: &str, saved: &Saved) -> io::Result<()> {
        store::write(target, &encode(saved))
    }

    /// The names of the credentials starting with `prefix`.
    pub fn list(prefix: &str) -> io::Result<Vec<String>> {
        let mut names: Vec<String> = store::names()?.into_iter().filter(|n| n.starts_with(prefix)).collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Whether there was one to delete.
    pub fn delete(target: &str) -> io::Result<bool> {
        store::delete(target)
    }

    /// Whether there was one to change.
    pub fn update(target: &str, change: impl FnOnce(&mut Saved)) -> io::Result<bool> {
        let Some(mut saved) = read(target)? else { return Ok(false) };
        change(&mut saved);
        write(target, &saved)?;
        Ok(true)
    }
}

#[cfg(unix)]
pub use unix::{delete, list, read, update, write, Saved};

/// The login keychain: a generic password per entry.
#[cfg(target_os = "macos")]
mod keychain {
    use std::io;

    use security_framework::item::{ItemClass, ItemSearchOptions, Limit, SearchResult};
    use security_framework::passwords;

    /// The service every entry of NativeTerm's is under.
    const SERVICE: &str = "NativeTerm";
    /// errSecItemNotFound: "The specified item could not be found in the
    /// keychain."
    const NOT_FOUND: i32 = -25300;

    fn failed(e: security_framework::base::Error) -> io::Error {
        io::Error::other(format!("keychain: {e}"))
    }

    pub fn read(name: &str) -> io::Result<Option<String>> {
        match passwords::get_generic_password(SERVICE, name) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(failed(e)),
        }
    }

    pub fn write(name: &str, value: &str) -> io::Result<()> {
        passwords::set_generic_password(SERVICE, name, value.as_bytes()).map_err(failed)
    }

    pub fn delete(name: &str) -> io::Result<bool> {
        match passwords::delete_generic_password(SERVICE, name) {
            Ok(()) => Ok(true),
            Err(e) if e.code() == NOT_FOUND => Ok(false),
            Err(e) => Err(failed(e)),
        }
    }

    pub fn names() -> io::Result<Vec<String>> {
        let found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(SERVICE)
            .load_attributes(true)
            .limit(Limit::All)
            .search();
        let results = match found {
            Ok(results) => results,
            Err(e) if e.code() == NOT_FOUND => return Ok(Vec::new()),
            Err(e) => return Err(failed(e)),
        };
        Ok(results
            .iter()
            .filter(|r| matches!(r, SearchResult::Dict(_)))
            .filter_map(|r| r.simplify_dict()?.get("acct").cloned())
            .collect())
    }
}

/// KWallet or the Secret Service, over the session's D-Bus.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) mod secret {
    use std::collections::HashMap;
    use std::io;
    use std::sync::OnceLock;

    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    /// Where the entries are: KWallet's folder, the application KWallet
    /// is told it is, the Secret Service items' attributes.
    const FOLDER: &str = "NativeTerm";
    const APPLICATION: &str = "nativeterm";
    const NAME_ATTRIBUTE: &str = "nativeterm-target";

    const SECRETS: &str = "org.freedesktop.secrets";
    const SECRETS_PATH: &str = "/org/freedesktop/secrets";
    const SERVICE: &str = "org.freedesktop.Secret.Service";
    const COLLECTION: &str = "org.freedesktop.Secret.Collection";
    const ITEM: &str = "org.freedesktop.Secret.Item";
    const PROMPT: &str = "org.freedesktop.Secret.Prompt";
    const SESSION: &str = "org.freedesktop.Secret.Session";
    const KWALLET: &str = "org.kde.KWallet";
    /// Newest first: (bus name, object path).
    const KWALLETS: [(&str, &str); 3] = [
        ("org.kde.kwalletd6", "/modules/kwalletd6"),
        ("org.kde.kwalletd5", "/modules/kwalletd5"),
        ("org.kde.kwalletd", "/modules/kwalletd"),
    ];

    fn failed(what: &str, e: impl std::fmt::Display) -> io::Error {
        io::Error::other(format!("{what}: {e}"))
    }

    fn none() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "no password store on this desktop (KWallet, Secret Service)")
    }

    /// The store this desktop has.
    pub enum Store {
        KWallet { bus: Connection, name: &'static str, path: &'static str },
        Secrets { bus: Connection },
    }

    /// The store, found once: KWallet on KDE (as Chromium), the Secret
    /// Service elsewhere, and the other where the first is not there.
    pub fn store() -> Option<&'static Store> {
        static STORE: OnceLock<Option<Store>> = OnceLock::new();
        STORE
            .get_or_init(|| {
                let bus = Connection::session().ok()?;
                let kde = std::env::var("XDG_CURRENT_DESKTOP")
                    .is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("KDE")));
                let kwallet = || {
                    KWALLETS
                        .iter()
                        .find(|(name, path)| kwallet_enabled(&bus, name, path))
                        .map(|(name, path)| Store::KWallet { bus: bus.clone(), name, path })
                };
                let secrets = || has_name(&bus, SECRETS).then(|| Store::Secrets { bus: bus.clone() });
                if kde {
                    kwallet().or_else(secrets)
                } else {
                    secrets().or_else(kwallet)
                }
            })
            .as_ref()
    }

    /// Whether `name` is on the bus, or would be started for a call.
    fn has_name(bus: &Connection, name: &str) -> bool {
        let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(bus) else { return false };
        let Ok(bus_name) = zbus::names::BusName::try_from(name) else { return false };
        dbus.name_has_owner(bus_name).unwrap_or(false)
            || dbus.list_activatable_names().is_ok_and(|names| names.iter().any(|n| n.as_str() == name))
    }

    fn kwallet_enabled(bus: &Connection, name: &'static str, path: &'static str) -> bool {
        has_name(bus, name)
            && Proxy::new(bus, name, path, KWALLET)
                .and_then(|p| p.call::<_, _, bool>("isEnabled", &()))
                .unwrap_or(false)
    }

    pub fn read(name: &str) -> io::Result<Option<String>> {
        match store().ok_or_else(none)? {
            Store::KWallet { bus, name: service, path } => {
                let wallet = KWallet::open(bus, service, path)?;
                let found = match wallet.has_entry(name) {
                    Ok(true) => wallet.read(name).map(Some),
                    Ok(false) => Ok(None),
                    Err(e) => Err(e),
                };
                wallet.close();
                found
            }
            Store::Secrets { bus } => {
                let secrets = Secrets::open(bus)?;
                match secrets.items(Some(name))?.first() {
                    Some(item) => secrets.secret(item).map(Some),
                    None => Ok(None),
                }
            }
        }
    }

    pub fn write(name: &str, value: &str) -> io::Result<()> {
        match store().ok_or_else(none)? {
            Store::KWallet { bus, name: service, path } => {
                let wallet = KWallet::open(bus, service, path)?;
                let written = wallet.write(name, value);
                wallet.close();
                written
            }
            Store::Secrets { bus } => Secrets::open(bus)?.write(name, value),
        }
    }

    pub fn delete(name: &str) -> io::Result<bool> {
        match store().ok_or_else(none)? {
            Store::KWallet { bus, name: service, path } => {
                let wallet = KWallet::open(bus, service, path)?;
                let removed = match wallet.has_entry(name) {
                    Ok(true) => wallet.remove(name).map(|()| true),
                    other => other,
                };
                wallet.close();
                removed
            }
            Store::Secrets { bus } => {
                let secrets = Secrets::open(bus)?;
                let items = secrets.items(Some(name))?;
                for item in &items {
                    secrets.delete(item)?;
                }
                Ok(!items.is_empty())
            }
        }
    }

    pub fn names() -> io::Result<Vec<String>> {
        match store() {
            None => Ok(Vec::new()),
            Some(Store::KWallet { bus, name: service, path }) => {
                let wallet = KWallet::open(bus, service, path)?;
                let names = wallet.names();
                wallet.close();
                names
            }
            Some(Store::Secrets { bus }) => {
                let secrets = Secrets::open(bus)?;
                let mut names = Vec::new();
                for item in secrets.items(None)? {
                    if let Some(name) = secrets.attributes(&item)?.remove(NAME_ATTRIBUTE) {
                        names.push(name);
                    }
                }
                Ok(names)
            }
        }
    }

    /// An open wallet (KWallet asks the person to unlock it where it is
    /// locked: `open` waits for that).
    struct KWallet {
        proxy: Proxy<'static>,
        handle: i32,
    }

    impl KWallet {
        fn open(bus: &Connection, name: &'static str, path: &'static str) -> io::Result<KWallet> {
            let proxy = Proxy::new(bus, name, path, KWALLET).map_err(|e| failed("KWallet", e))?;
            let wallet: String = proxy.call("networkWallet", &()).map_err(|e| failed("KWallet networkWallet", e))?;
            let handle: i32 =
                proxy.call("open", &(wallet.as_str(), 0_i64, APPLICATION)).map_err(|e| failed("KWallet open", e))?;
            if handle < 0 {
                return Err(io::Error::other("KWallet: the wallet was not opened"));
            }
            let wallet = KWallet { proxy, handle };
            let has = || wallet.call::<_, bool>("hasFolder", &(handle, FOLDER, APPLICATION));
            // (false where another made it a moment before: it is there then)
            if !has()? && !wallet.call::<_, bool>("createFolder", &(handle, FOLDER, APPLICATION))? && !has()? {
                return Err(io::Error::other("KWallet: its folder could not be made"));
            }
            Ok(wallet)
        }

        fn call<A, R>(&self, method: &str, args: &A) -> io::Result<R>
        where
            A: serde::ser::Serialize + zbus::zvariant::DynamicType,
            R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
        {
            self.proxy.call(method, args).map_err(|e| failed(&format!("KWallet {method}"), e))
        }

        fn has_entry(&self, key: &str) -> io::Result<bool> {
            self.call("hasEntry", &(self.handle, FOLDER, key, APPLICATION))
        }

        fn read(&self, key: &str) -> io::Result<String> {
            self.call("readPassword", &(self.handle, FOLDER, key, APPLICATION))
        }

        fn write(&self, key: &str, value: &str) -> io::Result<()> {
            let code: i32 = self.call("writePassword", &(self.handle, FOLDER, key, value, APPLICATION))?;
            if code == 0 {
                Ok(())
            } else {
                Err(io::Error::other(format!("KWallet writePassword: {code}")))
            }
        }

        fn remove(&self, key: &str) -> io::Result<()> {
            let code: i32 = self.call("removeEntry", &(self.handle, FOLDER, key, APPLICATION))?;
            if code == 0 {
                Ok(())
            } else {
                Err(io::Error::other(format!("KWallet removeEntry: {code}")))
            }
        }

        fn names(&self) -> io::Result<Vec<String>> {
            self.call("entryList", &(self.handle, FOLDER, APPLICATION))
        }

        fn close(self) {
            let _ = self.proxy.call::<_, _, i32>("close", &(self.handle, false, APPLICATION));
        }
    }

    /// The Secret Service's default collection, with a plain session: what
    /// goes over the session's bus is the person's own (as Chromium does).
    struct Secrets {
        bus: Connection,
        service: Proxy<'static>,
        collection: OwnedObjectPath,
        session: OwnedObjectPath,
    }

    /// A secret as the Secret Service carries it: (session, parameters,
    /// value, content type).
    type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

    impl Secrets {
        fn open(bus: &Connection) -> io::Result<Secrets> {
            let service = Proxy::new(bus, SECRETS, SECRETS_PATH, SERVICE).map_err(|e| failed("Secret Service", e))?;
            let (_, session): (OwnedValue, OwnedObjectPath) = service
                .call("OpenSession", &("plain", Value::from("")))
                .map_err(|e| failed("Secret Service OpenSession", e))?;
            let collection: OwnedObjectPath =
                service.call("ReadAlias", &("default",)).map_err(|e| failed("Secret Service ReadAlias", e))?;
            if collection.as_str() == "/" {
                return Err(io::Error::other("Secret Service: there is no default keyring"));
            }
            let secrets = Secrets { bus: bus.clone(), service, collection, session };
            secrets.unlock(&secrets.collection)?;
            Ok(secrets)
        }

        /// Unlocked, asking the person where it is locked.
        fn unlock(&self, path: &OwnedObjectPath) -> io::Result<()> {
            let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
                self.service.call("Unlock", &(vec![path.clone()],)).map_err(|e| failed("Secret Service Unlock", e))?;
            self.prompt(&prompt)
        }

        /// A prompt the service asks the person, waited for; nothing where
        /// it needs none ("/").
        fn prompt(&self, path: &OwnedObjectPath) -> io::Result<()> {
            if path.as_str() == "/" {
                return Ok(());
            }
            let prompt = Proxy::new(&self.bus, SECRETS, path.clone(), PROMPT).map_err(|e| failed("prompt", e))?;
            let mut completed = prompt.receive_signal("Completed").map_err(|e| failed("prompt", e))?;
            prompt.call::<_, _, ()>("Prompt", &("",)).map_err(|e| failed("prompt", e))?;
            let signal = completed.next().ok_or_else(|| io::Error::other("the prompt went away"))?;
            let (dismissed, _): (bool, OwnedValue) = signal.body().deserialize().map_err(|e| failed("prompt", e))?;
            if dismissed {
                Err(io::Error::other("the keyring was not unlocked"))
            } else {
                Ok(())
            }
        }

        fn attributes_of(name: Option<&str>) -> HashMap<&str, &str> {
            let mut attributes = HashMap::from([("application", APPLICATION)]);
            if let Some(name) = name {
                attributes.insert(NAME_ATTRIBUTE, name);
            }
            attributes
        }

        fn collection(&self) -> io::Result<Proxy<'static>> {
            Proxy::new(&self.bus, SECRETS, self.collection.clone(), COLLECTION).map_err(|e| failed("Secret Service", e))
        }

        /// NativeTerm's items (`name`'s only, where given).
        fn items(&self, name: Option<&str>) -> io::Result<Vec<OwnedObjectPath>> {
            self.collection()?
                .call("SearchItems", &(Secrets::attributes_of(name),))
                .map_err(|e| failed("SearchItems", e))
        }

        fn attributes(&self, item: &OwnedObjectPath) -> io::Result<HashMap<String, String>> {
            let item = Proxy::new(&self.bus, SECRETS, item.clone(), ITEM).map_err(|e| failed("item", e))?;
            item.get_property("Attributes").map_err(|e| failed("item Attributes", e))
        }

        fn secret(&self, item: &OwnedObjectPath) -> io::Result<String> {
            let secrets: HashMap<OwnedObjectPath, Secret> = self
                .service
                .call("GetSecrets", &(vec![item.clone()], &self.session))
                .map_err(|e| failed("GetSecrets", e))?;
            let (_, _, value, _) = secrets.get(item).ok_or_else(|| io::Error::other("the item has no secret"))?;
            Ok(String::from_utf8_lossy(value).into_owned())
        }

        fn write(&self, name: &str, value: &str) -> io::Result<()> {
            let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
            properties.insert("org.freedesktop.Secret.Item.Label", Value::from(format!("NativeTerm: {name}")));
            properties
                .insert("org.freedesktop.Secret.Item.Attributes", Value::from(Secrets::attributes_of(Some(name))));
            let secret = (&self.session, Vec::<u8>::new(), value.as_bytes().to_vec(), "text/plain");
            let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = self
                .collection()?
                .call("CreateItem", &(properties, secret, true))
                .map_err(|e| failed("CreateItem", e))?;
            self.prompt(&prompt)
        }

        fn delete(&self, item: &OwnedObjectPath) -> io::Result<()> {
            let item = Proxy::new(&self.bus, SECRETS, item.clone(), ITEM).map_err(|e| failed("item", e))?;
            let prompt: OwnedObjectPath = item.call("Delete", &()).map_err(|e| failed("Delete", e))?;
            self.prompt(&prompt)
        }
    }

    impl Drop for Secrets {
        fn drop(&mut self) {
            if let Ok(session) = Proxy::new(&self.bus, SECRETS, self.session.clone(), SESSION) {
                let _ = session.call::<_, _, ()>("Close", &());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nothing_saved_under_a_name_nobody_uses() {
        if !super::supported() {
            return;
        }
        let name = format!("NativeTerm-Tests-os-{}", std::process::id());
        assert_eq!(super::read(&name).unwrap(), None);
        assert!(!super::delete(&name).unwrap());
    }

    /// Written, listed, read, changed and deleted, in the store there is
    /// (the test's entry is removed, whatever happens).
    #[test]
    fn an_entry_goes_through_the_store() {
        if !super::supported() {
            return;
        }
        let name = format!("NativeTerm-Tests-os-entry-{}", std::process::id());
        let saved = super::Saved { user: "root".into(), secret: "p@ss wörd \"1\"".into(), comment: String::new() };
        // (a login keychain reached over ssh is locked to it: it can be read
        // there, written only in the desktop's session)
        if let Err(e) = super::write(&name, &saved) {
            if cfg!(target_os = "macos") && e.to_string().contains("User interaction is not allowed") {
                eprintln!("the keychain can't be written from here: {e}");
                return;
            }
            panic!("{e}");
        }
        let result = std::panic::catch_unwind(|| {
            assert_eq!(super::read(&name).unwrap().as_ref(), Some(&saved));
            assert!(super::list("NativeTerm-Tests-os-entry-").unwrap().contains(&name));
            assert!(super::update(&name, |s| s.comment = "refused".into()).unwrap());
            let changed = super::read(&name).unwrap().unwrap();
            assert_eq!((changed.comment.as_str(), changed.secret.as_str()), ("refused", saved.secret.as_str()));
        });
        let deleted = super::delete(&name);
        result.unwrap();
        assert!(deleted.unwrap());
        assert_eq!(super::read(&name).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn what_an_entry_holds_off_windows() {
        let saved = super::Saved { user: "u".into(), secret: "a\"b\\c".into(), comment: "x".into() };
        assert_eq!(super::unix::decode(&super::unix::encode(&saved)), saved);
        // one written by hand: a password alone
        assert_eq!(super::unix::decode("hunter2").secret, "hunter2");
        assert_eq!(super::unix::decode("{\"a\":1}").secret, "{\"a\":1}");
    }
}
