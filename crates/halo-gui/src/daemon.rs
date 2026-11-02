//! D-Bus client of the daemon. zbus 6 is async, so the window carries its own single-thread
//! tokio runtime and blocks on each call: they are short and come from the Qt thread. The window
//! reads everything at once in `snapshot` (a QML timer calls it every second) and sends each
//! action as one call.

use zbus::{Connection, Proxy};

const NAME: &str = "org.halo.Halo1";
const PATH: &str = "/org/halo/Halo1";

pub struct Daemon {
    rt: tokio::runtime::Runtime,
    conn: Connection,
}

#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    pub backend: String,
    pub frame: Vec<String>,
    pub base: Vec<String>,
    pub brightness: u8,
    pub effect: String,
    pub speed: String,
    pub hardware: bool,
    pub effects: Vec<String>,
    pub games_enabled: bool,
    /// (game, bridge, connected, may paint)
    pub games: Vec<(String, String, bool, bool)>,
    pub config: Vec<(String, String)>,
    pub layers: Vec<String>,
    /// (AppID, name) of the running Steam game; empty if none.
    pub current_game: (String, String),
    /// (AppID, name, value)
    pub profiles: Vec<(String, String, String)>,
    /// (AppID, name) of the installed Steam games.
    pub installed: Vec<(String, String)>,
    /// (AppID, name, halo|none|foreign) of the Proton prefixes.
    pub prefixes: Vec<(String, String, String)>,
}

impl Daemon {
    pub fn connect() -> zbus::Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| zbus::Error::InputOutput(std::sync::Arc::new(e)))?;
        let conn = rt.block_on(Connection::session())?;
        let d = Self { rt, conn };
        // Fail here, not in the first snapshot, if the daemon is not there.
        d.rt.block_on(async { d.proxy().await?.get_property::<String>("Backend").await })?;
        Ok(d)
    }

    async fn proxy(&self) -> zbus::Result<Proxy<'_>> {
        Proxy::new(&self.conn, NAME, PATH, NAME).await
    }

    pub fn snapshot(&self) -> zbus::Result<Snapshot> {
        self.rt.block_on(async {
            let p = self.proxy().await?;
            Ok(Snapshot {
                backend: p.get_property("Backend").await?,
                frame: p.get_property("Frame").await?,
                base: p.get_property("Base").await?,
                brightness: p.get_property("Brightness").await?,
                effect: p.get_property("Effect").await?,
                speed: p.get_property("EffectSpeed").await?,
                hardware: p.get_property("EffectHardware").await?,
                effects: p.get_property("Effects").await?,
                games_enabled: p.get_property("GamesEnabled").await?,
                games: p.get_property("Games").await?,
                config: p.call("Config", &()).await?,
                layers: p.get_property("Layers").await?,
                current_game: p.get_property("CurrentGame").await?,
                profiles: p.get_property("Profiles").await?,
                installed: p.get_property("InstalledGames").await?,
                prefixes: p.get_property("Prefixes").await?,
            })
        })
    }

    /// A call whose reply does not matter. The error comes ready to show.
    pub fn call<B>(&self, method: &str, body: &B) -> Result<(), String>
    where
        B: serde::ser::Serialize + zbus::zvariant::DynamicType,
    {
        self.rt
            .block_on(async { self.proxy().await?.call_method(method, body).await.map(|_| ()) })
            .map_err(|e| {
                let text = e.to_string();
                match text.strip_prefix("org.freedesktop.DBus.Error.").and_then(|t| t.split_once(": ")) {
                    Some((_, msg)) => msg.to_string(),
                    None => text,
                }
            })
    }
}
