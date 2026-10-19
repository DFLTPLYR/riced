use super::{Pixmaps, TrayAction, TrayCache, TrayCommand, TrayItem, menu};
use crate::shell::Plant;
use iced::futures::{FutureExt, SinkExt, StreamExt, channel::mpsc};
use std::collections::HashMap;
use zbus::{interface, object_server::SignalEmitter, zvariant::OwnedValue};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";
const PATH: &str = "/StatusNotifierWatcher";

#[cfg(test)]
#[path = "tests.rs"]
mod integration;

struct Server {
    cache: TrayCache,
    registrations: mpsc::Sender<Registration>,
}

enum Registration {
    Item(String, String),
    Host(String),
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Server {
    async fn register_status_notifier_item(
        &self,
        service: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("missing sender".into()))?;
        let (bus, path) =
            address(&service, sender.as_str()).map_err(zbus::fdo::Error::InvalidArgs)?;
        self.registrations
            .clone()
            .send(Registration::Item(bus, path))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn register_status_notifier_host(&self, service: String) -> zbus::fdo::Result<()> {
        zbus::names::BusName::try_from(service.as_str())
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        self.registrations
            .clone()
            .send(Registration::Host(service))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.cache
            .snapshot()
            .into_iter()
            .map(|item| item.id)
            .collect()
    }
    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

fn address(service: &str, sender: &str) -> Result<(String, String), String> {
    let (bus, path) = if service.starts_with('/') {
        (sender, service)
    } else if let Some(index) = service.find('/') {
        (&service[..index], &service[index..])
    } else {
        (service, "/StatusNotifierItem")
    };
    zbus::names::BusName::try_from(bus).map_err(|e| e.to_string())?;
    zbus::zvariant::ObjectPath::try_from(path).map_err(|e| e.to_string())?;
    Ok((bus.into(), path.into()))
}

struct Remote {
    bus: String,
    path: String,
    owner: String,
    menu_path: Option<String>,
    theme_paths: Vec<std::path::PathBuf>,
    item: TrayItem,
}

impl Remote {
    async fn read(&mut self, conn: &zbus::Connection) -> zbus::Result<()> {
        let props_proxy = zbus::Proxy::new(
            conn,
            self.bus.as_str(),
            self.path.as_str(),
            "org.freedesktop.DBus.Properties",
        )
        .await?;
        let mut props: HashMap<String, OwnedValue> = props_proxy.call("GetAll", &(ITEM,)).await?;
        let text = |key: &str| {
            props
                .get(key)
                .and_then(|v| <&str>::try_from(v).ok())
                .unwrap_or("")
                .to_owned()
        };
        let icon_name = text("IconName");
        self.theme_paths = theme_paths(props.get("IconThemePath").map(|value| &**value));
        let icon_path = crate::icons::resolve(&icon_name, 24, &self.theme_paths)
            .map(|path| path.to_string_lossy().into_owned());
        let attention_name = text("AttentionIconName");
        let overlay_name = text("OverlayIconName");
        let menu_path = props
            .get("Menu")
            .and_then(|v| <&zbus::zvariant::ObjectPath<'_>>::try_from(v).ok())
            .filter(|path| path.as_str() != "/")
            .map(|path| path.as_str().to_owned());
        self.item.app_id = text("Id");
        self.item.title = text("Title");
        self.item.status = text("Status");
        self.item.icon_name = icon_name;
        self.item.icon_path = icon_path;
        self.item.is_menu = props
            .get("ItemIsMenu")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false);
        self.item.icon_pixmap = take_pixmap(&mut props, "IconPixmap");
        self.item.attention_icon = take_pixmap(&mut props, "AttentionIconPixmap")
            .or_else(|| crate::icons::image(&attention_name, 24, &self.theme_paths));
        self.item.overlay_icon = take_pixmap(&mut props, "OverlayIconPixmap")
            .or_else(|| crate::icons::image(&overlay_name, 24, &self.theme_paths));
        if let Some(tooltip) = props
            .remove("ToolTip")
            .and_then(|v| <(String, Pixmaps, String, String)>::try_from(v).ok())
        {
            self.item.tooltip_title = tooltip.2;
            self.item.tooltip = tooltip.3;
        } else {
            self.item.tooltip_title.clear();
            self.item.tooltip.clear();
        }
        if self.menu_path != menu_path {
            self.item.menu = None;
        }
        self.menu_path = menu_path;
        if self.menu_path.is_some() {
            self.refresh_menu(conn).await;
        }
        Ok(())
    }

    async fn refresh_menu(&mut self, conn: &zbus::Connection) {
        let Some(path) = &self.menu_path else {
            return;
        };
        let result = async {
            let proxy = zbus::Proxy::new(conn, self.bus.as_str(), path.as_str(), MENU).await?;
            menu::refresh(&proxy, &self.theme_paths).await
        }
        .await;
        match result {
            Ok(menu) => self.item.menu = Some(menu),
            Err(error) => tracing::debug!(item = %self.item.id, %error, "tray menu unavailable"),
        }
    }

    async fn command(&mut self, conn: &zbus::Connection, action: TrayAction) -> zbus::Result<()> {
        match action {
            TrayAction::RefreshIcons => self.read(conn).await?,
            TrayAction::Open(entry) | TrayAction::MenuEvent(entry) => {
                let Some(path) = &self.menu_path else {
                    return Ok(());
                };
                let proxy = zbus::Proxy::new(conn, self.bus.as_str(), path.as_str(), MENU).await?;
                if matches!(action, TrayAction::Open(_)) {
                    let _: bool = proxy.call("AboutToShow", &(entry,)).await?;
                } else if self
                    .item
                    .menu
                    .as_ref()
                    .is_some_and(|menu| menu.actionable(entry))
                {
                    let _: () = proxy
                        .call("Event", &(entry, "clicked", OwnedValue::from(0_i32), 0_u32))
                        .await?;
                }
                self.refresh_menu(conn).await;
            }
            _ => {
                let proxy =
                    zbus::Proxy::new(conn, self.bus.as_str(), self.path.as_str(), ITEM).await?;
                match action {
                    TrayAction::Activate(x, y) => {
                        proxy.call::<_, _, ()>("Activate", &(x, y)).await?
                    }
                    TrayAction::Secondary(x, y) => {
                        proxy.call::<_, _, ()>("SecondaryActivate", &(x, y)).await?
                    }
                    TrayAction::Context(x, y) => {
                        proxy.call::<_, _, ()>("ContextMenu", &(x, y)).await?
                    }
                    TrayAction::Scroll(delta, direction) => {
                        proxy
                            .call::<_, _, ()>("Scroll", &(delta, direction))
                            .await?
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

fn theme_paths(value: Option<&zbus::zvariant::Value<'_>>) -> Vec<std::path::PathBuf> {
    use zbus::zvariant::Value;
    let paths: Vec<_> = match value {
        Some(Value::Str(path)) => vec![path.as_str()],
        Some(Value::Array(paths)) => paths
            .iter()
            .filter_map(|path| <&str>::try_from(path).ok())
            .collect(),
        _ => Vec::new(),
    };
    paths
        .into_iter()
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect()
}

fn take_pixmap(
    props: &mut HashMap<String, OwnedValue>,
    key: &str,
) -> Option<iced::widget::image::Handle> {
    props
        .remove(key)
        .and_then(|v| Pixmaps::try_from(v).ok())
        .and_then(super::pixmap)
}

fn publish(cache: &TrayCache, remotes: &[Remote], tx: &mut mpsc::Sender<Plant>) {
    cache.replace(remotes.iter().map(|remote| remote.item.clone()).collect());
    let _ = tx.try_send(Plant::TrayChanged);
}

pub(crate) async fn serve(cache: TrayCache, mut tx: mpsc::Sender<Plant>) {
    let result = match zbus::connection::Builder::session() {
        Ok(builder) => run(&cache, &mut tx, builder).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        tracing::warn!(%error, "system tray stopped");
    }
    if let Ok(mut state) = cache.0.lock() {
        state.commands = None;
        state.items.clear();
    }
    let _ = tx.send(Plant::TrayChanged).await;
    std::future::pending::<()>().await;
}

async fn run(
    cache: &TrayCache,
    tx: &mut mpsc::Sender<Plant>,
    builder: zbus::connection::Builder<'_>,
) -> zbus::Result<()> {
    let (registration_tx, mut registrations) = mpsc::channel(64);
    let conn = builder
        .allow_name_replacements(false)
        .replace_existing_names(false)
        .name(WATCHER)?
        .serve_at(
            PATH,
            Server {
                cache: cache.clone(),
                registrations: registration_tx,
            },
        )?
        .method_timeout(std::time::Duration::from_secs(5))
        .build()
        .await?;
    let (commands_tx, mut commands) = mpsc::channel::<TrayCommand>(64);
    if let Ok(mut state) = cache.0.lock() {
        state.commands = Some(commands_tx);
    }
    // One signal stream also observes name loss, so registrations cannot leak
    // after an app exits. Property reads and menu refreshes stay off the UI task.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .build();
    let mut signals = zbus::MessageStream::for_match_rule(rule, &conn, Some(256)).await?;
    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    let emitter = SignalEmitter::new(&conn, PATH)?;
    Server::status_notifier_host_registered(&emitter).await?;
    let mut remotes: Vec<Remote> = Vec::new();
    loop {
        iced::futures::select! {
            registration = registrations.next().fuse() => {
                let Some(registration) = registration else { break; };
                match registration {
                    Registration::Host(service) => {
                        if dbus.get_name_owner(service.as_str().try_into()?).await.is_ok() {
                            Server::status_notifier_host_registered(&emitter).await?;
                        }
                    }
                    Registration::Item(bus, path) => {
                        let id = format!("{bus}{path}");
                        if remotes.iter().any(|remote| remote.item.id == id) { continue; }
                        let Ok(owner) = dbus.get_name_owner(bus.as_str().try_into()?).await else { continue; };
                        let mut remote = Remote { bus, path, owner: owner.to_string(), menu_path: None, theme_paths: Vec::new(),
                            item: TrayItem { id: id.clone(), ..Default::default() } };
                        match remote.read(&conn).await {
                            Ok(()) => { remotes.push(remote); publish(cache, &remotes, tx);
                                Server::status_notifier_item_registered(&emitter, &id).await?; }
                            Err(error) => tracing::debug!(%id, %error, "tray item unavailable"),
                        }
                    }
                }
            },
            command = commands.next().fuse() => {
                let Some(command) = command else { break; };
                if let Some(remote) = remotes.iter_mut().find(|remote| remote.item.id == command.id) {
                    if let Err(error) = remote.command(&conn, command.action).await {
                        tracing::warn!(item = %command.id, %error, "tray action failed");
                    }
                    publish(cache, &remotes, tx);
                }
            },
            message = signals.next().fuse() => {
                let Some(Ok(message)) = message else { break; };
                let header = message.header();
                let interface = header.interface().map(|v| v.as_str()).unwrap_or("");
                let member = header.member().map(|v| v.as_str()).unwrap_or("");
                if interface == "org.freedesktop.DBus" && member == "NameOwnerChanged" {
                    if let Ok((name, old, new)) = message.body().deserialize::<(String, String, String)>()
                        && !old.is_empty() && old != new {
                        let removed: Vec<_> = remotes.iter().filter(|r| r.bus == name || r.owner == name)
                            .map(|r| r.item.id.clone()).collect();
                        remotes.retain(|r| r.bus != name && r.owner != name);
                        if !removed.is_empty() { publish(cache, &remotes, tx); }
                        for id in removed { Server::status_notifier_item_unregistered(&emitter, &id).await?; }
                    }
                    continue;
                }
                let sender = header.sender().map(|v| v.as_str()).unwrap_or("");
                let path = header.path().map(|v| v.as_str()).unwrap_or("");
                let mut changed = false;
                for remote in remotes.iter_mut().filter(|remote| remote.owner == sender) {
                    if remote.path == path && (interface == ITEM || interface == "org.freedesktop.DBus.Properties") {
                        if let Err(error) = remote.read(&conn).await { tracing::debug!(%error, "tray refresh failed"); }
                        changed = true;
                    } else if remote.menu_path.as_deref() == Some(path) && interface == MENU {
                        remote.refresh_menu(&conn).await;
                        changed = true;
                    }
                }
                if changed { publish(cache, &remotes, tx); }
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn icon_theme_paths_accept_strings_and_arrays_without_relative_roots() {
        use zbus::zvariant::Value;
        let path = Value::from("/private/icons");
        assert_eq!(
            theme_paths(Some(&path)),
            [std::path::PathBuf::from("/private/icons")]
        );
        let paths = Value::from(vec!["relative", "/first", "/second"]);
        assert_eq!(
            theme_paths(Some(&paths)),
            [
                std::path::PathBuf::from("/first"),
                std::path::PathBuf::from("/second")
            ]
        );
        assert!(theme_paths(Some(&Value::from(12_u32))).is_empty());
    }
    #[test]
    fn registration_accepts_service_and_sender_relative_paths() {
        assert_eq!(
            address("/CustomItem", ":1.12").unwrap(),
            (":1.12".into(), "/CustomItem".into())
        );
        assert_eq!(
            address("org.test.Tray", ":1.12").unwrap(),
            ("org.test.Tray".into(), "/StatusNotifierItem".into())
        );
        assert!(address("", ":1.12").is_err());
    }
}
